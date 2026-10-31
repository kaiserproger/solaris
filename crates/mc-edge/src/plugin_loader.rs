//! Shared Loader protocol transport for v2 configuration sessions.

use std::collections::BTreeSet;

use mc_loader::{LoaderArtifactRequest, LoaderClientAck, LoaderManifest, LoaderSession};
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundCustomPayload, ConfigurationDisconnect,
    ServerboundCustomPayload,
};
use mc_protocol::packets::{CustomPayload, Packet};

use crate::connection::{Connection, text_component_nbt};

/// Declare the serverbound Loader channels with `minecraft:register`. NeoForge
/// clients send a custom payload only on a channel the server declared; vanilla
/// clients ignore the payload.
pub(crate) async fn send_channel_register(conn: &mut Connection) -> anyhow::Result<()> {
    let channels = [
        mc_loader::loader_hello_channel(),
        mc_loader::loader_request_channel(),
        mc_loader::loader_ack_channel(),
        mc_loader::loader_view_action_channel(),
    ];
    let payload = channels
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\0")
        .into_bytes();
    conn.send_packet(&ClientboundCustomPayload {
        payload: CustomPayload::Unknown {
            channel: mc_data::Identifier::parse("minecraft:register")?,
            payload,
        },
    })
    .await?;
    Ok(())
}

/// Ask whether the client speaks the Loader. Vanilla clients ignore the unknown
/// configuration payload; a Loader client answers `solaris:loader/hello`.
pub(crate) async fn send_probe(conn: &mut Connection) -> anyhow::Result<()> {
    conn.send_packet(&ClientboundCustomPayload {
        payload: CustomPayload::Unknown {
            channel: mc_loader::loader_probe_channel().clone(),
            payload: Vec::new(),
        },
    })
    .await?;
    Ok(())
}

/// Whether a client configuration frame is the empty Loader hello.
pub(crate) fn is_hello(id: i32, body: &bytes::Bytes) -> anyhow::Result<bool> {
    if id != ServerboundCustomPayload::ID {
        return Ok(false);
    }
    match CustomPayload::decode_serverbound(&mut body.clone())? {
        CustomPayload::Unknown { channel, payload }
            if channel == *mc_loader::loader_hello_channel() =>
        {
            anyhow::ensure!(payload.is_empty(), "Loader hello carries no payload");
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub(crate) async fn negotiate(
    conn: &mut Connection,
    manifest: &LoaderManifest,
    prefs: &mut crate::configuration::ClientPrefs,
) -> anyhow::Result<LoaderSession> {
    let result = negotiate_inner(conn, manifest, prefs).await;
    if let Err(error) = &result {
        let reason = format!(
            "This server requires the Solaris Loader client mod. Install it and reconnect. \
             (Loader negotiation failed: {error})"
        );
        if let Ok(reason_nbt) = text_component_nbt(&reason) {
            let _ = conn
                .send_packet(&ConfigurationDisconnect { reason_nbt })
                .await;
        }
    }
    result
}

async fn negotiate_inner(
    conn: &mut Connection,
    manifest: &LoaderManifest,
    prefs: &mut crate::configuration::ClientPrefs,
) -> anyhow::Result<LoaderSession> {
    conn.send_packet(&ClientboundCustomPayload {
        payload: CustomPayload::Unknown {
            channel: mc_loader::loader_manifest_channel().clone(),
            payload: manifest.encode()?,
        },
    })
    .await?;
    let mut requests = BTreeSet::new();
    let mut ignored = 0usize;
    loop {
        let frame = conn
            .read_frame_in(mc_protocol::State::Configuration)
            .await?;
        anyhow::ensure!(
            frame.id != AcknowledgeFinishConfiguration::ID,
            "client finished configuration before acknowledging required Loader packages"
        );
        if frame.id == ServerboundCustomPayload::ID {
            let mut body = frame.body.clone();
            let payload = CustomPayload::decode_serverbound(&mut body)?;
            anyhow::ensure!(body.is_empty(), "trailing Loader custom payload bytes");
            if let CustomPayload::Unknown { channel, payload } = payload {
                if channel == *mc_loader::loader_ack_channel() {
                    let ack = LoaderClientAck::decode(&payload)?;
                    return Ok(manifest.bind_ack(&ack)?);
                }
                if channel == *mc_loader::loader_request_channel() {
                    let request = LoaderArtifactRequest::decode(&payload)?;
                    let (bundle, artifact) = manifest.requested_artifact(&request)?;
                    anyhow::ensure!(
                        requests.insert(request.cache_key.clone()),
                        "duplicate Loader artifact request"
                    );
                    anyhow::ensure!(
                        artifact.len() as u64 == bundle.size_bytes,
                        "Loader artifact size no longer matches manifest"
                    );
                    for (index, chunk) in artifact
                        .chunks(mc_loader::LOADER_ARTIFACT_CHUNK_BYTES)
                        .enumerate()
                    {
                        let offset = index * mc_loader::LOADER_ARTIFACT_CHUNK_BYTES;
                        let last = offset + chunk.len() == artifact.len();
                        conn.send_packet(&ClientboundCustomPayload {
                            payload: CustomPayload::Unknown {
                                channel: mc_loader::loader_artifact_channel().clone(),
                                payload: mc_loader::encode_artifact_chunk(
                                    &bundle.cache_key,
                                    offset as u64,
                                    last,
                                    chunk,
                                )?,
                            },
                        })
                        .await?;
                    }
                    continue;
                }
            }
        }
        crate::configuration::note_ignored(prefs, frame.id, frame.body)?;
        ignored += 1;
        anyhow::ensure!(ignored <= 32, "too many unrelated Loader handshake packets");
    }
}
