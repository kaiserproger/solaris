//! Configuration state handler.
//!
//! Wire choreography:
//!
//! ```text
//! S → C  Update Enabled Features   (`minecraft:vanilla`)
//! S → C  Clientbound Known Packs   (advertise `minecraft:core:<version>`)
//! C → S  Serverbound Known Packs   (the subset the client also has)
//! S → C  Registry Data × N         (one per built-in registry; entries use
//!                                   the matched client pack or full sidecar
//!                                   Network-NBT payloads)
//! S → C  Loader Manifest           (only when plugins declare client bundles)
//! C → S  Loader Request × N        (one per missing exact cache identity)
//! S → C  Loader Artifact × N       (bounded contiguous chunks)
//! C → S  Loader Ack                (only after verified cache publication)
//! S → C  Finish Configuration
//! C → S  Acknowledge Finish Configuration
//!        → state transitions to Play
//! ```
//!
//! Inbound configuration packets that aren't `Serverbound Known Packs`
//! or `Acknowledge Finish Configuration` are read and handled without
//! blocking the handshake — robust to optional `Client Information` and
//! `Plugin Message` traffic the client may emit at any point.

#[cfg(test)]
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

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
    AcknowledgeFinishConfiguration, ClientboundCustomPayload, ClientboundKnownPacks,
    ConfigurationDisconnect, FinishConfiguration, KnownPackEntry, RegistryData, RegistryEntry,
    ServerboundClientInformation, ServerboundCustomPayload, ServerboundKnownPacks,
    ServerboundResourcePack, UpdateEnabledFeatures, UpdateTags, UpdateTagsEntry,
    UpdateTagsRegistry,
};
use mc_protocol::{CodecError, State, TARGET_RELEASE};
use mc_world::{ChunkGeometry, OVERWORLD_GEOMETRY};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, info, warn};

use crate::connection::{PRE_PLAY_READ_TIMEOUT, read_frame_with_timeout, write_packet};
use crate::error::ConnectionError;
use crate::loader::{
    LOADER_ARTIFACT_CHUNK_BYTES, LoaderArtifactRequest, LoaderClientAck, LoaderManifest,
    LoaderSession, encode_artifact_chunk, loader_ack_channel, loader_artifact_channel,
    loader_manifest_channel, loader_request_channel,
};
use crate::login::LoggedInProfile;

const MAX_IGNORED_CONFIGURATION_PACKETS: usize = 32;
const MAX_LOADER_DISCONNECT_BUNDLES: usize = 8;
const LOADER_HANDSHAKE_READ_TIMEOUT: Duration = Duration::from_secs(120);

fn decode_configuration_exact<P: Packet>(id: i32, mut body: Bytes) -> Result<P, ConnectionError> {
    let packet = P::decode(&mut body)?;
    let trailing = body.remaining();
    if trailing != 0 {
        return Err(ConnectionError::TrailingBytes {
            state: State::Configuration,
            id,
            trailing,
        });
    }
    Ok(packet)
}

fn expect_empty_configuration_body(id: i32, body: &Bytes) -> Result<(), ConnectionError> {
    if body.is_empty() {
        return Ok(());
    }
    Err(ConnectionError::TrailingBytes {
        state: State::Configuration,
        id,
        trailing: body.len(),
    })
}

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
    pub(crate) loader_manifest: Option<&'a LoaderManifest>,
}

