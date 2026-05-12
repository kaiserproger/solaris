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

use bytes::BytesMut;
use mc_data::VanillaData;
use mc_protocol::TARGET_RELEASE;
use mc_protocol::frame::Compression;
use mc_protocol::packets::Packet;
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundKnownPacks, FinishConfiguration, KnownPackEntry,
    RegistryData, RegistryEntry, ServerboundKnownPacks,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, info, warn};

use crate::connection::{read_frame, write_packet};
use crate::error::ConnectionError;
use crate::login::LoggedInProfile;

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
    profile: &LoggedInProfile,
    data: &VanillaData,
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
        Compression::Disabled,
    )
    .await?;

    // Step 2: read frames until we see the client's KnownPacks response.
    let client_packs = loop {
        let frame = read_frame(reader, buf, Compression::Disabled).await?;
        if frame.id == ServerboundKnownPacks::ID {
            let mut body = frame.body;
            let parsed = ServerboundKnownPacks::decode(&mut body)?;
            break parsed.packs;
        }
        debug!(
            id = format!("{:#04x}", frame.id),
            len = frame.body.len(),
            "ignored unsolicited Configuration packet"
        );
    };

    // If the client did not echo back our pack the `has_data = false`
    // shortcut is unsound — they have no built-in data to fall back on
    // and Solaris would otherwise be silently sending empty registries.
    // We still proceed (so the connection at least gets through
    // Configuration cleanly) but log a loud warning so the operator
    // knows the resulting Play session will probably misbehave.
    let client_has_our_pack = client_packs.iter().any(|p| {
        p.namespace == our_pack.namespace && p.id == our_pack.id && p.version == our_pack.version
    });
    if !client_has_our_pack {
        warn!(
            player = %profile.name,
            advertised = %format!("{}:{}:{}", our_pack.namespace, our_pack.id, our_pack.version),
            client_packs = ?client_packs,
            "client did not acknowledge our core pack; subsequent registries \
             will reference built-in data the client does not have"
        );
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
            Compression::Disabled,
        )
        .await?;
    }
    debug!(
        registries = data.registry_count(),
        entries = data.entry_count(),
        "sent Registry Data"
    );

    // Step 4: tell the client we are done configuring.
    write_packet(writer, &FinishConfiguration, Compression::Disabled).await?;

    // Step 5: wait for AcknowledgeFinishConfiguration, ignoring
    // anything else.
    loop {
        let frame = read_frame(reader, buf, Compression::Disabled).await?;
        if frame.id == AcknowledgeFinishConfiguration::ID {
            break;
        }
        debug!(
            id = format!("{:#04x}", frame.id),
            "ignored Configuration packet while waiting for ack"
        );
    }

    info!(
        player = %profile.name,
        client_pack_count = client_packs.len(),
        "configuration complete; Play state not yet implemented (M1.g)"
    );

    Ok(())
}
