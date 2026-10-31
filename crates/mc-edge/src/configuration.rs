//! Configuration state handler.
//!
//! Uses the existing vanilla configuration choreography and the shared
//! `mc-loader` manifest/artifact/ack contract when packages declare client bundles.
//! Packet IDs and layouts come only from `mc-protocol`.
//!
//! ```text
//! S → C  Update Enabled Features   (`minecraft:vanilla`)
//! S → C  Clientbound Known Packs   (advertise `minecraft:core:<version>`)
//! C → S  Serverbound Known Packs   (the subset the client also has)
//! S → C  Registry Data × N         (one per built-in registry; entries use
//!                                   the matched client pack or full sidecar
//!                                   Network-NBT payloads)
//! S → C  Update Tags               (from `mc-data` tags)
//! S → C  Brand custom payload      (AFTER registries)
//! S → C  Finish Configuration
//! C → S  Acknowledge Finish Configuration
//!        → connection transitions to Play
//! ```
//!
//! Inbound configuration packets that aren't `Serverbound Known Packs`
//! or `Acknowledge Finish Configuration` are read and tolerated without
//! blocking the handshake — robust to optional `Client Information`,
//! brand `Custom Payload`, and `Resource Pack` traffic the client may
//! emit at any point. No cookies, no SelectKnownPacks.

use bytes::Bytes;
use mc_protocol::TARGET_RELEASE;
use mc_protocol::packets::CustomPayload;
use mc_protocol::packets::Packet;
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundCustomPayload, ClientboundKnownPacks,
    ConfigurationDisconnect, FinishConfiguration, KnownPackEntry, RegistryData, RegistryEntry,
    ServerboundClientInformation, ServerboundCustomPayload, ServerboundKnownPacks,
    ServerboundResourcePack, UpdateEnabledFeatures, UpdateTags, UpdateTagsEntry,
    UpdateTagsRegistry,
};

use crate::connection::{Connection, text_component_nbt};

const MAX_IGNORED_CONFIGURATION_PACKETS: usize = 32;

/// Client-advertised preferences from the latest `Client Information` packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientPrefs {
    /// Requested view distance, unclamped (the edge clamps before admission).
    pub view_distance: i8,
    /// `ClientInformation.language`, e.g. `en_us` or `ru_ru`; empty until sent.
    pub language: String,
}

impl Default for ClientPrefs {
    fn default() -> Self {
        Self {
            view_distance: 8,
            language: String::new(),
        }
    }
}

/// Outcome of a successful configuration: the client's preferences from the
/// latest `Client Information` packet seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigOutcome {
    pub client_view_distance: i8,
    /// Client locale carried to the core so settlement dialogs match it.
    pub client_language: String,
    /// Bound only after the client accepted exact verified package identities.
    pub loader_session: Option<mc_loader::LoaderSession>,
}

