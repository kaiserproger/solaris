//! Configuration state handler.
//!
//! M1.e scope: drive the wire mechanics of the state cleanly. We tell
//! the client we know `minecraft:core` for the running game version, so
//! it should use its built-in registry data; we send no `Registry Data`
//! packets ourselves. A real vanilla 26.1 client may still refuse to
//! enter Play without explicit registries — proving full compatibility
//! is M1.g+ work (see `docs/milestones/M1.md`).
//!
//! Inbound configuration packets that aren't `Serverbound Known Packs`
//! or `Acknowledge Finish Configuration` are read, logged at debug
//! level, and discarded. This is robust to optional packets like
//! `Client Information` and `Plugin Message` that the client may or may
//! not send depending on its settings.

use bytes::BytesMut;
use mc_protocol::TARGET_RELEASE;
use mc_protocol::frame::Compression;
use mc_protocol::packets::Packet;
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundKnownPacks, FinishConfiguration, KnownPackEntry,
    ServerboundKnownPacks,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, info};

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
) -> Result<(), ConnectionError>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    debug!(player = %profile.name, uuid = %profile.uuid, "entering Configuration state");

    // Step 1: advertise our known data packs.
    write_packet(
        writer,
        &ClientboundKnownPacks {
            packs: vec![server_known_pack()],
        },
        Compression::Disabled,
    )
    .await?;

    // Step 2: read frames until we see the client's KnownPacks response.
    // Anything else (Client Information, plugin messages, …) is dropped
    // after a debug log.
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
    debug!(
        client_packs = client_packs.len(),
        "received Serverbound Known Packs"
    );

    // Step 3: tell the client we are done configuring.
    write_packet(writer, &FinishConfiguration, Compression::Disabled).await?;

    // Step 4: wait for AcknowledgeFinishConfiguration, again ignoring
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

    // Real implementation would now transition the dispatcher into the
    // Play state. We just return — `server.rs` will drop the connection.
    // A real vanilla client will surface a "connection lost" message,
    // which is the expected M1.e behavior.
    Ok(())
}
