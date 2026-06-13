//! Configuration state handler.
//!
//! Wire choreography:
//!
//! ```text
//! S → C  Clientbound Known Packs   (advertise `minecraft:core:<version>`)
//! C → S  Serverbound Known Packs   (the subset the client also has)
//! S → C  Registry Data × N         (one per built-in registry; all entries
//!                                   are `has_data = false` — client reads
//!                                   built-in data via the matched pack)
//! S → C  Finish Configuration
//! C → S  Acknowledge Finish Configuration
//!        → state transitions to Play
//! ```
//!
//! Inbound configuration packets that aren't `Serverbound Known Packs`
//! or `Acknowledge Finish Configuration` are read, logged at debug
//! level, and discarded — robust to optional `Client Information` and
//! `Plugin Message` traffic the client may emit at any point.

use bytes::{Buf, Bytes, BytesMut};
use mc_data::VanillaData;
use mc_data::tags::TagsData;
use mc_extension::DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES;
use mc_protocol::codec::{DEFAULT_MAX_STRING_LEN, ReadMc};
use mc_protocol::frame::Compression;
use mc_protocol::packets::CustomPayload;
use mc_protocol::packets::Packet;
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundKnownPacks, FinishConfiguration, KnownPackEntry,
    RegistryData, RegistryEntry, ServerboundClientInformation, ServerboundCustomPayload,
    ServerboundKnownPacks, ServerboundResourcePack, UpdateTags, UpdateTagsEntry,
    UpdateTagsRegistry,
};
use mc_protocol::{State, TARGET_RELEASE};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, info, warn};

use crate::connection::{PRE_PLAY_READ_TIMEOUT, read_frame_with_timeout, write_packet};
use crate::error::ConnectionError;
use crate::login::LoggedInProfile;

const MAX_IGNORED_CONFIGURATION_PACKETS: usize = 32;

/// The Known Packs entry we advertise as the data pack the running
/// game version corresponds to. Built from [`TARGET_RELEASE`] so a
/// version bump only touches one place.
fn server_known_pack() -> KnownPackEntry {
    KnownPackEntry {
        namespace: "minecraft".into(),
        id: "core".into(),
        version: TARGET_RELEASE.to_string(),
    }
}