pub(crate) struct ConfigurationOutcome {
    pub(crate) custom_payloads: Vec<ConfigurationCustomPayload>,
    pub(crate) loader_session: Option<LoaderSession>,
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
) -> Result<ConfigurationOutcome, ConnectionError>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    debug!(player = %profile.name, uuid = %profile.uuid, "entering Configuration state");
    // Vanilla publishes feature flags before registry/known-pack negotiation.
    write_packet(
        writer,
        &UpdateEnabledFeatures {
            features: vec![
                mc_data::Identifier::parse("minecraft:vanilla")
                    .expect("static feature id is valid"),
            ],
        },
        compression,
    )
    .await?;

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
    let mut loader_acknowledged = context.loader_manifest.is_none();
    let mut loader_session = None;
    let mut loader_requests = BTreeSet::new();

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
            let parsed = decode_configuration_exact::<ServerboundKnownPacks>(frame.id, frame.body)?;
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
            let information =
                decode_configuration_exact::<ServerboundClientInformation>(frame.id, frame.body)?
                    .information;
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
            let _ = handle_configuration_custom_payload(
                frame.body,
                "before_known_packs",
                context.custom_payload_policy,
                &mut custom_payloads,
                None,
                &mut loader_acknowledged,
                &mut loader_session,
                &mut loader_requests,
            )?;
            continue;
        }
        if frame.id == ServerboundResourcePack::ID {
            let status =
                decode_configuration_exact::<ServerboundResourcePack>(frame.id, frame.body)?.status;
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

    if let Some(manifest) = context.loader_manifest {
        let payload = manifest
            .encode()
            .map_err(|error| ConnectionError::LoaderHandshake {
                reason: error.to_string(),
            })?;
        write_packet(
            writer,
            &ClientboundCustomPayload {
                payload: CustomPayload::Unknown {
                    channel: loader_manifest_channel().clone(),
                    payload,
                },
            },
            compression,
        )
        .await?;
        debug!(
            bundles = manifest.bundles.len(),
            protocol = manifest.protocol,
            "sent Solaris Loader manifest"
        );
        if let Err(error) = complete_loader_handshake(
            reader,
            writer,
            buf,
            compression,
            context.custom_payload_policy,
            &mut custom_payloads,
            manifest,
            &mut loader_acknowledged,
            &mut loader_session,
            &mut loader_requests,
        )
        .await
        {
            send_loader_disconnect(writer, compression, manifest).await;
            return Err(error);
        }
    }

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
            expect_empty_configuration_body(frame.id, &frame.body)?;
            if !loader_acknowledged {
                return Err(ConnectionError::LoaderHandshake {
                    reason: "client finished Configuration without acknowledging the Solaris Loader manifest"
                        .to_owned(),
                });
            }
            break;
        }
        if frame.id == ServerboundClientInformation::ID {
            let information =
                decode_configuration_exact::<ServerboundClientInformation>(frame.id, frame.body)?
                    .information;
            debug!(
                language = %information.language,
                requested_view_distance = information.view_distance,
                "client information noted while waiting for Configuration ack"
            );
            note_ignored_configuration_packet(&mut ignored_packets)?;
            continue;
        }
        if frame.id == ServerboundCustomPayload::ID {
            let request = handle_configuration_custom_payload(
                frame.body,
                "before_finish_ack",
                context.custom_payload_policy,
                &mut custom_payloads,
                None,
                &mut loader_acknowledged,
                &mut loader_session,
                &mut loader_requests,
            )?;
            if let Some(request) = request {
                send_loader_artifact(
                    writer,
                    compression,
                    context
                        .loader_manifest
                        .expect("artifact request requires a loader manifest"),
                    &request,
                )
                .await?;
            } else {
                note_ignored_configuration_packet(&mut ignored_packets)?;
            }
            continue;
        }
        if frame.id == ServerboundResourcePack::ID {
            let status =
                decode_configuration_exact::<ServerboundResourcePack>(frame.id, frame.body)?.status;
            debug!(
                id = %status.id,
                action = ?status.action,
                terminal = status.action.is_terminal(),
                "resource-pack status noted while waiting for Configuration ack"
            );
            note_ignored_configuration_packet(&mut ignored_packets)?;
            continue;
        }
        note_ignored_configuration_packet(&mut ignored_packets)?;
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

    Ok(ConfigurationOutcome {
        custom_payloads,
        loader_session,
    })
}

