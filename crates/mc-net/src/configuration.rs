//! Configuration state handler.
//!
//! Wire choreography:
//!
//! ```text
//! S → C  Clientbound Known Packs   (advertise `minecraft:core:<version>`)
//! C → S  Serverbound Known Packs   (the subset the client also has)
//! S → C  Registry Data × N         (one per built-in registry; entries use
//!                                   the matched client pack or full sidecar
//!                                   Network-NBT payloads)
//! S → C  Finish Configuration
//! C → S  Acknowledge Finish Configuration
//!        → state transitions to Play
//! ```
//!
//! Inbound configuration packets that aren't `Serverbound Known Packs`
//! or `Acknowledge Finish Configuration` are read and handled without
//! blocking the handshake — robust to optional `Client Information` and
//! `Plugin Message` traffic the client may emit at any point.

use std::sync::Arc;

use bytes::{Buf, Bytes, BytesMut};
use mc_data::VanillaData;
use mc_data::tags::TagsData;
use mc_extension::{CustomPayloadPolicy, DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES};
use mc_nbt::Tag;
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
use mc_protocol::{CodecError, State, TARGET_RELEASE};
use mc_world::{ChunkGeometry, OVERWORLD_GEOMETRY};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, info, warn};

use crate::connection::{PRE_PLAY_READ_TIMEOUT, read_frame_with_timeout, write_packet};
use crate::error::ConnectionError;
use crate::login::LoggedInProfile;

const MAX_IGNORED_CONFIGURATION_PACKETS: usize = 32;

fn override_overworld_dimension_geometry(
    payload: Arc<[u8]>,
    geometry: ChunkGeometry,
) -> Result<Arc<[u8]>, ConnectionError> {
    if geometry == OVERWORLD_GEOMETRY {
        return Ok(payload);
    }

    let mut cursor = payload.as_ref();
    let mut root = mc_nbt::read_network(&mut cursor).map_err(CodecError::from)?;
    if !cursor.is_empty() {
        return Err(
            CodecError::NotSupported("overworld dimension payload has trailing NBT bytes").into(),
        );
    }
    let Tag::Compound(fields) = &mut root else {
        unreachable!("read_network only returns a compound root");
    };
    replace_dimension_int(fields.as_mut_slice(), "min_y", geometry.min_y())?;
    replace_dimension_int(fields.as_mut_slice(), "height", geometry.height())?;
    replace_dimension_int(fields.as_mut_slice(), "logical_height", geometry.height())?;

    let mut encoded = Vec::with_capacity(payload.len());
    mc_nbt::write_network(&mut encoded, &root).map_err(CodecError::from)?;
    Ok(encoded.into())
}

fn replace_dimension_int(
    fields: &mut [(String, Tag)],
    name: &'static str,
    value: i32,
) -> Result<(), ConnectionError> {
    let Some((_, field)) = fields.iter_mut().find(|(field, _)| field == name) else {
        return Err(CodecError::NotSupported(
            "overworld dimension payload is missing a geometry field",
        )
        .into());
    };
    let Tag::Int(field) = field else {
        return Err(
            CodecError::NotSupported("overworld dimension geometry field is not an int").into(),
        );
    };
    *field = value;
    Ok(())
}

fn prepare_registry_entry_payload(
    payload: Option<Arc<[u8]>>,
    registry: &str,
    entry: &str,
    send_full_registry_data: bool,
    chunk_geometry: ChunkGeometry,
) -> Result<Option<Arc<[u8]>>, ConnectionError> {
    let override_overworld = chunk_geometry != OVERWORLD_GEOMETRY
        && registry == "minecraft:dimension_type"
        && entry == "minecraft:overworld";
    if !send_full_registry_data && !override_overworld {
        return Ok(None);
    }

    let payload = payload.ok_or_else(|| ConnectionError::MissingRegistryPayload {
        registry: registry.to_string(),
        entry: entry.to_string(),
    })?;
    if override_overworld {
        override_overworld_dimension_geometry(payload, chunk_geometry).map(Some)
    } else {
        Ok(Some(payload))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfigurationCustomPayload {
    pub(crate) channel: String,
    pub(crate) payload: Bytes,
}

pub(crate) struct ConfigurationContext<'a> {
    pub(crate) data: &'a VanillaData,
    pub(crate) tags: &'a TagsData,
    pub(crate) chunk_geometry: ChunkGeometry,
    pub(crate) custom_payload_policy: Option<&'a CustomPayloadPolicy>,
}

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
    context: ConfigurationContext<'_>,
) -> Result<Vec<ConfigurationCustomPayload>, ConnectionError>
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

    let mut custom_payloads = Vec::new();

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
            handle_configuration_custom_payload(
                frame.body,
                "before_known_packs",
                context.custom_payload_policy,
                &mut custom_payloads,
            )?;
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

    // A matching pack lets the client use its built-in definitions. Otherwise
    // every definition must come from the pre-encoded sidecar index.
    let client_has_our_pack = client_packs.iter().any(|p| {
        p.namespace == our_pack.namespace && p.id == our_pack.id && p.version == our_pack.version
    });
    let advertised_pack = format!(
        "{}:{}:{}",
        our_pack.namespace, our_pack.id, our_pack.version
    );
    let send_full_registry_data = !client_has_our_pack;
    if send_full_registry_data && !context.data.has_full_registry_payloads() {
        warn!(
            player = %profile.name,
            advertised = %advertised_pack,
            client_packs = ?client_packs,
            "client did not acknowledge our core pack and no full sidecar registry payloads exist"
        );
        return Err(ConnectionError::MissingKnownPack {
            advertised: advertised_pack,
        });
    }
    if send_full_registry_data {
        info!(
            player = %profile.name,
            advertised = %advertised_pack,
            "client did not acknowledge our core pack; sending full Registry Data"
        );
    }

    // Step 3: send every registry in the same ordering used by tags and play
    // packets. Full payloads are cheap Arc clones of startup-built bytes.
    for registry in context.data.registries() {
        let entries = registry
            .entries
            .iter()
            .map(|name| {
                let nbt_payload = prepare_registry_entry_payload(
                    context.data.registry_entry_payload(&registry.id, name),
                    registry.id.as_str(),
                    name.as_str(),
                    send_full_registry_data,
                    context.chunk_geometry,
                )?;
                Ok(RegistryEntry {
                    name: name.clone(),
                    nbt_payload,
                })
            })
            .collect::<Result<Vec<_>, ConnectionError>>()?;
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
        registries = context.data.registry_count(),
        entries = context.data.entry_count(),
        full_payloads = send_full_registry_data,
        "sent Registry Data"
    );

    // Step 3.5: ship the tag set. Mojang's built-in datapack contains
    // enchantment definitions etc. that reference `#minecraft:item`,
    // `#minecraft:entity_type` and `#minecraft:block` tags; without
    // this packet the client kicks itself on `FinishConfiguration`
    // with "Unbound tags" because nothing populated those references.
    let tag_packet = UpdateTags {
        registries: context
            .tags
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
    let tag_count = context.tags.total_tags();
    let tag_entries = context.tags.total_entries();
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
            handle_configuration_custom_payload(
                frame.body,
                "before_finish_ack",
                context.custom_payload_policy,
                &mut custom_payloads,
            )?;
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

    Ok(custom_payloads)
}