pub(crate) async fn handle<R, W>(
    reader: &mut R,
    writer: &mut W,
    buf: &mut BytesMut,
    compression: Compression,
    profile: &LoggedInProfile,
    data: &VanillaData,
    tags: &TagsData,
) -> Result<(), ConnectionError>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    debug!(player = %profile.name, uuid = %profile.uuid, "entering Configuration state");
    // Step 1: advertise our known data packs.
    let our_pack = server_known_pack();
    write_packet(
        writer,
        &ClientboundKnownPacks {
            packs: vec![our_pack.clone()],
        },
        compression,
    )
    .await?;

    // Step 2: read frames until we see the client's KnownPacks response.
    let mut ignored_packets = 0usize;
    let client_packs = loop {
        let frame = read_frame_with_timeout(
            reader,
            buf,
            compression,
            State::Configuration,
            PRE_PLAY_READ_TIMEOUT,
        )
        .await?;
        if frame.id == ServerboundKnownPacks::ID {
            let mut body = frame.body;
            let parsed = ServerboundKnownPacks::decode(&mut body)?;
            break parsed.packs;
        }
        ignored_packets += 1;
        if ignored_packets > MAX_IGNORED_CONFIGURATION_PACKETS {
            return Err(ConnectionError::IgnoredPacketBudgetExceeded {
                state: State::Configuration,
                max: MAX_IGNORED_CONFIGURATION_PACKETS,
            });
        }
        if frame.id == ServerboundClientInformation::ID {
            let mut body = frame.body;
            let information = ServerboundClientInformation::decode(&mut body)?.information;
            debug!(
                language = %information.language,
                requested_view_distance = information.view_distance,
                chat_visibility = ?information.chat_visibility,
                chat_colors = information.chat_colors,
                model_customisation = information.model_customisation,
                main_hand = ?information.main_hand,
                text_filtering_enabled = information.text_filtering_enabled,
                allows_listing = information.allows_listing,
                particle_status = ?information.particle_status,
                "client information noted during Configuration"
            );
            continue;
        }
        if frame.id == ServerboundCustomPayload::ID {
            handle_configuration_custom_payload(frame.body, "before_known_packs")?;
            continue;
        }
        if frame.id == ServerboundResourcePack::ID {
            let mut body = frame.body;
            let status = ServerboundResourcePack::decode(&mut body)?.status;
            debug!(
                id = %status.id,
                action = ?status.action,
                terminal = status.action.is_terminal(),
                "resource-pack status noted during Configuration"
            );
            continue;
        }
        debug!(
            id = format!("{:#04x}", frame.id),
            len = frame.body.len(),
            "ignored unsolicited Configuration packet"
        );
    };

    // If the client did not echo back our pack the `has_data = false`
    // shortcut is unsound: they have no confirmed built-in data to fall
    // back on and Solaris cannot yet send full registry NBT payloads.
    let client_has_our_pack = client_packs.iter().any(|p| {
        p.namespace == our_pack.namespace && p.id == our_pack.id && p.version == our_pack.version
    });
    if !client_has_our_pack {
        let advertised_pack = format!(
            "{}:{}:{}",
            our_pack.namespace, our_pack.id, our_pack.version
        );
        warn!(
            player = %profile.name,
            advertised = %advertised_pack,
            client_packs = ?client_packs,
            "client did not acknowledge our core pack; closing before unsound Registry Data"
        );
        return Err(ConnectionError::MissingKnownPack {
            advertised: advertised_pack,
        });
    }

    // Step 3: send a Registry Data packet for every built-in registry,
    // with every entry having `has_data = false` (== use built-in).
    for registry in data.registries() {
        let entries = registry
            .entries
            .iter()
            .map(|name| RegistryEntry {
                name: name.clone(),
                nbt_payload: None,
            })
            .collect();
        write_packet(
            writer,
            &RegistryData {
                registry_id: registry.id.clone(),
                entries,
            },
            compression,
        )
        .await?;
    }
    debug!(
        registries = data.registry_count(),
        entries = data.entry_count(),
        "sent Registry Data"
    );

    // Step 3.5: ship the tag set. Mojang's built-in datapack contains
    // enchantment definitions etc. that reference `#minecraft:item`,
    // `#minecraft:entity_type` and `#minecraft:block` tags; without
    // this packet the client kicks itself on `FinishConfiguration`
    // with "Unbound tags" because nothing populated those references.
    let tag_packet = UpdateTags {
        registries: tags
            .registries
            .iter()
            .map(|(registry, tag_map)| UpdateTagsRegistry {
                registry: registry.clone(),
                tags: tag_map
                    .iter()
                    .map(|(tag_id, entries)| UpdateTagsEntry {
                        tag: tag_id.clone(),
                        entries: entries.clone(),
                    })
                    .collect(),
            })
            .collect(),
    };
    let tag_count = tags.total_tags();
    let tag_entries = tags.total_entries();
    write_packet(writer, &tag_packet, compression).await?;
    debug!(
        registries = tag_packet.registries.len(),
        tags = tag_count,
        entries = tag_entries,
        "sent Update Tags",
    );

    // Step 4: tell the client we are done configuring.
    write_packet(writer, &FinishConfiguration, compression).await?;

    // Step 5: wait for AcknowledgeFinishConfiguration, ignoring
    // anything else.
    let mut ignored_packets = 0usize;
    loop {
        let frame = read_frame_with_timeout(
            reader,
            buf,
            compression,
            State::Configuration,
            PRE_PLAY_READ_TIMEOUT,
        )
        .await?;
        if frame.id == AcknowledgeFinishConfiguration::ID {
            break;
        }
        ignored_packets += 1;
        if ignored_packets > MAX_IGNORED_CONFIGURATION_PACKETS {
            return Err(ConnectionError::IgnoredPacketBudgetExceeded {
                state: State::Configuration,
                max: MAX_IGNORED_CONFIGURATION_PACKETS,
            });
        }
        if frame.id == ServerboundClientInformation::ID {
            let mut body = frame.body;
            let information = ServerboundClientInformation::decode(&mut body)?.information;
            debug!(
                language = %information.language,
                requested_view_distance = information.view_distance,
                "client information noted while waiting for Configuration ack"
            );
            continue;
        }
        if frame.id == ServerboundCustomPayload::ID {
            handle_configuration_custom_payload(frame.body, "before_finish_ack")?;
            continue;
        }
        if frame.id == ServerboundResourcePack::ID {
            let mut body = frame.body;
            let status = ServerboundResourcePack::decode(&mut body)?.status;
            debug!(
                id = %status.id,
                action = ?status.action,
                terminal = status.action.is_terminal(),
                "resource-pack status noted while waiting for Configuration ack"
            );
            continue;
        }
        debug!(
            id = format!("{:#04x}", frame.id),
            "ignored Configuration packet while waiting for ack"
        );
    }

    info!(
        player = %profile.name,
        client_pack_count = client_packs.len(),
        "configuration complete; entering Play state"
    );

    Ok(())
}

fn handle_configuration_custom_payload(
    mut body: Bytes,
    context: &'static str,
) -> Result<(), ConnectionError> {
    if body.len() > DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES {
        warn!(
            len = body.len(),
            max = DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES,
            context,
            "oversized Configuration custom payload rejected before decode"
        );
        return Ok(());
    }

    let channel = body.read_identifier()?;
    if channel == *CustomPayload::brand_channel() {
        let brand = body.read_string(DEFAULT_MAX_STRING_LEN)?;
        debug!(brand = %brand, context, "client brand noted during Configuration");
        return Ok(());
    }

    let payload_len = body.remaining();
    debug!(
        channel = %channel.as_str(),
        len = payload_len,
        context,
        "Configuration custom payload ignored"
    );

    Ok(())
}