async fn send_loader_disconnect<W>(
    writer: &mut W,
    compression: Compression,
    manifest: &LoaderManifest,
) where
    W: AsyncWriteExt + Unpin,
{
    let reason = loader_disconnect_reason(manifest);
    let reason_nbt = match text_component_nbt(&reason) {
        Ok(reason_nbt) => reason_nbt,
        Err(error) => {
            warn!(?error, "failed to encode Solaris Loader disconnect reason");
            return;
        }
    };
    if let Err(error) =
        write_packet(writer, &ConfigurationDisconnect { reason_nbt }, compression).await
    {
        debug!(
            ?error,
            "failed to send Solaris Loader Configuration disconnect"
        );
    }
}

fn loader_disconnect_reason(manifest: &LoaderManifest) -> String {
    let loaders = manifest
        .bundles
        .iter()
        .flat_map(|bundle| bundle.loaders.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|loader| match loader {
            crate::LoaderPlatform::Fabric => "Fabric",
            crate::LoaderPlatform::NeoForge => "NeoForge",
            crate::LoaderPlatform::Forge => "Forge",
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mut bundle_identities = manifest
        .bundles
        .iter()
        .take(MAX_LOADER_DISCONNECT_BUNDLES)
        .map(|bundle| format!("{}:{}@{}", bundle.owner, bundle.id, bundle.version))
        .collect::<Vec<_>>();
    if manifest.bundles.len() > MAX_LOADER_DISCONNECT_BUNDLES {
        bundle_identities.push(format!(
            "and {} more",
            manifest.bundles.len() - MAX_LOADER_DISCONNECT_BUNDLES
        ));
    }
    let bundles = bundle_identities.join(", ");
    format!(
        "This server requires Solaris Loader. Supported loaders: {loaders}. Required bundles: {bundles}. Install Solaris Loader and reconnect."
    )
}

fn text_component_nbt(text: &str) -> Result<Vec<u8>, CodecError> {
    let mut out = Vec::new();
    mc_nbt::write_network(
        &mut out,
        &Tag::Compound(vec![("text".to_owned(), Tag::String(text.to_owned()))]),
    )?;
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
async fn complete_loader_handshake<R, W>(
    reader: &mut R,
    writer: &mut W,
    buf: &mut BytesMut,
    compression: Compression,
    custom_payload_policy: Option<&CustomPayloadPolicy>,
    custom_payloads: &mut Vec<ConfigurationCustomPayload>,
    manifest: &LoaderManifest,
    loader_acknowledged: &mut bool,
    loader_session: &mut Option<LoaderSession>,
    loader_requests: &mut BTreeSet<String>,
) -> Result<(), ConnectionError>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let mut ignored_packets = 0usize;
    while !*loader_acknowledged {
        let frame = read_frame_with_timeout(
            reader,
            buf,
            compression,
            State::Configuration,
            LOADER_HANDSHAKE_READ_TIMEOUT,
        )
        .await?;
        if frame.id == ServerboundClientInformation::ID {
            let information =
                decode_configuration_exact::<ServerboundClientInformation>(frame.id, frame.body)?
                    .information;
            debug!(
                language = %information.language,
                requested_view_distance = information.view_distance,
                "client information noted while waiting for Solaris Loader"
            );
            note_ignored_configuration_packet(&mut ignored_packets)?;
            continue;
        }
        if frame.id == ServerboundCustomPayload::ID {
            let request = handle_configuration_custom_payload(
                frame.body,
                "before_loader_ack",
                custom_payload_policy,
                custom_payloads,
                Some(manifest),
                loader_acknowledged,
                loader_session,
                loader_requests,
            )?;
            if let Some(request) = request {
                send_loader_artifact(writer, compression, manifest, &request).await?;
            } else if !*loader_acknowledged {
                note_ignored_configuration_packet(&mut ignored_packets)?;
            }
            continue;
        }
        if frame.id == ServerboundResourcePack::ID {
            let status =
                decode_configuration_exact::<ServerboundResourcePack>(frame.id, frame.body)?.status;
            debug!(
                id = %status.id,
                action = ?status.action,
                terminal = status.action.is_terminal(),
                "resource-pack status noted while waiting for Solaris Loader"
            );
            note_ignored_configuration_packet(&mut ignored_packets)?;
            continue;
        }
        note_ignored_configuration_packet(&mut ignored_packets)?;
        debug!(
            id = format!("{:#04x}", frame.id),
            "ignored Configuration packet while waiting for Solaris Loader"
        );
    }
    Ok(())
}

fn note_ignored_configuration_packet(count: &mut usize) -> Result<(), ConnectionError> {
    *count += 1;
    if *count > MAX_IGNORED_CONFIGURATION_PACKETS {
        return Err(ConnectionError::IgnoredPacketBudgetExceeded {
            state: State::Configuration,
            max: MAX_IGNORED_CONFIGURATION_PACKETS,
        });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_configuration_custom_payload(
    mut body: Bytes,
    context: &'static str,
    custom_payload_policy: Option<&CustomPayloadPolicy>,
    custom_payloads: &mut Vec<ConfigurationCustomPayload>,
    loader_manifest: Option<&LoaderManifest>,
    loader_acknowledged: &mut bool,
    loader_session: &mut Option<LoaderSession>,
    loader_requests: &mut BTreeSet<String>,
) -> Result<Option<LoaderArtifactRequest>, ConnectionError> {
    if body.len() > DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES {
        warn!(
            len = body.len(),
            max = DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES,
            context,
            "oversized Configuration custom payload rejected before decode"
        );
        return Ok(None);
    }

    let channel = body.read_identifier()?;
    if channel == *CustomPayload::brand_channel() {
        let brand = body.read_string(DEFAULT_MAX_STRING_LEN)?;
        expect_empty_configuration_body(ServerboundCustomPayload::ID, &body)?;
        debug!(brand = %brand, context, "client brand noted during Configuration");
        return Ok(None);
    }
    if channel == *loader_request_channel() {
        let Some(manifest) = loader_manifest else {
            debug!(
                context,
                "ignored unsolicited Solaris Loader artifact request"
            );
            return Ok(None);
        };
        let request = LoaderArtifactRequest::decode(&body).map_err(|error| {
            ConnectionError::LoaderHandshake {
                reason: error.to_string(),
            }
        })?;
        manifest.requested_artifact(&request).map_err(|error| {
            ConnectionError::LoaderHandshake {
                reason: error.to_string(),
            }
        })?;
        if !loader_requests.insert(request.cache_key.clone()) {
            return Err(ConnectionError::LoaderHandshake {
                reason: format!(
                    "client requested loader cache identity {} more than once",
                    request.cache_key
                ),
            });
        }
        return Ok(Some(request));
    }
    if channel == *loader_ack_channel() {
        let Some(manifest) = loader_manifest else {
            debug!(
                context,
                "ignored unsolicited Solaris Loader acknowledgement"
            );
            return Ok(None);
        };
        let ack =
            LoaderClientAck::decode(&body).map_err(|error| ConnectionError::LoaderHandshake {
                reason: error.to_string(),
            })?;
        let session =
            manifest
                .bind_ack(&ack)
                .map_err(|error| ConnectionError::LoaderHandshake {
                    reason: error.to_string(),
                })?;
        *loader_session = Some(session);
        *loader_acknowledged = true;
        debug!(
            platform = ?ack.platform,
            loader_version = %ack.loader_version,
            "Solaris Loader manifest acknowledged"
        );
        return Ok(None);
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
            return Ok(None);
        }
        if payload_len > policy.max_payload_bytes() {
            warn!(
                channel = %channel.as_str(),
                len = payload_len,
                max = policy.max_payload_bytes(),
                context,
                "Configuration custom payload denied by extension size policy"
            );
            return Ok(None);
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
        return Ok(None);
    }

    debug!(
        channel = %channel.as_str(),
        len = payload_len,
        context,
        "Configuration custom payload ignored"
    );

    Ok(None)
}

async fn send_loader_artifact<W>(
    writer: &mut W,
    compression: Compression,
    manifest: &LoaderManifest,
    request: &LoaderArtifactRequest,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let (bundle, path) =
        manifest
            .requested_artifact(request)
            .map_err(|error| ConnectionError::LoaderHandshake {
                reason: error.to_string(),
            })?;
    let metadata = tokio::fs::metadata(path).await?;
    if !metadata.is_file() || metadata.len() != bundle.size_bytes {
        return Err(ConnectionError::LoaderHandshake {
            reason: format!(
                "server artifact {} no longer matches declared size {}",
                path.display(),
                bundle.size_bytes
            ),
        });
    }
    let mut file = tokio::fs::File::open(path).await?;
    let mut offset = 0_u64;
    while offset < bundle.size_bytes {
        let remaining = bundle.size_bytes - offset;
        let chunk_len = usize::try_from(remaining.min(LOADER_ARTIFACT_CHUNK_BYTES as u64))
            .expect("loader chunk length fits usize");
        let mut chunk = vec![0_u8; chunk_len];
        file.read_exact(&mut chunk).await?;
        let last = offset + chunk_len as u64 == bundle.size_bytes;
        let payload =
            encode_artifact_chunk(&bundle.cache_key, offset, last, &chunk).map_err(|error| {
                ConnectionError::LoaderHandshake {
                    reason: error.to_string(),
                }
            })?;
        write_packet(
            writer,
            &ClientboundCustomPayload {
                payload: CustomPayload::Unknown {
                    channel: loader_artifact_channel().clone(),
                    payload,
                },
            },
            compression,
        )
        .await?;
        offset += chunk_len as u64;
    }
    let mut trailing = [0_u8; 1];
    if file.read(&mut trailing).await? != 0 {
        return Err(ConnectionError::LoaderHandshake {
            reason: format!(
                "server artifact {} grew while it was transferred",
                path.display()
            ),
        });
    }
    debug!(
        cache_key = %bundle.cache_key,
        bytes = offset,
        "sent Solaris Loader artifact"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        LOADER_PROTOCOL_VERSION, LoaderBundle, LoaderContentKind, LoaderPermission, LoaderPlatform,
    };
    use mc_protocol::packets::{
        ChatVisibility, ClientInformation, MainHand, ParticleStatus, ResourcePackAction,
        ResourcePackStatus,
    };
    use uuid::Uuid;

    fn encoded_with_trailing<P: Packet>(packet: &P) -> Bytes {
        let mut body = Vec::new();
        packet.encode(&mut body).unwrap();
        body.push(0xff);
        Bytes::from(body)
    }

    #[test]
    fn exact_configuration_decoder_rejects_known_packet_trailing_bytes() {
        let known_packs = ServerboundKnownPacks { packs: Vec::new() };
        assert!(matches!(
            decode_configuration_exact::<ServerboundKnownPacks>(
                ServerboundKnownPacks::ID,
                encoded_with_trailing(&known_packs),
            ),
            Err(ConnectionError::TrailingBytes { trailing: 1, .. })
        ));

        let client_information = ServerboundClientInformation {
            information: ClientInformation {
                language: "en_us".to_owned(),
                view_distance: 8,
                chat_visibility: ChatVisibility::Full,
                chat_colors: true,
                model_customisation: 0,
                main_hand: MainHand::Right,
                text_filtering_enabled: false,
                allows_listing: true,
                particle_status: ParticleStatus::All,
            },
        };
        assert!(matches!(
            decode_configuration_exact::<ServerboundClientInformation>(
                ServerboundClientInformation::ID,
                encoded_with_trailing(&client_information),
            ),
            Err(ConnectionError::TrailingBytes { trailing: 1, .. })
        ));

        let resource_pack = ServerboundResourcePack {
            status: ResourcePackStatus {
                id: Uuid::nil(),
                action: ResourcePackAction::Accepted,
            },
        };
        assert!(matches!(
            decode_configuration_exact::<ServerboundResourcePack>(
                ServerboundResourcePack::ID,
                encoded_with_trailing(&resource_pack),
            ),
            Err(ConnectionError::TrailingBytes { trailing: 1, .. })
        ));
    }

    #[test]
    fn finish_ack_requires_an_empty_body() {
        assert!(
            expect_empty_configuration_body(AcknowledgeFinishConfiguration::ID, &Bytes::new(),)
                .is_ok()
        );
        assert!(matches!(
            expect_empty_configuration_body(
                AcknowledgeFinishConfiguration::ID,
                &Bytes::from_static(&[0]),
            ),
            Err(ConnectionError::TrailingBytes { trailing: 1, .. })
        ));
    }

    #[test]
    fn brand_payload_rejects_trailing_bytes() {
        let mut body = Vec::new();
        CustomPayload::Brand("vanilla".to_owned())
            .encode_serverbound(&mut body)
            .unwrap();
        body.push(0xff);

        assert!(matches!(
            handle_configuration_custom_payload(
                Bytes::from(body),
                "test",
                None,
                &mut Vec::new(),
                None,
                &mut false,
                &mut None,
                &mut BTreeSet::new(),
            ),
            Err(ConnectionError::TrailingBytes { trailing: 1, .. })
        ));
    }

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

    fn loader_manifest() -> LoaderManifest {
        LoaderManifest {
            protocol: LOADER_PROTOCOL_VERSION,
            bundles: vec![LoaderBundle {
                owner: "example".to_owned(),
                id: "screen".to_owned(),
                version: "1".to_owned(),
                artifact: "client/screen.zip".to_owned(),
                sha256: "a".repeat(64),
                size_bytes: 128,
                loaders: vec![
                    LoaderPlatform::Fabric,
                    LoaderPlatform::NeoForge,
                    LoaderPlatform::Forge,
                ],
                content: vec![LoaderContentKind::Screens],
                permissions: vec![LoaderPermission::OpenScreens],
                cache_key: format!("example:screen/1/{}", "a".repeat(64)),
                source_path: None,
                block_id: None,
                block_name: None,
            }],
        }
    }

    fn loader_ack_body(ack: &LoaderClientAck) -> Bytes {
        let mut body = Vec::new();
        CustomPayload::Unknown {
            channel: loader_ack_channel().clone(),
            payload: serde_json::to_vec(ack).unwrap(),
        }
        .encode_serverbound(&mut body)
        .unwrap();
        Bytes::from(body)
    }

    fn loader_request_body(request: &LoaderArtifactRequest) -> Bytes {
        let mut body = Vec::new();
        CustomPayload::Unknown {
            channel: loader_request_channel().clone(),
            payload: serde_json::to_vec(request).unwrap(),
        }
        .encode_serverbound(&mut body)
        .unwrap();
        Bytes::from(body)
    }

    #[test]
    fn configuration_loader_ack_validates_before_extension_routing() {
        let manifest = loader_manifest();
        let ack = LoaderClientAck {
            protocol: LOADER_PROTOCOL_VERSION,
            platform: LoaderPlatform::Fabric,
            loader_version: "0.1.0".to_owned(),
            accepted_permissions: vec![LoaderPermission::OpenScreens],
            cached_bundles: vec![manifest.bundles[0].cache_key.clone()],
            carrier_block_state_ids: BTreeMap::new(),
        };
        let mut acknowledged = false;
        let mut loader_session = None;
        let mut retained = Vec::new();

        handle_configuration_custom_payload(
            loader_ack_body(&ack),
            "test",
            None,
            &mut retained,
            Some(&manifest),
            &mut acknowledged,
            &mut loader_session,
            &mut BTreeSet::new(),
        )
        .unwrap();

        assert!(acknowledged);
        assert_eq!(
            loader_session.as_ref().map(LoaderSession::platform),
            Some(LoaderPlatform::Fabric)
        );
        assert!(retained.is_empty());
    }

    #[test]
    fn configuration_loader_ack_rejects_missing_cache_identity() {
        let manifest = loader_manifest();
        let ack = LoaderClientAck {
            protocol: LOADER_PROTOCOL_VERSION,
            platform: LoaderPlatform::Forge,
            loader_version: "0.1.0".to_owned(),
            accepted_permissions: vec![LoaderPermission::OpenScreens],
            cached_bundles: Vec::new(),
            carrier_block_state_ids: BTreeMap::new(),
        };
        let mut acknowledged = false;

        let error = handle_configuration_custom_payload(
            loader_ack_body(&ack),
            "test",
            None,
            &mut Vec::new(),
            Some(&manifest),
            &mut acknowledged,
            &mut None,
            &mut BTreeSet::new(),
        )
        .unwrap_err();

        assert!(matches!(error, ConnectionError::LoaderHandshake { .. }));
        assert!(!acknowledged);
    }

    #[test]
    fn configuration_loader_request_resolves_exact_manifest_artifact() {
        let mut manifest = loader_manifest();
        manifest.bundles[0].source_path = Some("/plugin/client/screen.zip".into());
        let request = LoaderArtifactRequest {
            protocol: LOADER_PROTOCOL_VERSION,
            cache_key: manifest.bundles[0].cache_key.clone(),
        };
        let mut requests = BTreeSet::new();

        let decoded = handle_configuration_custom_payload(
            loader_request_body(&request),
            "test",
            None,
            &mut Vec::new(),
            Some(&manifest),
            &mut false,
            &mut None,
            &mut requests,
        )
        .unwrap()
        .unwrap();

        assert_eq!(decoded, request);
        assert!(matches!(
            handle_configuration_custom_payload(
                loader_request_body(&request),
                "test",
                None,
                &mut Vec::new(),
                Some(&manifest),
                &mut false,
                &mut None,
                &mut requests,
            ),
            Err(ConnectionError::LoaderHandshake { .. })
        ));
    }

    #[test]
    fn loader_disconnect_reason_names_supported_loaders_and_required_bundle() {
        let manifest = loader_manifest();

        assert_eq!(
            loader_disconnect_reason(&manifest),
            "This server requires Solaris Loader. Supported loaders: Fabric, NeoForge, Forge. Required bundles: example:screen@1. Install Solaris Loader and reconnect."
        );
    }

    #[tokio::test]
    async fn loader_handshake_failure_sends_configuration_disconnect() {
        let manifest = loader_manifest();
        let (mut server, mut client) = tokio::io::duplex(4_096);

        send_loader_disconnect(&mut server, Compression::Disabled, &manifest).await;
        let mut buf = BytesMut::new();
        let frame = read_frame_with_timeout(
            &mut client,
            &mut buf,
            Compression::Disabled,
            State::Configuration,
            Duration::from_secs(1),
        )
        .await
        .unwrap();

        assert_eq!(frame.id, ConfigurationDisconnect::ID);
        let mut body = frame.body;
        let disconnect = ConfigurationDisconnect::decode(&mut body).unwrap();
        let mut reason = disconnect.reason_nbt.as_slice();
        let decoded = mc_nbt::read_network(&mut reason).unwrap();
        assert_eq!(
            decoded,
            Tag::Compound(vec![(
                "text".to_owned(),
                Tag::String(loader_disconnect_reason(&manifest))
            )])
        );
        assert!(reason.is_empty());
    }
}