fn handle_configuration_custom_payload(
    mut body: Bytes,
    context: &'static str,
    custom_payload_policy: Option<&CustomPayloadPolicy>,
    custom_payloads: &mut Vec<ConfigurationCustomPayload>,
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
    if let Some(policy) = custom_payload_policy {
        if !policy.allows_channel(channel.as_str()) {
            debug!(
                channel = %channel.as_str(),
                len = payload_len,
                context,
                "Configuration custom payload denied by extension policy"
            );
            return Ok(());
        }
        if payload_len > policy.max_payload_bytes() {
            warn!(
                channel = %channel.as_str(),
                len = payload_len,
                max = policy.max_payload_bytes(),
                context,
                "Configuration custom payload denied by extension size policy"
            );
            return Ok(());
        }
        custom_payloads.push(ConfigurationCustomPayload {
            channel: channel.as_str().to_string(),
            payload: body.copy_to_bytes(payload_len),
        });
        debug!(
            channel = %channel.as_str(),
            len = payload_len,
            context,
            "Configuration custom payload retained for extension"
        );
        return Ok(());
    }

    debug!(
        channel = %channel.as_str(),
        len = payload_len,
        context,
        "Configuration custom payload ignored"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dimension_payload(min_y: i32, height: i32, logical_height: i32) -> Arc<[u8]> {
        let root = Tag::Compound(vec![
            ("min_y".into(), Tag::Int(min_y)),
            ("height".into(), Tag::Int(height)),
            ("logical_height".into(), Tag::Int(logical_height)),
            ("has_skylight".into(), Tag::Byte(1)),
        ]);
        let mut payload = Vec::new();
        mc_nbt::write_network(&mut payload, &root).unwrap();
        payload.into()
    }

    fn int_field(tag: &Tag, name: &str) -> Option<i32> {
        let Tag::Compound(fields) = tag else {
            return None;
        };
        fields.iter().find_map(|(field, value)| {
            (field == name)
                .then_some(value)
                .and_then(|value| match value {
                    Tag::Int(value) => Some(*value),
                    _ => None,
                })
        })
    }

    #[test]
    fn custom_geometry_overrides_decoded_overworld_dimension_fields() {
        let vanilla = dimension_payload(-64, 384, 384);
        let geometry = ChunkGeometry::new(0, 256).unwrap();

        let patched = prepare_registry_entry_payload(
            Some(vanilla),
            "minecraft:dimension_type",
            "minecraft:overworld",
            false,
            geometry,
        )
        .unwrap()
        .expect("custom overworld must carry data even for a known pack");
        let mut bytes = patched.as_ref();
        let decoded = mc_nbt::read_network(&mut bytes).unwrap();

        assert_eq!(int_field(&decoded, "min_y"), Some(0));
        assert_eq!(int_field(&decoded, "height"), Some(256));
        assert_eq!(int_field(&decoded, "logical_height"), Some(256));
        assert!(bytes.is_empty());
    }

    #[test]
    fn vanilla_geometry_preserves_captured_payload_bytes() {
        let vanilla = dimension_payload(-64, 384, 384);

        let unchanged =
            override_overworld_dimension_geometry(Arc::clone(&vanilla), OVERWORLD_GEOMETRY)
                .unwrap();

        assert!(Arc::ptr_eq(&unchanged, &vanilla));
    }

    #[test]
    fn custom_geometry_without_captured_overworld_payload_fails_closed() {
        let error = prepare_registry_entry_payload(
            None,
            "minecraft:dimension_type",
            "minecraft:overworld",
            false,
            ChunkGeometry::new(0, 256).unwrap(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ConnectionError::MissingRegistryPayload { registry, entry }
                if registry == "minecraft:dimension_type" && entry == "minecraft:overworld"
        ));
    }
}