/// Runs the configuration state over an already-logged-in connection.
/// `tags` is the full resolved tag set (`mc_data::tags::load` at startup):
/// without it the client kicks itself on `FinishConfiguration` with
/// "Unbound tags" (vanilla datapack references `#minecraft:*` tags).
pub async fn handle_configuration(
    conn: &mut Connection,
    data: &mc_data::VanillaData,
    tags: &mc_data::tags::TagsData,
    loader_manifest: Option<&mc_loader::LoaderManifest>,
    loader_required: bool,
    resource_pack: &crate::config::ResourcePackSection,
) -> anyhow::Result<ConfigOutcome> {
    // Vanilla publishes feature flags before registry/known-pack negotiation.
    conn.send_packet(&UpdateEnabledFeatures {
        features: vec![
            mc_data::Identifier::parse("minecraft:vanilla").expect("static feature id is valid"),
        ],
    })
    .await?;

    // Loader content first declares its serverbound channels. Optional-only content
    // then asks whether the client speaks the Loader before known-pack negotiation,
    // so its hello (sent while handling the probe) is read ahead of its known-pack
    // reply.
    let mut loader_hello = false;
    if loader_manifest.is_some() {
        crate::plugin_loader::send_channel_register(conn).await?;
        if !loader_required {
            crate::plugin_loader::send_probe(conn).await?;
        }
    }

    // Step 1: advertise our known data packs (a 26.1.2 session's own release).
    let our_pack = server_known_pack(if conn.legacy().is_some() {
        crate::legacy::RELEASE
    } else {
        TARGET_RELEASE
    });
    conn.send_packet(&ClientboundKnownPacks {
        packs: vec![our_pack.clone()],
    })
    .await?;

    // Step 2: read frames until we see the client's KnownPacks response.
    let mut prefs = ClientPrefs::default();
    let mut ignored = 0usize;
    let client_packs = loop {
        let frame = conn
            .read_frame_in(mc_protocol::State::Configuration)
            .await?;
        if frame.id == ServerboundKnownPacks::ID {
            let parsed = decode_exact::<ServerboundKnownPacks>(frame.id, frame.body)?;
            break parsed.packs;
        }
        if crate::plugin_loader::is_hello(frame.id, &frame.body)? {
            loader_hello = true;
            continue;
        }
        if note_ignored(&mut prefs, frame.id, frame.body)? {
            ignored += 1;
            if ignored > MAX_IGNORED_CONFIGURATION_PACKETS {
                anyhow::bail!(
                    "too many ignored configuration packets (>{MAX_IGNORED_CONFIGURATION_PACKETS})"
                );
            }
        }
    };

    // A matching pack lets the client use its built-in definitions.
    // Otherwise every definition must come from the pre-encoded sidecar.
    let client_has_our_pack = client_packs.iter().any(|p| {
        p.namespace == our_pack.namespace && p.id == our_pack.id && p.version == our_pack.version
    });
    let send_full_registry_data = !client_has_our_pack;
    if send_full_registry_data && !data.has_full_registry_payloads() {
        let advertised = format!(
            "{}:{}:{}",
            our_pack.namespace, our_pack.id, our_pack.version
        );
        anyhow::bail!(
            "client lacks known pack {advertised} and no sidecar registry payloads exist"
        );
    }

    // Step 3: send every registry in sidecar order.
    for registry in data.registries() {
        let entries = registry
            .entries
            .iter()
            .map(|name| {
                let nbt_payload = if send_full_registry_data {
                    Some(
                        data.registry_entry_payload(&registry.id, name)
                            .ok_or_else(|| {
                                anyhow::anyhow!(
                                    "missing registry payload for {} {}",
                                    registry.id.as_str(),
                                    name.as_str()
                                )
                            })?,
                    )
                } else {
                    None
                };
                Ok(RegistryEntry {
                    name: name.clone(),
                    nbt_payload,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        conn.send_packet(&RegistryData {
            registry_id: registry.id.clone(),
            entries,
        })
        .await?;
    }

    // Step 3.5: ship the full tag set. Mojang's built-in datapack contains
    // references to `#minecraft:*` tags; without this packet the client
    // kicks itself on `FinishConfiguration` with "Unbound tags".
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
    conn.send_packet(&tag_packet).await?;

    // Vanilla publishes the server brand after registry/tag negotiation,
    // immediately before finishing the configuration phase.
    conn.send_packet(&ClientboundCustomPayload {
        payload: CustomPayload::Brand(server_brand()),
    })
    .await?;

    // Required bundles negotiate with every client (a vanilla client is refused
    // there). Optional-only manifests negotiate only with a client that answered
    // the probe; everyone else joins without Loader content.
    let loader_session = match loader_manifest {
        Some(manifest) if loader_required || loader_hello => {
            Some(crate::plugin_loader::negotiate(conn, manifest, &mut prefs).await?)
        }
        _ => None,
    };
    crate::resource_pack::negotiate(conn, resource_pack, &mut prefs).await?;

    // Step 4: tell the client we are done configuring.
    conn.send_packet(&FinishConfiguration).await?;

    // Step 5: wait for AcknowledgeFinishConfiguration, ignoring anything else.
    ignored = 0;
    loop {
        let frame = conn
            .read_frame_in(mc_protocol::State::Configuration)
            .await?;
        if frame.id == AcknowledgeFinishConfiguration::ID {
            expect_empty(frame.id, &frame.body)?;
            break;
        }
        if note_ignored_frame(frame.id, frame.body, &mut prefs)? {
            ignored += 1;
            if ignored > MAX_IGNORED_CONFIGURATION_PACKETS {
                anyhow::bail!(
                    "too many ignored configuration packets (>{MAX_IGNORED_CONFIGURATION_PACKETS})"
                );
            }
        }
    }

    Ok(ConfigOutcome {
        client_view_distance: prefs.view_distance,
        client_language: prefs.language,
        loader_session,
    })
}

/// Sends a configuration-state disconnect and returns an Err for the caller.
pub async fn configuration_disconnect(
    conn: &mut Connection,
    reason: &str,
) -> anyhow::Result<ConfigOutcome> {
    conn.send_packet(&ConfigurationDisconnect {
        reason_nbt: text_component_nbt(reason)?,
    })
    .await?;
    anyhow::bail!("configuration disconnect: {reason}")
}

/// The Known Packs entry we advertise as the data pack the session's game
/// version corresponds to ([`TARGET_RELEASE`], or 26.1.2 for a legacy session).
fn server_known_pack(release: &str) -> KnownPackEntry {
    KnownPackEntry {
        namespace: "minecraft".to_owned(),
        id: "core".to_owned(),
        version: release.to_owned(),
    }
}

fn server_brand() -> String {
    format!(
        "Solaris {} (MC {TARGET_RELEASE})",
        env!("CARGO_PKG_VERSION")
    )
}

fn decode_exact<P: Packet>(id: i32, mut body: Bytes) -> anyhow::Result<P> {
    let packet = P::decode(&mut body)?;
    if !body.is_empty() {
        anyhow::bail!("trailing bytes in configuration packet {id:#04x}");
    }
    Ok(packet)
}

fn expect_empty(id: i32, body: &Bytes) -> anyhow::Result<()> {
    if !body.is_empty() {
        anyhow::bail!("trailing bytes in configuration packet {id:#04x}");
    }
    Ok(())
}

/// Tolerates one non-terminal inbound packet. Returns `true` when the packet
/// counts against the ignore budget. Updates client preferences from client
/// information; brand custom payloads and resource-pack statuses are noted
/// and ignored.
pub(crate) fn note_ignored(prefs: &mut ClientPrefs, id: i32, body: Bytes) -> anyhow::Result<bool> {
    if id == ServerboundClientInformation::ID {
        let info = decode_exact::<ServerboundClientInformation>(id, body)?.information;
        prefs.view_distance = info.view_distance;
        prefs.language = info.language;
        return Ok(true);
    }
    if id == ServerboundCustomPayload::ID {
        let _ = CustomPayload::decode_serverbound(&mut body.clone())?;
        return Ok(true);
    }
    if id == ServerboundResourcePack::ID {
        let _ = decode_exact::<ServerboundResourcePack>(id, body)?;
        return Ok(true);
    }
    Ok(true)
}

fn note_ignored_frame(id: i32, body: Bytes, prefs: &mut ClientPrefs) -> anyhow::Result<bool> {
    note_ignored(prefs, id, body)
}

#[cfg(test)]
#[path = "configuration_tests.rs"]
mod tests;
