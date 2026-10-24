//! v2 edge server: accept loop, per-connection pipeline, tick pump.
//!
//! ```text
//! handshake -> status | login -> configuration -> join burst -> play loop
//!                                                       ^            |
//!                                                       |     tick pump: subscribe
//!                                                       |       + publish() + route
//!                                                       +-- ViewerState (plan/fanout)
//! ```
//!
//! # Slow-consumer rule (per-connection half; the core owns the journal
//! # write-ahead + `TickOutput` delivery shape)
//!
//! Each connection owns **two** bounded mpsc queues, both sized
//! [`OUTBOUND_QUEUE_BOUND`]:
//!
//! - **reliable**: [`PlayerChange`](mc_core::boundary::PlayerChange)-derived
//!   packets (join-burst tail, block acks, menus, health, chat, syncpos
//!   corrections, disconnect). In order, never dropped; on overflow the
//!   connection is **disconnected** rather than stalling the tick.
//! - **world**: chunk/entity/block packets. Before each publish the pump
//!   reads the queue's free slots as the viewer's credit; the plan emits at
//!   most that many frames, so nothing planned is dropped, and what did not
//!   fit stays unsent in the viewer cache for a newer snapshot to carry.
//!
//! Reliable-overflow disconnect is the only drop. The writer task drains
//! reliable first, then world.
//!
//! # Lagged pump (world-skip signal, not data loss)
//!
//! The pump's broadcast subscription treats `Lagged` as a world-skip
//! signal: every viewer is marked `skipped` and the next output is planned
//! by reconciling each viewer against that newest snapshot (chunks whose
//! revision moved on are resent, entity moves/appearance compare with what
//! was last sent, vanished entities are removed). It still drains the
//! reliable outbox, because reliable results are never skipped. Only
//! one-shot visuals (swings, hurt flashes) of skipped ticks are lost.
//!
//! # Delivery shape (world broadcast + reliable per-player outbox)
//!
//! After every broadcast tick the pump drains `Core::try_drain_outbox`
//! and routes reliable changes FIRST (before the world publish), then
//! publishes the world snapshot. Only the pump's subscribe site would
//! change if delivery is ever reshaped again.

use mc_core::Core;
use mc_core::boundary::{ChangeSet, ConnectionId, PlayerInput, TickOutput};
use mc_data::VanillaData;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Instant;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, warn};

use crate::config::EdgeConfig;
use crate::dashboard::{
    DashboardChunks, DashboardEntities, DashboardState, DashboardWorld, LoadQueueCapabilities,
    LoadQueueGauge, LoadQueues, unix_ns,
};
use mc_protocol::packets::Packet;

/// Per-connection outbound queue bound (reliable and world queues alike).
pub use crate::fanout::OUTBOUND_QUEUE_BOUND;

/// Next-connection-id counter start (1-based; 0 is reserved as "none").
const FIRST_CONNECTION_ID: u64 = 1;

/// How many consecutive `submit() == false` sheds before the connection is
/// dropped for backpressure (prevents one wedged client spinning forever).
const MAX_SUBMIT_SHEDS: u32 = 64;

/// Per-Play-packet-id token buckets. Paper's packet limiter is permissive for
/// normal clients; these defaults retain that headroom while bounding one
/// packet type independently of all other client traffic.
/// IDs beyond the implemented packet prefix share one bucket, so arbitrary
/// client-supplied VarInts cannot allocate unlimited buckets or renew the burst.
#[derive(Debug)]
struct PacketLimiter {
    rate_per_second: f64,
    burst: f64,
    buckets: HashMap<i32, (f64, Instant)>,
}

impl PacketLimiter {
    fn new(config: &crate::config::AntiCheatSection) -> Self {
        Self {
            rate_per_second: f64::from(config.packet_rate_per_second),
            burst: f64::from(config.packet_burst),
            buckets: HashMap::new(),
        }
    }

    fn allow(&mut self, packet_id: i32) -> bool {
        self.allow_at(packet_id, Instant::now())
    }

    fn allow_at(&mut self, packet_id: i32, now: Instant) -> bool {
        if self.rate_per_second == 0.0 || self.burst == 0.0 {
            return true;
        }
        let packet_id = if (0..=mc_protocol::packets::play::ServerboundCustomClickAction::ID)
            .contains(&packet_id)
        {
            packet_id
        } else {
            -1
        };
        let (tokens, updated) = self.buckets.entry(packet_id).or_insert((self.burst, now));
        *tokens = (*tokens + now.duration_since(*updated).as_secs_f64() * self.rate_per_second)
            .min(self.burst);
        *updated = now;
        if *tokens < 1.0 {
            return false;
        }
        *tokens -= 1.0;
        true
    }
}

const NATIVE_SETTLEMENTS_OWNER: &str = "solaris-settlements";

fn native_settlements_bundle() -> anyhow::Result<mc_script::ClientBundle> {
    static BUNDLE: LazyLock<Result<mc_script::ClientBundle, String>> =
        LazyLock::new(|| build_native_settlements_bundle().map_err(|error| error.to_string()));
    BUNDLE
        .as_ref()
        .map(Clone::clone)
        .map_err(anyhow::Error::msg)
}

fn build_native_settlements_bundle() -> anyhow::Result<mc_script::ClientBundle> {
    let index = br#"{
  "schema": 2,
  "screens": [{
    "id": "solaris-settlements:overview",
    "overlay": true,
    "title": "Settlements",
    "widgets": [],
    "view_request_bindings": [{
      "key": "key.keyboard.c",
      "view": "solaris-settlements:overview"
    }]
  }]
}"#;
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    archive.start_file(
        "solaris-client.json",
        zip::write::SimpleFileOptions::default(),
    )?;
    archive.write_all(index)?;
    let bytes = archive.finish()?.into_inner();
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    Ok(mc_script::ClientBundle::new(
        NATIVE_SETTLEMENTS_OWNER,
        "native-settlements",
        "1",
        "native/settlements.zip",
        sha256,
        bytes.len() as u64,
        "native/settlements.zip".into(),
        Arc::from(bytes),
        vec![
            mc_script::ClientLoader::Fabric,
            mc_script::ClientLoader::NeoForge,
            mc_script::ClientLoader::Forge,
        ],
        vec![
            mc_script::ClientContentKind::Views,
            mc_script::ClientContentKind::ViewActions,
        ],
        vec![
            mc_script::ClientPermission::PresentViews,
            mc_script::ClientPermission::SendViewActions,
        ],
    )
    .with_required(false))
}

enum LoaderViewIngress {
    NotLoader,
    Discard,
    Request(PlayerInput),
}

fn loader_view_ingress(
    manifest: &mc_loader::LoaderManifest,
    connection: ConnectionId,
    body: &[u8],
) -> anyhow::Result<LoaderViewIngress> {
    use mc_protocol::packets::CustomPayload;
    use mc_protocol::packets::Packet;
    use mc_protocol::packets::play::ServerboundCustomPayload;

    let mut body = body;
    let packet = ServerboundCustomPayload::decode(&mut body)?;
    anyhow::ensure!(body.is_empty(), "trailing Loader Play custom payload bytes");
    let CustomPayload::Unknown { channel, payload } = packet.payload else {
        return Ok(LoaderViewIngress::NotLoader);
    };
    if channel != *mc_loader::loader_view_action_channel() {
        return Ok(LoaderViewIngress::NotLoader);
    }
    let mc_loader::LoaderViewClientRequest::ViewRequest { request_kind } =
        mc_loader::decode_loader_view_client_request(&payload)?
    else {
        return Ok(LoaderViewIngress::Discard);
    };
    let Some((owner, _)) = manifest
        .declared_view_kinds()
        .find(|(_, declared)| *declared == &request_kind)
    else {
        return Ok(LoaderViewIngress::Discard);
    };
    if !manifest.plugin_grants(
        owner,
        mc_loader::LoaderContentKind::Views,
        mc_loader::LoaderPermission::PresentViews,
    ) {
        return Ok(LoaderViewIngress::Discard);
    }
    Ok(LoaderViewIngress::Request(PlayerInput::LoaderViewRequest {
        connection,
        owner: owner.to_owned(),
        request_kind,
    }))
}

/// The edge server: shared core, vanilla data, config, and live state.
pub struct EdgeServer {
    /// The authoritative core. The edge touches it only through
    /// `submit` / `subscribe` / `try_drain_outbox` / `shutdown`.
    pub hub: Arc<Core>,
    /// Vanilla registries for Configuration + join burst.
    pub data: Arc<VanillaData>,
    /// Resolved vanilla tag set for the Configuration tag packet.
    pub tags: Arc<mc_data::tags::TagsData>,
    /// Chunk-encoding registries (built once at startup; the tick pump
    /// must encode against the real biome/block/item tables, never the
    /// single-biome fallback inside `fanout::publish`).
    pub wire: WireRegistries,
    /// Server configuration.
    pub cfg: EdgeConfig,
    /// Live connection registry (for pump routing + dashboard roster).
    pub connections: Arc<ConnectionRegistry>,
    /// Dashboard state (updated off the tick path).
    pub dashboard: Arc<DashboardState>,
    /// Tick stats for the dashboard rollup.
    pub tick_stats: Arc<TickStats>,
    /// Network counters backing `/stats network`.
    pub net_counters: Arc<NetCounters>,
    /// Join-burst recipe packets, built once from the core's recipe list.
    pub recipes: crate::recipes::RecipePackets,
    pub(crate) admin: crate::admin::Admin,
    pub(crate) console_state: crate::console::ConsoleState,
    stop: tokio::sync::watch::Sender<bool>,
    suggestion_items: Vec<String>,
    suggestion_blocks: Vec<String>,
    suggestion_entities: Vec<String>,
    suggestion_structures: Vec<String>,
    online_authentication: Option<crate::login::OnlineAuthentication>,
    /// Velocity modern-forwarding secret, when `proxy.mode = "velocity"`.
    velocity: Option<crate::proxy::VelocityForwarding>,
    loader_manifest: Option<mc_loader::LoaderManifest>,
    /// Whether a bundle in `loader_manifest` is required (vanilla clients refused).
    loader_required: bool,
    /// `[lod]` tile service for native clients; `None` when disabled.
    pub(crate) lod: Option<crate::lod::LodService>,
    /// Optional Java Distant Horizons presentation; shares native LOD workers.
    pub(crate) dh: Option<Arc<crate::dh::Service>>,
}

/// Owned chunk-encoding registries for the pump path.
pub struct WireRegistries {
    pub dimension_type_ids: [i32; 3],
    /// Full `worldgen/biome` registry from the sidecar (NOT the fallback).
    pub biomes: mc_data::Registry,
    /// Block table from the required-blocks report.
    pub blocks: mc_world::BlockRegistry,
    /// Item table for block-entity encoding.
    pub items: mc_data::items::ItemRegistry,
    /// Block-entity type table.
    pub block_entity_types: mc_data::block_entity_types::BlockEntityTypeRegistry,
}

/// Live connection registry: outbound queue handles keyed by connection,
/// plus the player-name roster for the dashboard.
/// Stashed join result: (entity, spawn position, game mode, dimension).
type JoinedSpawn = (
    mc_core::boundary::EntityId,
    [f64; 3],
    u8,
    mc_core::dimensions::Dimension,
);

/// The Loader manifest: every plugin bundle plus the native settlements view
/// bundle. The native bundle is optional, so a deployment whose plugin bundles are
/// all optional (or absent) offers the manifest only to clients that answer the
/// Loader probe and never makes the Loader mandatory for vanilla clients.
pub(crate) fn loader_manifest(
    plugin_bundles: &[mc_script::ClientBundle],
) -> anyhow::Result<Option<mc_loader::LoaderManifest>> {
    let mut bundles = plugin_bundles.to_vec();
    bundles.push(native_settlements_bundle()?);
    let manifest = mc_loader::LoaderManifest::from_script_bundles(&bundles)?;
    manifest.encode()?;
    Ok(Some(manifest))
}

#[path = "server_lifecycle.rs"]
mod lifecycle;
use lifecycle::ConnectionLifecycle;

impl EdgeServer {
    /// Build a server from its owned pieces plus fresh live state.
    /// Fails when the biome registry, block table, or light table cannot
    /// be built: chunk encoding without them would silently send no
    /// terrain or unlit chunks the client never considers loaded.
    pub fn new(
        hub: Arc<Core>,
        data: Arc<VanillaData>,
        tags: Arc<mc_data::tags::TagsData>,
        cfg: EdgeConfig,
    ) -> anyhow::Result<Self> {
        cfg.computer.validate()?;
        let dashboard = dashboard_state(&cfg);
        let loader_manifest = loader_manifest(&hub.registry().plugin_client_bundles)?;
        // Any required bundle forces Loader negotiation on every client; a
        // manifest of only optional bundles is offered to Loader clients alone.
        let loader_required = hub
            .registry()
            .plugin_client_bundles
            .iter()
            .any(mc_script::ClientBundle::required);
        if let Some(manifest) = &loader_manifest {
            manifest.validate_custom_items(hub.registry().plugin_items(), |id| {
                hub.registry().is_native_item(id)
            })?;
        }
        if !hub.plugin_packages().is_empty() {
            dashboard.set_plugin_source(Arc::clone(&hub));
        }
        let connections = Arc::new(ConnectionRegistry::new());
        dashboard.set_queue_source(Arc::clone(&connections));
        let biomes = data.registry("worldgen/biome").cloned().ok_or_else(|| {
            anyhow::anyhow!("worldgen/biome registry missing: chunk encoding impossible")
        })?;
        let blocks = mc_world::BlockRegistry::from_report(
            &mc_data::blocks::solaris_required_blocks_report(),
        )
        .map_err(|e| anyhow::anyhow!("block registry: {e}"))?;
        let wire = WireRegistries {
            dimension_type_ids: crate::dimensions::registry_ids(&data)?,
            biomes,
            blocks,
            items: mc_data::items::solaris_required_items(),
            block_entity_types: mc_data::block_entity_types::solaris_required_block_entity_types(),
        };
        let recipes = crate::recipes::RecipePackets::new(hub.registry())?;
        let admin = crate::admin::Admin::open(&cfg.server_dir)?;
        if let Some(enabled) = cfg.server.whitelist
            && !cfg.server_dir.join("whitelist.enabled").exists()
        {
            admin.set_whitelist(enabled)?;
        }
        let (stop, _) = tokio::sync::watch::channel(false);
        let online_authentication = cfg
            .auth
            .online_mode
            .then(|| crate::login::OnlineAuthentication::with_mojang_verifier(false))
            .transpose()?;
        let velocity = match cfg.proxy.mode {
            crate::config::ProxyMode::None => None,
            crate::config::ProxyMode::Velocity => {
                Some(crate::proxy::VelocityForwarding::new(&cfg.proxy.secret)?)
            }
        };
        let suggestion_items = hub
            .registry()
            .items
            .iter()
            .map(|(name, _)| name.to_string())
            .collect();
        let suggestion_blocks = mc_data::blocks::solaris_required_blocks_report()
            .into_iter()
            .map(|block| block.id.to_string())
            .collect();
        let suggestion_entities = data
            .registry("entity_type")
            .map(|registry| registry.entries.iter().map(ToString::to_string).collect())
            .unwrap_or_default();
        let suggestion_structures =
            crate::command_tree::structure_suggestion_ids(&cfg.data.vanilla_data_dir);
        let lod = cfg
            .lod
            .enabled
            .then(|| crate::lod::LodService::new(&cfg.lod, &wire.biomes));
        let dh = cfg
            .distant_horizons
            .enabled
            .then(|| {
                let workers = lod
                    .as_ref()
                    .map(|lod| Arc::clone(&lod.workers))
                    .unwrap_or_else(|| {
                        Arc::new(tokio::sync::Semaphore::new(cfg.lod.workers.max(1)))
                    });
                crate::dh::Service::new(&hub, &cfg, workers).map(Arc::new)
            })
            .transpose()?;
        Ok(Self {
            admin,
            console_state: Default::default(),
            stop,
            suggestion_items,
            suggestion_blocks,
            suggestion_entities,
            suggestion_structures,
            online_authentication,
            velocity,
            loader_manifest,
            loader_required,
            lod,
            dh,
            recipes,
            hub,
            data,
            tags,
            wire,
            cfg,
            connections,
            dashboard,
            tick_stats: Arc::new(TickStats::new()),
            net_counters: Arc::new(NetCounters::new()),
        })
    }
}
impl EdgeServer {
    fn send<P: mc_protocol::packets::Packet>(
        &self,
        id: ConnectionId,
        packet: &P,
    ) -> anyhow::Result<()> {
        let mut body = Vec::new();
        crate::vanilla_items::encode(id, packet, &mut body)?;
        let mut frame = bytes::BytesMut::new();
        mc_protocol::frame::append_frame(
            &mut frame,
            P::ID,
            &body,
            mc_protocol::frame::Compression::Threshold(256),
        )?;
        // Cold command delivery needs only the reliable sender, not a copy
        // of both queue handles. Release the registry before enqueueing.
        let sender = self
            .connections
            .inner
            .lock()
            .handles
            .get(&id)
            .map(|handles| handles.reliable_tx.clone());
        if let Some(sender) = sender
            && sender.try_send(frame.freeze()).is_err()
        {
            self.connections.remove(id);
            anyhow::bail!("reliable command queue full");
        }
        Ok(())
    }

    fn feedback(&self, id: ConnectionId, text: &str) -> anyhow::Result<()> {
        if id == mc_core::command_console::CONNECTION {
            self.console_state.feedback(text);
            return Ok(());
        }
        let mut content_nbt = Vec::new();
        mc_nbt::write_network(
            &mut content_nbt,
            &mc_nbt::Tag::Compound(vec![("text".into(), mc_nbt::Tag::String(text.into()))]),
        )?;
        self.send(
            id,
            &mc_protocol::packets::play::ClientboundSystemChat {
                content_nbt,
                overlay: false,
            },
        )
    }

    pub(crate) fn kick(&self, id: ConnectionId, reason: serde_json::Value) -> anyhow::Result<()> {
        let fields = reason
            .as_object()
            .expect("disconnect component")
            .iter()
            .map(|(key, value)| {
                let tag = if let Some(values) = value.as_array() {
                    mc_nbt::Tag::List(mc_nbt::ListTag {
                        element_type: mc_nbt::tag_type::STRING,
                        elements: values
                            .iter()
                            .map(|value| {
                                mc_nbt::Tag::String(value.as_str().unwrap_or_default().into())
                            })
                            .collect(),
                    })
                } else {
                    mc_nbt::Tag::String(value.as_str().unwrap_or_default().into())
                };
                (key.clone(), tag)
            })
            .collect();
        let mut reason_nbt = Vec::new();
        mc_nbt::write_network(&mut reason_nbt, &mc_nbt::Tag::Compound(fields))?;
        self.connections
            .stash_disconnect_reason(id, reason.to_string());
        self.send(
            id,
            &mc_protocol::packets::play::PlayDisconnect { reason_nbt },
        )
    }

    /// Vanilla `/transfer <hostname> [port] [players]`: sends the clientbound
    /// Transfer packet to each target. The client drops the current
    /// connection itself and reconnects; the server keeps the session until
    /// the socket closes.
    fn transfer_command(&self, id: ConnectionId, args: &[&str]) -> anyhow::Result<()> {
        let Some(hostname) = args.first().filter(|host| !host.is_empty()) else {
            self.feedback(id, "Incomplete command")?;
            return Ok(());
        };
        let (port, names) = match args.get(1) {
            Some(token) if token.bytes().all(|byte| byte.is_ascii_digit()) => {
                let Ok(port) = token.parse::<u16>() else {
                    self.feedback(id, "Incorrect argument for command")?;
                    return Ok(());
                };
                if port == 0 {
                    self.feedback(id, "Incorrect argument for command")?;
                    return Ok(());
                }
                (i32::from(port), &args[2..])
            }
            _ => (25_565, &args[1..]),
        };
        let mut targets = Vec::new();
        for name in names {
            match self.admin.find(name) {
                Some((other, _, display, _)) => targets.push((other, display)),
                None => {
                    self.feedback(id, "No player was found")?;
                    return Ok(());
                }
            }
        }
        if targets.is_empty() {
            if id == mc_core::command_console::CONNECTION {
                self.feedback(id, "Must specify at least one player to transfer")?;
                return Ok(());
            }
            let display = self
                .connections
                .inner
                .lock()
                .names
                .get(&id)
                .cloned()
                .unwrap_or_default();
            targets.push((id, display));
        }
        for (other, _) in &targets {
            self.send(
                *other,
                &mc_protocol::packets::play::ClientboundTransfer {
                    host: (*hostname).to_owned(),
                    port,
                },
            )?;
        }
        let message = if targets.len() == 1 {
            format!("Transferring {} to {hostname}:{port}", targets[0].1)
        } else {
            format!(
                "Transferring {} players to {hostname}:{port}",
                targets.len()
            )
        };
        self.feedback(id, &message)
    }

    async fn handle_edge_frame(
        &self,
        id: ConnectionId,
        packet_id: i32,
        body: &[u8],
    ) -> anyhow::Result<bool> {
        use mc_protocol::packets::{
            Packet,
            play::{ServerboundClientInformation, ServerboundCommandSuggestion},
        };
        if packet_id == ServerboundClientInformation::ID {
            let mut input = body;
            let request = ServerboundClientInformation::decode(&mut input)?;
            anyhow::ensure!(input.is_empty(), "trailing client information data");
            let view_distance = effective_view_distance(
                request.information.view_distance,
                self.cfg.server.view_distance,
            );
            let mut registry = self.connections.inner.lock();
            if registry.view_distances.get(&id) != Some(&view_distance) {
                anyhow::ensure!(
                    self.hub.submit(PlayerInput::SetViewDistance {
                        connection: id,
                        view_distance: view_distance as u8,
                    }),
                    "view distance admission queue is full"
                );
                registry.view_distances.insert(id, view_distance);
            }
            let locale = request.information.language;
            if registry.locales.get(&id) != Some(&locale) {
                anyhow::ensure!(
                    self.hub.submit(PlayerInput::SetLocale {
                        connection: id,
                        locale: locale.clone(),
                    }),
                    "locale admission queue is full"
                );
                registry.locales.insert(id, locale);
            }
            return Ok(true);
        }
        if packet_id != ServerboundCommandSuggestion::ID {
            return Ok(false);
        }
        let mut input = body;
        let request = ServerboundCommandSuggestion::decode(&mut input)?;
        anyhow::ensure!(input.is_empty(), "trailing command suggestion data");
        // Locked sessions render suggestions at level 0: op-list state must
        // not leak privileged roots to an unauthenticated client.
        let level = if self.hub.session_locked(id) {
            0
        } else {
            self.admin.session_level(id)
        };
        let response = crate::command_tree::suggestions(
            request.id,
            &request.command,
            level,
            &self.connections.roster(),
            &self.suggestion_items,
            &self.suggestion_blocks,
            &self.suggestion_entities,
            &self.suggestion_structures,
            self.hub.plugin_commands(),
        );
        self.send(id, &response)?;
        Ok(true)
    }

    pub(crate) async fn handle_admin_command(
        &self,
        id: ConnectionId,
        command: &str,
    ) -> anyhow::Result<bool> {
        let permission_level = if id == mc_core::command_console::CONNECTION {
            4
        } else {
            self.admin.session_level(id)
        };
        self.execute_admin_command(id, command, permission_level)
            .await
    }

    // Play already resolved the permission for the authoritative input.
    // A locked (unauthenticated) session must not run any edge-side command
    // effects: every mutating admin root is fenced here, and anything else
    // falls through to the native gate, which refuses locked gameplay inputs.
    async fn execute_admin_command(
        &self,
        id: ConnectionId,
        command: &str,
        permission_level: u8,
    ) -> anyhow::Result<bool> {
        if id != mc_core::command_console::CONNECTION && self.hub.session_locked(id) {
            return Ok(false);
        }
        let mut words = command.split_whitespace();
        let root = words.next().unwrap_or_default();
        if self
            .hub
            .plugin_commands()
            .iter()
            .any(|(name, _)| name == root)
        {
            return Ok(false);
        }
        if matches!(root, "tps" | "mspt") {
            if permission_level < 2 || words.next().is_some() {
                self.feedback(id, "Unknown or incomplete command")?;
                return Ok(true);
            }
            let metrics = self.dashboard.snapshot();
            let mspt = metrics.tick.p50_us as f64 / 1_000.0;
            let message = if root == "tps" {
                format!(
                    "TPS: {:.2}; MSPT: {:.2} (p95 {:.2}, p99 {:.2})",
                    metrics.tps,
                    mspt,
                    metrics.tick.p95_us as f64 / 1_000.0,
                    metrics.tick.p99_us as f64 / 1_000.0
                )
            } else {
                format!(
                    "MSPT: {:.2} median, {:.2} p95, {:.2} p99",
                    mspt,
                    metrics.tick.p95_us as f64 / 1_000.0,
                    metrics.tick.p99_us as f64 / 1_000.0
                )
            };
            self.feedback(id, &message)?;
            return Ok(true);
        }
        if root == "settlement" && words.clone().eq(["guide"]) {
            self.feedback(id, crate::command_tree::SETTLEMENT_GUIDE)?;
            return Ok(true);
        }
        if root == "list" {
            let option = words.next();
            if option.is_some_and(|arg| arg != "uuids") || words.next().is_some() {
                self.feedback(id, "Incorrect argument for command")?;
            } else {
                let names = self.admin.roster(option.is_some());
                self.feedback(
                    id,
                    &format!(
                        "There are {} of a max of {} players online: {}",
                        names.len(),
                        self.cfg.server.max_players,
                        names.join(", ")
                    ),
                )?;
            }
            return Ok(true);
        }
        if root == "help" {
            let path = command.strip_prefix("help").unwrap_or_default().trim();
            match crate::command_tree::help(
                (!path.is_empty()).then_some(path),
                permission_level,
                self.hub.plugin_commands(),
            ) {
                Ok(usages) => self.feedback(id, &usages.join("\n"))?,
                Err(error) => self.feedback(id, &error)?,
            }
            return Ok(true);
        }
        if !matches!(
            root,
            "op" | "deop"
                | "ban"
                | "pardon"
                | "ban-ip"
                | "pardon-ip"
                | "kick"
                | "mute"
                | "unmute"
                | "warn"
                | "transfer"
                | "whitelist"
                | "banlist"
                | "stop"
                | "save-all"
        ) {
            return Ok(false);
        }
        if !crate::command_tree::permitted(command, permission_level, self.hub.plugin_commands()) {
            self.feedback(id, "Unknown or incomplete command")?;
            return Ok(true);
        }
        if root == "whitelist" {
            self.whitelist_command(id, &words.collect::<Vec<_>>())?;
            return Ok(true);
        }
        if root == "banlist" {
            self.banlist_command(id, &words.collect::<Vec<_>>())?;
            return Ok(true);
        }
        if root == "transfer" {
            self.transfer_command(id, &words.collect::<Vec<_>>())?;
            return Ok(true);
        }
        let target = words.next();
        let arguments = words.collect::<Vec<_>>();
        // `/ban` and `/mute` accept an optional duration as the first argument
        // after the target; anything that does not parse as `1d`/`2h30m`/... is
        // the start of the reason, so vanilla `/ban <player> <reason>` is intact.
        let (duration, reason) = if matches!(root, "ban" | "mute") {
            match arguments.split_first() {
                Some((first, rest)) => match crate::admin::parse_duration(first) {
                    Some(duration) => (Some(duration), rest.join(" ")),
                    None => (None, arguments.join(" ")),
                },
                None => (None, String::new()),
            }
        } else {
            (None, arguments.join(" "))
        };
        if root == "stop" {
            if target.is_some() {
                self.feedback(id, "Incorrect argument for command")?;
            } else {
                self.feedback(id, "Stopping the server")?;
                self.stop.send_replace(true);
            }
            return Ok(true);
        }
        if root == "save-all" {
            if target.is_some_and(|arg| arg != "flush") || !reason.is_empty() {
                self.feedback(id, "Incorrect argument for command")?;
            } else {
                self.feedback(id, "Saving the game (this may take a moment!)")?;
                let hub = Arc::clone(&self.hub);
                let saved = tokio::task::spawn_blocking(move || hub.checkpoint()).await?;
                self.feedback(
                    id,
                    if saved {
                        "Saved the game"
                    } else {
                        "Failed to save the game"
                    },
                )?;
            }
            return Ok(true);
        }
        let Some(target) = target else {
            self.feedback(id, "Incomplete command")?;
            return Ok(true);
        };
        let live = self.admin.find(target);
        if matches!(root, "op" | "deop" | "pardon" | "pardon-ip") && !reason.is_empty() {
            self.feedback(id, "Incorrect argument for command")?;
            return Ok(true);
        }
        let source = self
            .connections
            .inner
            .lock()
            .names
            .get(&id)
            .cloned()
            .unwrap_or_else(|| "Server".into());
        let result =
            if root == "ban-ip" || root == "pardon-ip" {
                let ip = target
                    .parse::<std::net::IpAddr>()
                    .ok()
                    .or_else(|| live.as_ref().map(|s| s.3));
                if let Some(ip) = ip {
                    let result = self.admin.ban_ip(
                        &ip.to_string(),
                        &source,
                        if reason.is_empty() {
                            "Banned by an operator."
                        } else {
                            &reason
                        },
                        root == "ban-ip",
                    );
                    if result.is_ok() && root == "ban-ip" {
                        for (other, _) in self.connections.snapshot() {
                            if self.admin.has_ip(other, ip) {
                                self.kick(
                                other,
                                serde_json::json!({"translate":"multiplayer.disconnect.ip_banned"}),
                            )?;
                            }
                        }
                    }
                    result
                } else {
                    self.feedback(id, "Invalid IP address")?;
                    return Ok(true);
                }
            } else if root == "kick" {
                if let Some((other, _, _, _)) = live {
                    self.kick(
                        other,
                        if reason.is_empty() {
                            serde_json::json!({"translate":"multiplayer.disconnect.kicked"})
                        } else {
                            serde_json::json!({"text":reason})
                        },
                    )
                } else {
                    self.feedback(id, "No player was found")
                }
            } else {
                // A locked session never reaches here: the fence above
                // returns `Ok(false)`. Username targeting stays as-is so
                // unlocked operators and the console can manage offline-mode
                // and not-yet-joined names.
                let profile = live
                    .as_ref()
                    .map(|s| (s.1, s.2.clone()))
                    .or_else(|| self.admin.profile(target))
                    .or_else(|| {
                        (!self.cfg.auth.online_mode)
                            .then(|| (crate::crypto::offline_uuid(target), target.into()))
                    });
                let Some((uuid, name)) = profile else {
                    self.feedback(id, "Unknown player")?;
                    return Ok(true);
                };
                match root {
                    "op" | "deop" => {
                        let result = self.admin.set_op(uuid, &name, root == "op");
                        if result.is_ok()
                            && let Some((other, _, _, _)) = live
                        {
                            let permission = PlayerInput::PermissionLevel {
                                connection: other,
                                level: self.admin.level(uuid),
                            };
                            let mut permission_sheds = 0;
                            while !self.hub.submit(permission.clone()) {
                                permission_sheds += 1;
                                if permission_sheds >= MAX_SUBMIT_SHEDS {
                                    self.kick(other, serde_json::json!({
                                    "text": "Authoritative permission update admission failed"
                                }))?;
                                    anyhow::bail!("authoritative permission update queue is full");
                                }
                                tokio::task::yield_now().await;
                            }
                            self.send(
                                other,
                                &crate::command_tree::tree(
                                    self.admin.level(uuid),
                                    self.hub.plugin_commands(),
                                ),
                            )?;
                        }
                        result
                    }
                    "ban" | "pardon" => {
                        let result = self.admin.ban_player_for(
                            uuid,
                            &name,
                            &source,
                            if reason.is_empty() {
                                "Banned by an operator."
                            } else {
                                &reason
                            },
                            duration,
                            root == "ban",
                        );
                        if result.is_ok()
                            && root == "ban"
                            && let Some((other, _, _, _)) = live
                        {
                            self.kick(
                                other,
                                serde_json::json!({"translate":"multiplayer.disconnect.banned"}),
                            )?;
                        }
                        result
                    }
                    "mute" | "unmute" => {
                        let result = self.admin.mute_player(
                            uuid,
                            &name,
                            &source,
                            if reason.is_empty() {
                                "Muted by an operator."
                            } else {
                                &reason
                            },
                            duration,
                            root == "mute",
                        );
                        if result.is_ok()
                            && root == "mute"
                            && let Some((other, _, _, _)) = live
                        {
                            self.feedback(other, "You have been muted.")?;
                        }
                        result
                    }
                    "warn" => {
                        let Some((other, _, _, _)) = live else {
                            self.feedback(id, "No player was found")?;
                            return Ok(true);
                        };
                        self.feedback(
                            other,
                            &format!(
                                "[Warned] {}",
                                if reason.is_empty() {
                                    "Follow the server rules."
                                } else {
                                    &reason
                                }
                            ),
                        )
                    }
                    _ => unreachable!(),
                }
            };
        match result {
            Ok(()) => {
                let expiry = match duration {
                    Some(duration) => format!(" (expires in {}s)", duration.as_secs()),
                    None => String::new(),
                };
                self.feedback(
                    id,
                    &format!("Command {root} completed for {target}{expiry}"),
                )?
            }
            Err(error) => self.feedback(id, &format!("Command failed: {error}"))?,
        }
        Ok(true)
    }

    /// Vanilla `/whitelist on|off|add|remove|list|reload`.
    ///
    /// The list file stays a plain `whitelist.json` array; the enforcement
    /// toggle persists in `whitelist.enabled` beside it.
    fn whitelist_command(&self, id: ConnectionId, words: &[&str]) -> anyhow::Result<()> {
        let (sub, rest): (Option<&str>, &[&str]) = match words.split_first() {
            Some((sub, rest)) => (Some(*sub), rest),
            None => (None, &[]),
        };
        match (sub, rest) {
            (Some("on"), []) => {
                self.admin.set_whitelist(true)?;
                self.feedback(id, "Whitelist is now turned on")
            }
            (Some("off"), []) => {
                self.admin.set_whitelist(false)?;
                self.feedback(id, "Whitelist is now turned off")
            }
            (Some("list"), []) => {
                let names = self.admin.whitelist_names();
                if names.is_empty() {
                    self.feedback(id, "There are no whitelisted players")
                } else {
                    self.feedback(
                        id,
                        &format!(
                            "There are {} whitelisted player(s): {}",
                            names.len(),
                            names.join(", ")
                        ),
                    )
                }
            }
            (Some("reload"), []) => {
                self.admin.reload_whitelist()?;
                self.feedback(id, "Reloaded the whitelist")
            }
            (Some("add"), [target]) => {
                let profile = self
                    .admin
                    .find(target)
                    .map(|(_, uuid, name, _)| (uuid, name))
                    .or_else(|| self.admin.profile(target))
                    .or_else(|| {
                        (!self.cfg.auth.online_mode)
                            .then(|| (crate::crypto::offline_uuid(target), (*target).to_owned()))
                    });
                let Some((uuid, name)) = profile else {
                    return self.feedback(id, "No player was found");
                };
                if self.admin.whitelist_add(uuid, &name)? {
                    self.feedback(id, &format!("Added {name} to the whitelist"))
                } else {
                    self.feedback(id, "That player is already whitelisted")
                }
            }
            (Some("remove"), [target]) => {
                if self.admin.whitelist_remove(target)? {
                    self.feedback(id, &format!("Removed {target} from the whitelist"))
                } else {
                    self.feedback(id, "Nothing changed. The player is not whitelisted")
                }
            }
            _ => self.feedback(id, "Incorrect argument for command"),
        }
    }

    /// Vanilla `/banlist players|ips`; expired temporary bans are not listed,
    /// exactly like vanilla's own list read.
    fn banlist_command(&self, id: ConnectionId, words: &[&str]) -> anyhow::Result<()> {
        let (players, ips) = self.admin.bans();
        match words {
            ["players"] => {
                self.feedback(
                    id,
                    &format!("There are {} total banned players:", players.len()),
                )?;
                for line in players {
                    self.feedback(id, &line)?;
                }
                Ok(())
            }
            ["ips"] => {
                self.feedback(id, &format!("There are {} total banned IPs:", ips.len()))?;
                for line in ips {
                    self.feedback(id, &line)?;
                }
                Ok(())
            }
            _ => self.feedback(id, "Incorrect argument for command"),
        }
    }
}
fn stream_peer_ip(peer: &str) -> Option<std::net::IpAddr> {
    peer.parse::<std::net::SocketAddr>()
        .ok()
        .map(|address| address.ip())
}

pub struct ConnectionRegistry {
    next_id: AtomicU64,
    inner: parking_lot::Mutex<RegistryInner>,
}

#[derive(Default)]
struct RegistryInner {
    // A Closing connection retains its slot until native/admin cleanup runs.
    admitted: HashSet<ConnectionId>,
    joining: HashSet<ConnectionId>,
    closers: HashMap<ConnectionId, tokio::sync::oneshot::Sender<()>>,
    handles: HashMap<ConnectionId, ConnectionHandles>,

    names: HashMap<ConnectionId, String>,
    view_distances: HashMap<ConnectionId, i32>,
    /// Client locale last admitted to the core (`ClientInformation.language`);
    /// deduplicates Play updates and is cleared on disconnect.
    locales: HashMap<ConnectionId, String>,
    roster_dirty: bool,
    // Join state and its waiters share the queue registry's atomic removal.
    pending_joins: HashMap<ConnectionId, JoinedSpawn>,
    pending_reliable: HashMap<ConnectionId, Vec<mc_core::boundary::PlayerChange>>,
    join_waiters: HashMap<ConnectionId, Vec<tokio::sync::oneshot::Sender<JoinedSpawn>>>,
    disconnect_reasons: HashMap<ConnectionId, String>,
}

/// Queue handles for one Play connection.
#[derive(Clone)]
pub struct ConnectionHandles {
    /// Reliable queue sender (join tail, acks, menus, health, chat, syncpos, disconnect).
    pub reliable_tx: mpsc::Sender<bytes::Bytes>,
    /// World queue sender (pre-framed chunk/entity packets).
    pub world_tx: mpsc::Sender<bytes::Bytes>,
}

impl ConnectionRegistry {
    /// Empty registry; ids start at 1.
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(FIRST_CONNECTION_ID),
            inner: parking_lot::Mutex::new(RegistryInner::default()),
        }
    }

    /// Allocate the next connection id.
    pub fn alloc_id(&self) -> ConnectionId {
        ConnectionId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Reserve a player slot atomically with the actual admission policy.
    /// Joining connections count too; the policy retains operator bypass.
    fn admit(
        &self,
        id: ConnectionId,
        maximum: usize,
        check: impl FnOnce(bool) -> Option<serde_json::Value>,
    ) -> Option<serde_json::Value> {
        let mut inner = self.inner.lock();
        let refusal = check(inner.admitted.len() >= maximum);
        if refusal.is_none() {
            inner.admitted.insert(id);
            inner.joining.insert(id);
        }
        refusal
    }

    #[cfg(test)]
    fn joining_id(&self) -> ConnectionId {
        let id = self.alloc_id();
        assert!(self.admit(id, usize::MAX, |_| None).is_none());
        id
    }

    /// Register a connection's queue handles and take the reliable changes the
    /// pump stashed before registration, in one critical section with
    /// [`handles_or_stash`][Self::handles_or_stash]: every drained change is
    /// either returned here for replay or routed by the pump through the
    /// registered handles, never stranded in between.
    pub fn register(
        &self,
        id: ConnectionId,
        handles: ConnectionHandles,
    ) -> anyhow::Result<Vec<mc_core::boundary::PlayerChange>> {
        let mut inner = self.inner.lock();
        anyhow::ensure!(
            inner.joining.remove(&id),
            "connection admission already closed"
        );
        inner.handles.insert(id, handles);
        Ok(inner.pending_reliable.remove(&id).unwrap_or_default())
    }

    /// Read actual mpsc occupied-or-reserved slots only on telemetry requests.
    /// Senders cannot reveal queued versus reserved slots, or byte totals.
    pub fn load_queues(&self) -> Option<LoadQueues> {
        let captured_unix_ns = unix_ns();
        let inner = self.inner.lock();
        let mut reliable = LoadQueueGauge::default();
        let mut world = LoadQueueGauge::default();
        for handles in inner.handles.values() {
            for (gauge, sender) in [
                (&mut reliable, &handles.reliable_tx),
                (&mut world, &handles.world_tx),
            ] {
                let capacity = sender.max_capacity();
                let occupied = capacity.saturating_sub(sender.capacity());
                gauge.capacity_slots += capacity;
                gauge.queued_or_reserved_slots += occupied;
                gauge.max_viewer_queued_or_reserved_slots =
                    gauge.max_viewer_queued_or_reserved_slots.max(occupied);
            }
        }
        Some(LoadQueues {
            captured_unix_ns,
            completed_unix_ns: unix_ns(),
            viewers: inner.handles.len(),
            capabilities: LoadQueueCapabilities {
                queued_or_reserved_slots: true,
                queued_slots: false,
                reserved_slots: false,
                queued_bytes: false,
                reserved_bytes: false,
            },
            reliable,
            world,
        })
    }

    /// Stash a Joined the pump drained before the connection registered.
    /// Also fires any registered join waiters for this connection.
    fn stash_join(&self, id: ConnectionId, joined: JoinedSpawn) {
        let senders = {
            let mut inner = self.inner.lock();
            if !inner.joining.contains(&id) && !inner.handles.contains_key(&id) {
                return;
            }
            inner.pending_joins.insert(id, joined);
            inner.join_waiters.remove(&id).unwrap_or_default()
        };
        for sender in senders {
            let _ = sender.send(joined);
        }
    }

    /// Take a stashed Joined for a joining connection. Non-removing: the
    /// stashed row doubles as the authoritative entity id for later viewer
    /// construction, so joins stay correct after the first take.
    pub fn take_join(&self, id: ConnectionId) -> Option<JoinedSpawn> {
        self.inner.lock().pending_joins.get(&id).copied()
    }

    /// Pump side of the join race: the handles of a registered connection,
    /// or else stash the drained `changes` for replay at
    /// [`register`][Self::register], atomically with the handle check. Their
    /// `Joined` is published for [`wait_join`][Self::wait_join] only after
    /// that, and the connection registers only after its join wait, so the
    /// stash is always in place before registration takes it.
    pub fn handles_or_stash(
        &self,
        id: ConnectionId,
        changes: &mut Vec<mc_core::boundary::PlayerChange>,
    ) -> Option<ConnectionHandles> {
        let joined = changes.iter().find_map(|change| match change {
            mc_core::boundary::PlayerChange::Joined {
                entity,
                position,
                game_mode,
                dimension,
                ..
            } => Some((*entity, *position, *game_mode, *dimension)),
            _ => None,
        });
        {
            let mut inner = self.inner.lock();
            if let Some(handles) = inner.handles.get(&id) {
                return Some(handles.clone());
            }
            if !inner.joining.contains(&id) {
                changes.clear();
                return None;
            }
            inner
                .pending_reliable
                .entry(id)
                .or_default()
                .append(changes);
        }
        if let Some(joined) = joined {
            self.stash_join(id, joined);
        }
        None
    }

    /// Stash a core-ordered disconnect reason for a connection. The play
    /// loop takes it after flushing the kick packet and logs peer+reason.
    pub fn stash_disconnect_reason(&self, id: ConnectionId, reason: String) {
        let mut inner = self.inner.lock();
        if inner.joining.contains(&id) || inner.handles.contains_key(&id) {
            inner.disconnect_reasons.insert(id, reason);
        }
    }

    /// Take a stashed core-ordered disconnect reason, if the pump routed one.
    pub fn take_disconnect_reason(&self, id: ConnectionId) -> Option<String> {
        self.inner.lock().disconnect_reasons.remove(&id)
    }

    /// Register interest in this connection's Joined. The pump fires the
    /// receiver when it routes the Joined (via [`stash_join`][Self::stash_join]).
    /// Callers must check [`take_join`][Self::take_join] first (fast path
    /// when the Joined already arrived), then await the receiver.
    pub fn wait_join(&self, id: ConnectionId) -> tokio::sync::oneshot::Receiver<JoinedSpawn> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let mut inner = self.inner.lock();
        if let Some(joined) = inner.pending_joins.get(&id) {
            let _ = sender.send(*joined);
        } else if inner.joining.contains(&id) {
            inner.join_waiters.entry(id).or_default().push(sender);
        }
        receiver
    }

    /// Stop delivery and wake the connection owner, including a blocked writer.
    /// Admission is released only by the lifecycle owner's final cleanup.
    /// Drops queue senders, the
    /// stashed join, stashed pre-registration reliable changes, any join
    /// waiters (their receivers fail closed), and the stashed disconnect
    /// reason.
    pub fn remove(&self, id: ConnectionId) {
        crate::vanilla_items::unmark(id);
        let closer = {
            let mut inner = self.inner.lock();
            inner.joining.remove(&id);
            inner.handles.remove(&id);
            if inner.names.remove(&id).is_some() {
                inner.roster_dirty = true;
            }
            inner.pending_joins.remove(&id);
            inner.pending_reliable.remove(&id);
            inner.join_waiters.remove(&id);
            inner.disconnect_reasons.remove(&id);
            inner.view_distances.remove(&id);
            inner.locales.remove(&id);
            inner.closers.remove(&id)
        };
        if let Some(closer) = closer {
            let _ = closer.send(());
        }
    }

    fn finish(&self, id: ConnectionId) {
        self.remove(id);
        self.inner.lock().admitted.remove(&id);
    }

    /// Look up one connection's handles.
    pub fn get(&self, id: ConnectionId) -> Option<ConnectionHandles> {
        self.inner.lock().handles.get(&id).cloned()
    }

    /// Snapshot all handles for pump routing.
    pub fn snapshot(&self) -> Vec<(ConnectionId, ConnectionHandles)> {
        self.inner
            .lock()
            .handles
            .iter()
            .map(|(id, handles)| (*id, handles.clone()))
            .collect()
    }

    // One handle copy per pump iteration; roster work only follows a change.
    fn routing_snapshot(
        &self,
    ) -> (
        HashMap<ConnectionId, ConnectionHandles>,
        Option<Vec<String>>,
    ) {
        let (handles, names) = {
            let mut inner = self.inner.lock();
            let names = if std::mem::take(&mut inner.roster_dirty) {
                Some(inner.names.values().cloned().collect::<Vec<_>>())
            } else {
                None
            };
            (inner.handles.clone(), names)
        };
        let names = names.map(|mut names| {
            names.sort();
            names
        });
        (handles, names)
    }

    /// Set a player name for the dashboard roster.
    pub fn set_name(&self, id: ConnectionId, name: String) {
        let mut inner = self.inner.lock();
        if inner.names.get(&id) != Some(&name) {
            inner.names.insert(id, name);
            inner.roster_dirty = true;
        }
    }

    /// Sorted roster names for the dashboard.
    pub fn roster(&self) -> Vec<String> {
        let mut names: Vec<_> = self.inner.lock().names.values().cloned().collect();
        names.sort();
        names
    }

    /// Current connection count.
    pub fn len(&self) -> usize {
        self.inner.lock().handles.len()
    }

    /// Whether no connections are registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for ConnectionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Latest published tick, written by the pump and read by status paths.
/// Dashboard TPS/percentiles come from its own windows (`dashboard.rs`).
pub struct TickStats {
    tick: AtomicU64,
}

impl TickStats {
    /// Empty stats.
    pub fn new() -> Self {
        Self {
            tick: AtomicU64::new(0),
        }
    }

    /// Record a tick arrival.
    pub fn record(&self, tick: u64) {
        self.tick.fetch_max(tick, Ordering::Relaxed);
    }

    /// Latest tick number observed.
    pub fn tick(&self) -> u64 {
        self.tick.load(Ordering::Relaxed)
    }
}

impl Default for TickStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Process-wide network counters (bytes written, reliable drops).
pub struct NetCounters {
    /// Total bytes written to Play connections.
    pub bytes_written: AtomicU64,
    /// Reliable-overflow disconnects.
    pub reliable_drops: AtomicU64,
}

impl NetCounters {
    /// Zero counters.
    pub fn new() -> Self {
        Self {
            bytes_written: AtomicU64::new(0),
            reliable_drops: AtomicU64::new(0),
        }
    }
}

impl Default for NetCounters {
    fn default() -> Self {
        Self::new()
    }
}

/// Accept loop: serves `listener` until it closes. Each connection runs on
/// its own task: handshake -> status | login -> configuration -> join ->
/// burst -> play loop. A single pump task fans every tick out to viewers.
pub async fn serve(listener: TcpListener, server: EdgeServer) -> anyhow::Result<()> {
    serve_with_console(listener, server, false).await
}

/// Serve with optional stdin command admission. Library/harness callers keep
/// their existing console-free `serve` entrypoint.
pub async fn serve_with_console(
    listener: TcpListener,
    server: EdgeServer,
    console: bool,
) -> anyhow::Result<()> {
    let server = Arc::new(server);
    let _remote_admin = crate::remote_admin::start(Arc::clone(&server)).await?;
    let mut pump = spawn_pump(Arc::clone(&server))?;
    server.console_state.running.store(true, Ordering::Relaxed);
    let load_metrics = spawn_load_metrics(&server);
    let mut stop = server.stop.subscribe();
    let addresses = std::sync::Arc::new(crate::conn_limit::AddressLimiter::new(
        server.cfg.network.connection_throttle_ms,
        server.cfg.network.max_connections_per_ip,
    ));
    let mut tasks = tokio::task::JoinSet::new();
    let console = if console {
        Some(crate::console::spawn(Arc::clone(&server))?)
    } else {
        None
    };
    let msmp = if server.cfg.msmp.enabled && msmp_allowed(&server.cfg) {
        Some(tokio::spawn(crate::msmp::serve(
            server.cfg.msmp.bind_address.clone(),
            server.cfg.msmp.port,
            server.cfg.msmp.bearer_token.clone(),
            Arc::clone(&server),
        )))
    } else {
        if server.cfg.msmp.enabled {
            tracing::warn!(
                addr = %server.cfg.msmp.bind_address,
                "MSMP enabled with non-loopback bind but allow_remote=false; MSMP not started"
            );
        }
        None
    };
    // Set when accept hit descriptor/buffer exhaustion: retried only after one of
    // our own sessions ends and releases its socket (no timer).
    let mut accept_blocked = false;
    loop {
        tokio::select! {
            _ = async { let _ = stop.wait_for(|stopping| *stopping).await; } => {
                for (id, _) in server.connections.snapshot() {
                    if let Err(error) = server.kick(id, serde_json::json!({"translate":"multiplayer.disconnect.server_shutdown"})) {
                        warn!(%error, connection = id.0, "shutdown kick delivery failed");
                    }
                }
                while !tasks.is_empty() {
                    if tokio::time::timeout(std::time::Duration::from_secs(10), tasks.join_next()).await.is_err() {
                        tasks.abort_all();
                        break;
                    }
                }
                let hub = Arc::clone(&server.hub);
                tokio::task::spawn_blocking(move || hub.shutdown()).await?;
                let (mut connections, _) = server.connections.routing_snapshot();
                route_outbox(&server, &mut HashMap::new(), &mut connections);
                if let Some(console) = &console {
                    console.abort();
                }
                pump.abort();
                load_metrics.abort();
                if let Some(msmp) = &msmp {
                    msmp.abort();
                }
                return Ok(());
            }
            _ = tasks.join_next(), if !tasks.is_empty() => accept_blocked = false,
            accepted = listener.accept(), if !accept_blocked => {
                let (stream, peer) = match accepted {
                    Ok(accepted) => accepted,
                    Err(error) => match accept_failure(&error, tasks.len()) {
                        AcceptFailure::Transient => {
                            debug!(%error, "transient accept failure");
                            continue;
                        }
                        AcceptFailure::WaitForSession => {
                            warn!(%error, sessions = tasks.len(), "accept exhausted resources; admission paused until a session ends");
                            accept_blocked = true;
                            continue;
                        }
                        AcceptFailure::Fatal => {
                            return Err(anyhow::anyhow!(error).context("listener accept failed"));
                        }
                    },
                };
                // Refused before the handshake: the throttle and the per-address
                // cap answer like an absent listener instead of buffering work.
                let address = Some(peer.ip());
                if let Some(address) = address
                    && !addresses.admit(address)
                {
                    debug!(%peer, "connection refused by the per-address limit");
                    drop(stream);
                    continue;
                }
                debug!(%peer, "accepted connection");
                let server = Arc::clone(&server);
                let addresses = Arc::clone(&addresses);
                tasks.spawn(async move {
                    if let Err(error) = handle_connection(stream, server).await {
                        debug!(%error, "connection closed");
                    }
                    if let Some(address) = address {
                        addresses.release(address);
                    }
                });
            }
            // Without its pump the server never delivers another tick.
            ended = &mut pump => match ended {
                Ok(()) => {
                    // Let each connection flush its routed terminal
                    // Disconnect before the process exits (bounded).
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                        while tasks.join_next().await.is_some() {}
                    })
                    .await;
                    anyhow::bail!("edge pump stopped: core broadcast closed")
                }
                Err(error) => anyhow::bail!("edge pump died: {error}"),
            },
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum AcceptFailure {
    /// One pending connection failed; the listener is fine.
    Transient,
    /// Descriptor/buffer exhaustion while our own sessions hold sockets: wait
    /// for one to end instead of failing the whole server.
    WaitForSession,
    /// Nothing of ours can free the resource, or the listener itself broke.
    Fatal,
}

fn accept_failure(error: &std::io::Error, open_sessions: usize) -> AcceptFailure {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::ConnectionAborted | ErrorKind::ConnectionReset | ErrorKind::Interrupted => {
            return AcceptFailure::Transient;
        }
        _ => {}
    }
    // EMFILE, ENFILE, ENOBUFS, ENOMEM.
    let exhausted = matches!(error.raw_os_error(), Some(24 | 23 | 105 | 12));
    if exhausted && open_sessions > 0 {
        AcceptFailure::WaitForSession
    } else {
        AcceptFailure::Fatal
    }
}

/// Per-connection pipeline. Status connections return after the ping;
/// Play connections run until disconnect or overflow.
async fn handle_connection(stream: TcpStream, server: Arc<EdgeServer>) -> anyhow::Result<()> {
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "<unknown>".to_owned());
    let id = server.connections.alloc_id();
    let mut lifecycle = ConnectionLifecycle::new(&server.connections, &server.admin, id);
    let cancelled = lifecycle.cancelled.take().expect("one connection owner");
    let result = tokio::select! {
        biased;
        _ = cancelled => {
            warn!(%peer, connection = id.0, "connection closed by delivery policy");
            Err(anyhow::anyhow!("connection delivery stopped"))
        }
        result = run_connection(stream, Arc::clone(&server), id, peer.clone(), &mut lifecycle) => result,
    };
    lifecycle.stop_writer().await;
    result
}

async fn run_connection(
    stream: TcpStream,
    server: Arc<EdgeServer>,
    id: ConnectionId,
    peer: String,
    lifecycle: &mut ConnectionLifecycle<'_>,
) -> anyhow::Result<()> {
    let mut conn = crate::connection::Connection::new(stream);
    let pre_play = async {
        // --- Handshake: route to status or login. Every `?` below logs the
        // --- peer + stage + reason first: a silent TCP close is undebuggable.
        let Some(handshake) = (match crate::handshake::read_handshake(
            &mut conn,
            server.cfg.server.accepts_transfers,
        )
        .await
        {
            Ok(next) => next,
            Err(error) => {
                warn!(%peer, connection = id.0, stage = "handshake", %error, "connection closed");
                return Err(error);
            }
        }) else {
            warn!(%peer, connection = id.0, stage = "handshake", "handshake refused (disconnect packet sent); connection closed");
            return Ok(None);
        };
        let next_state = handshake.next;
        if next_state == crate::handshake::NextState::Status {
            let motd = server.cfg.server.motd.clone();
            let max = server.cfg.server.max_players;
            let online = server.connections.len();
            if let Err(error) = crate::status::handle_status(&mut conn, &motd, max, online).await {
                warn!(%peer, connection = id.0, stage = "status", %error, "connection closed");
                return Err(error);
            }
            return Ok(None);
        }

        // --- Login (offline, or online verification when configured). ---
        let peer_ip = stream_peer_ip(&peer);
        let outcome = match crate::login::handle_login_admitted(
            &mut conn,
            &crate::login::LoginOpts {
                online_mode: server.cfg.auth.online_mode,
                compression_threshold: crate::login::DEFAULT_COMPRESSION_THRESHOLD,
                velocity: server.velocity.clone(),
            },
            server.online_authentication.as_ref(),
            peer_ip,
            |uuid, ip| {
                if *server.stop.borrow() {
                    return Some(
                        serde_json::json!({"translate":"multiplayer.disconnect.server_shutdown"}),
                    );
                }
                server
                    .connections
                    .admit(id, server.cfg.server.max_players as usize, |full| {
                        server.admin.admission(uuid, ip, full)
                    })
            },
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                warn!(%peer, connection = id.0, stage = "login", %error, "connection closed");
                return Err(error);
            }
        };
        let Some(outcome) = outcome else {
            warn!(%peer, connection = id.0, stage = "login", "login rejected (disconnect packet sent); connection closed");
            return Ok(None);
        };

        // --- Configuration (registries, tags, finish). ---
        let config_outcome = match crate::configuration::handle_configuration(
            &mut conn,
            &server.data,
            &server.tags,
            server.loader_manifest.as_ref(),
            server.loader_required,
            &server.cfg.resource_pack,
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                warn!(%peer, connection = id.0, stage = "configuration", name = %outcome.name, %error, "connection closed");
                return Err(error);
            }
        };
        Ok(Some((outcome, config_outcome)))
    };
    let Some((outcome, config_outcome)) =
        tokio::time::timeout(crate::connection::PRE_PLAY_TOTAL_TIMEOUT, pre_play)
            .await
            .map_err(|_| anyhow::anyhow!("total pre-play deadline exceeded"))??
    else {
        return Ok(());
    };
    // Access-list changes during Configuration revoke admission before Join.
    if let Some(reason) = server.admin.admission(
        outcome.uuid,
        outcome.remote_address.or(stream_peer_ip(&peer)),
        false,
    ) {
        anyhow::bail!("admission revoked during Configuration: {reason}");
    }

    // The radius and locale precede Join on the reliable queue: the first
    // residency reconciliation never expands this client to the server-wide
    // maximum, and a dialog opened right after join already reads the client
    // language.
    let view_distance = effective_view_distance(
        config_outcome.client_view_distance,
        server.cfg.server.view_distance,
    );
    lifecycle.native = Some(
        server.hub.join_session(
            id,
            outcome.uuid,
            outcome.name.clone(),
            server.admin.level(outcome.uuid),
            view_distance as u8,
            config_outcome.client_language.clone(),
        ).inspect_err(|error| {
            warn!(%peer, connection = id.0, stage = "join-submit", name = %outcome.name, %error, "native session refused; connection closed");
        })?
    );

    // --- Wait for our Joined (bounded): the tick loop answers with the
    // --- entity id + spawn the burst needs.
    let (entity, spawn, game_mode, dimension) = match wait_for_join(
        &server,
        id,
        &outcome.name,
        config_outcome.client_view_distance,
        &config_outcome.client_language,
    )
    .await
    {
        Ok(joined) => joined,
        Err(error) => {
            warn!(%peer, connection = id.0, stage = "join-wait", name = %outcome.name, %error, "connection closed");
            return Err(error);
        }
    };

    if config_outcome.loader_session.is_none() {
        // A client without the Loader cannot resolve plugin item models: it sees
        // the carrier item instead of the vanilla missing-model cube.
        crate::vanilla_items::mark(id);
        conn.show_vanilla_item_models();
    }

    if config_outcome.loader_session.is_some() {
        // The input follows Join on the same reliable authority queue. It is
        // never inferred from declared bundles or a client-provided brand.
        let ready = PlayerInput::PluginClientReady { connection: id };
        anyhow::ensure!(
            server.hub.submit(ready),
            "authoritative Loader session admission queue is full or stopped"
        );
    }

    // --- Join burst (compressed from here on). ---
    let simulation_distance = i32::from(server.cfg.server.simulation_distance);
    let latest = latest_output(&server);
    if let Err(error) = crate::play::send_join_burst(crate::play::JoinBurst {
        conn: &mut conn,
        data: &server.data,
        out: &latest,
        viewer_entity: entity,
        dimension,
        pos: spawn,
        view_distance,
        simulation_distance,
        game_mode,
        recipes: &server.recipes,
        op_level: if server.hub.session_locked(id) {
            0
        } else {
            server.admin.level(outcome.uuid)
        },
        plugin_commands: server.hub.plugin_commands(),
    })
    .await
    {
        warn!(%peer, connection = id.0, stage = "join-burst", name = %outcome.name, %error, "connection closed");
        return Err(error);
    }

    // --- Register outbound queues; then replay the stashed pre-join burst:
    // --- the pump may have drained the Join tick before registration, in
    // --- which case `route_outbox` stashed the whole ordered reliable list
    // --- (Joined + menus/health/experience/...) for this connection.
    // --- Replay it in order through the same `plan_player_changes` mapping
    // --- so a rejoining player gets the authoritative inventory/XP sync.
    let (reliable_tx, reliable_rx) =
        mpsc::channel::<bytes::Bytes>(crate::fanout::OUTBOUND_QUEUE_BOUND);
    let (world_tx, world_rx) = mpsc::channel::<bytes::Bytes>(crate::fanout::OUTBOUND_QUEUE_BOUND);
    let stashed = server.connections.register(
        id,
        ConnectionHandles {
            reliable_tx,
            world_tx,
        },
    )?;
    if !stashed.is_empty() {
        let handles = server
            .connections
            .get(id)
            .expect("queues inserted above for this connection");
        // Adopt the Joined entity id if stashed (matches the pump path).
        let joined_entity = stashed.iter().find_map(|change| match change {
            mc_core::boundary::PlayerChange::Joined { entity, .. } => Some(*entity),
            _ => None,
        });
        let mut replay_viewer = configured_viewer(
            &server,
            id,
            joined_entity.unwrap_or(entity),
            spawn,
            dimension,
        );
        // Skip the stale `Joined` sync itself: the join burst already sent
        // teleport id 1 + empty inventory/health at this position, and the
        // `Joined` arm would re-emit all four (a second teleport + a reset
        // to empty). Replay only the authoritative tail after it.
        let tail: Vec<mc_core::boundary::PlayerChange> = stashed
            .into_iter()
            .skip_while(|change| !matches!(change, mc_core::boundary::PlayerChange::Joined { .. }))
            .skip(1)
            .collect();
        for planned in crate::plan::plan_player_changes(&tail, &mut replay_viewer) {
            // case disconnect loudly instead of sending a partial burst.
            if handles.reliable_tx.try_send(planned.data).is_err() {
                warn!(%peer, connection = id.0, stage = "join-replay", name = %outcome.name, "stashed join burst overflows outbound queue; connection closed");
                return Ok(());
            }
        }
    }
    server.connections.set_name(id, outcome.name.clone());
    if let Some(ip) = outcome.remote_address.or(stream_peer_ip(&peer)) {
        server
            .admin
            .register(id, outcome.uuid, outcome.name.clone(), ip);
    }
    server.feedback(id, "Поселения: /settlement guide")?;
    if *server.stop.borrow() {
        server.kick(
            id,
            serde_json::json!({"translate":"multiplayer.disconnect.server_shutdown"}),
        )?;
    }
    // Far-terrain LOD: only the negotiated native client is told about it and
    // may ask for tiles (`[lod]`).
    let native_client = config_outcome
        .loader_session
        .as_ref()
        .is_some_and(|session| session.platform() == mc_loader::LoaderPlatform::Native);
    let mut lod =
        (server.lod.is_some() && native_client).then(|| crate::lod::LodConnection::new(spawn));
    if lod.is_some()
        && let Some(handles) = server.connections.get(id)
    {
        for frame in crate::lod::hello_frames(&server)? {
            let _ = handles.reliable_tx.try_send(frame);
        }
    }
    let (dh_tx, dh_rx) = mpsc::channel(128);
    let dh =
        server.dh.as_ref().filter(|_| !native_client).map(|_| {
            crate::dh::Connection::start(Arc::clone(&server), id, entity, dimension, dh_tx)
        });
    if dh.is_some()
        && let Some(handles) = server.connections.get(id)
    {
        let _ = handles.reliable_tx.try_send(crate::dh::advertisement()?);
    }
    let computer = if server.cfg.computer.enabled {
        Some(crate::computer::Connection::start(
            Arc::clone(&server),
            id,
            entity,
            peer.parse()?,
        ))
    } else {
        None
    };

    // --- Play: reads and writes progress independently. A writer task owns
    // --- the write half (keepalive, then reliable before world); this task
    // --- reads inputs, so a slow socket write never delays the client's
    // --- inputs/acks. Queue discipline: a full reliable queue disconnects
    // --- at route time; world packets are planned within free slots.
    let mut decode_state = crate::decode::DecodeState::default();
    let mut inputs = Vec::new();
    let mut sheds = 0u32;
    let mut packet_limiter = PacketLimiter::new(&server.cfg.anti_cheat);
    let (mut reader, writer) = conn.into_play_halves();
    let (keepalive_tx, keepalive_rx) = mpsc::channel(1);
    lifecycle.writer = Some(tokio::spawn(play_writer(
        writer,
        Arc::clone(&server.net_counters),
        Arc::clone(&server.connections),
        id,
        PlayReceivers {
            reliable: reliable_rx,
            world: world_rx,
            dh: dh_rx,
            keepalive: keepalive_rx,
        },
    )));
    let writer_task = lifecycle.writer.as_mut().expect("one writer owner");
    let mut writer_finished = false;
    let exit_stage: &'static str = 'play_loop: loop {
        tokio::select! {
            ended = &mut *writer_task => {
                writer_finished = true;
                match ended {
                    Ok((stage, Some(reason))) => {
                        warn!(%peer, connection = id.0, stage = "play", name = %outcome.name, reason = %reason, "core-ordered disconnect flushed; connection closed");
                        break 'play_loop stage;
                    }
                    Ok((stage, None)) => break 'play_loop stage,
                    Err(_) => break 'play_loop "writer-panicked",
                }
            }
            // No read deadline in Play: client silence is governed by
            // keepalive, never by the pre-play 10 s timeout.
            frame = reader.read_frame() => {
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(error) => {
                        warn!(%peer, connection = id.0, stage = "play-read", name = %outcome.name, %error, "connection closed");
                        break 'play_loop "read-frame";
                    }
                };
                if !packet_limiter.allow(frame.id) {
                    warn!(%peer, connection = id.0, stage = "play-rate-limit", name = %outcome.name, packet = frame.id, "play packet rate limit exceeded");
                    if let Err(error) = server.kick(id, serde_json::json!({"text":"Too many packets of this type"})) {
                        warn!(%peer, connection = id.0, %error, "rate-limit kick undeliverable; connection closed");
                        break 'play_loop "rate-limit-kick";
                    }
                    continue;
                }
                if frame.id == mc_protocol::packets::play::ServerboundKeepAlive::ID {
                    let mut body = &frame.body[..];
                    match mc_protocol::packets::play::ServerboundKeepAlive::decode(&mut body) {
                        Ok(reply) if body.is_empty() => {
                            if keepalive_tx.try_send(reply.id).is_err() {
                                break 'play_loop "keepalive-reply-overflow";
                            }
                        }
                        _ => break 'play_loop "keepalive-malformed",
                    }
                    continue;
                }
                match server.handle_edge_frame(id, frame.id, &frame.body).await {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(error) => {
                        warn!(%error, connection = id.0, "edge command decode/delivery failed");
                        break 'play_loop "edge-command-error";
                    }
                }
                if let Some(computer) = &computer
                    && frame.id == mc_protocol::packets::play::ServerboundCustomPayload::ID
                {
                    match crate::computer::payload(&frame.body) {
                        Ok(Some(payload)) => {
                            if let Err(error) = computer.receive(&payload) {
                                warn!(%error, connection = id.0, "computer input refused");
                                break 'play_loop "computer-input";
                            }
                            continue;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            warn!(%error, connection = id.0, "computer packet malformed");
                            break 'play_loop "computer-decode";
                        }
                    }
                }
                if let Some(dh) = &dh
                    && frame.id == mc_protocol::packets::play::ServerboundCustomPayload::ID
                    && let Some(payload) = crate::dh::payload(&frame.body)
                {
                    dh.receive(payload);
                    continue;
                }
                if let Some(lod) = &mut lod
                    && frame.id == mc_protocol::packets::play::ServerboundCustomPayload::ID
                    && let Some(payload) = crate::lod::request_payload(&frame.body)
                {
                    if server.hub.session_locked(id) {
                        continue;
                    }
                    // Native LOD currently serves the root world only. Its
                    // radius follows committed state, never a rejected Move.
                    let latest = latest_output(&server);
                    let Some(player) = latest.snapshot.entities.get(entity) else {
                        continue;
                    };
                    lod.moved(player.position);
                    if let Err(error) = lod.request(&server, id, &payload) {
                        warn!(%peer, connection = id.0, stage = "lod-request", name = %outcome.name, %error, "connection closed");
                        break 'play_loop "lod-request-decode";
                    }
                    continue;
                }
                // A Loader view request is admitted only from the connection
                // that completed Configuration negotiation. It bypasses the
                // generic decoder so the same custom payload is never decoded
                // twice, while other Loader action-channel messages remain
                // intentionally inert until their native authorities land.
                let handled_loader = if config_outcome.loader_session.is_some()
                    && frame.id == mc_protocol::packets::play::ServerboundCustomPayload::ID
                {
                    match loader_view_ingress(
                        server
                            .loader_manifest
                            .as_ref()
                            .expect("negotiated Loader session retains its manifest"),
                        id,
                        &frame.body,
                    ) {
                        Ok(LoaderViewIngress::NotLoader) => false,
                        Ok(LoaderViewIngress::Discard) => true,
                        Ok(LoaderViewIngress::Request(input)) => {
                            inputs.push(input);
                            true
                        }
                        Err(error) => {
                            warn!(%peer, connection = id.0, stage = "loader-view-decode", name = %outcome.name, %error, "connection closed");
                            break 'play_loop "loader-view-decode";
                        }
                    }
                } else {
                    false
                };
                if !handled_loader
                    && let Err(reason) = crate::decode::decode_play_frame(
                        id,
                        frame.id,
                        &frame.body,
                        &mut decode_state,
                        &mut inputs,
                    )
                {
                    warn!(%peer, connection = id.0, stage = "play-decode", name = %outcome.name, frame = frame.id, reason = %reason, "connection closed");
                    break 'play_loop "decode-error";
                }
                if inputs.iter().any(|input| matches!(input, PlayerInput::Respawn { .. }))
                    && let Some(dh) = &dh
                {
                    dh.respawn();
                }
                for mut input in inputs.drain(..) {
                    // Mutes block communication, not unrelated command rights.
                    if input.is_communication()
                        && !server.hub.session_locked(id)
                        && server
                            .admin
                            .uuid_of(id)
                            .is_some_and(|uuid| server.admin.muted(uuid))
                    {
                        if let Err(error) = server.feedback(id, "You are muted.") {
                            warn!(%peer, connection = id.0, %error, "mute feedback undeliverable; connection closed");
                            break 'play_loop "mute-feedback";
                        }
                        continue;
                    }
                    if let PlayerInput::Command { command, permission_level, .. } = &mut input {
                        // A locked session keeps no edge-cached permission: no
                        // username or op-list state may escalate it. The fence
                        // refuses admin roots and the native gate admits only
                        // login/register roots at level 0.
                        *permission_level = if server.hub.session_locked(id) {
                            0
                        } else {
                            server.admin.session_level(id)
                        };
                        match server.execute_admin_command(id, command, *permission_level).await {
                            Ok(true) => continue,
                            Ok(false) => {}
                            Err(error) => {
                                warn!(%error, connection = id.0, "administrative command failed");
                                break 'play_loop "admin-command-error";
                            }
                        }
                        if !crate::command_tree::permitted(
                            command,
                            *permission_level,
                            server.hub.plugin_commands(),
                        ) {
                            if server.feedback(id, "Unknown or incomplete command").is_err() {
                                break 'play_loop "command-feedback-overflow";
                            }
                            continue;
                        }
                    }
                    let submitted = match server.hub.admit_build(input).await {
                        Ok(input) => server.hub.submit(input),
                        Err(refusal) => server.hub.resync_build(refusal),
                    };
                    if submitted {
                        sheds = 0;
                    } else {
                        sheds += 1;
                        if sheds >= MAX_SUBMIT_SHEDS {
                            warn!(%peer, connection = id.0, stage = "play-submit", name = %outcome.name, "hub queue full; connection closed");
                            break 'play_loop "submit-shed";
                        }
                    }
                }
            }
        }
    };
    if !writer_finished {
        // Wait for the cancelled writer to drop its socket half, so the
        // session's end (which re-opens accept after fd exhaustion) really
        // means the socket is released.
        writer_task.abort();
        let _ = writer_task.await;
    }
    lifecycle.writer.take();
    warn!(%peer, connection = id.0, stage = "play", exit = exit_stage, name = %outcome.name, "connection closed");

    Ok(())
}

struct PlayReceivers {
    reliable: mpsc::Receiver<bytes::Bytes>,
    world: mpsc::Receiver<bytes::Bytes>,
    dh: mpsc::Receiver<crate::dh::Outbound>,
    keepalive: mpsc::Receiver<i64>,
}

/// Play writer task: keepalive first, then reliable before world, so a
/// reliable frame never waits behind a world backlog. Frames already queued
/// go out together in one write (one syscall instead of one per frame),
/// taken in the same priority order and bounded by [`WRITE_BATCH_BYTES`].
/// Returns the exit stage and, for a core-ordered kick, its reason.
async fn play_writer(
    mut writer: crate::connection::PlayWriter,
    net_counters: Arc<NetCounters>,
    connections: Arc<ConnectionRegistry>,
    id: ConnectionId,
    receivers: PlayReceivers,
) -> (&'static str, Option<String>) {
    let PlayReceivers {
        reliable: mut reliable_rx,
        world: mut world_rx,
        dh: mut dh_rx,
        keepalive: mut keepalive_rx,
    } = receivers;
    // Vanilla Play KeepAlive: the server pings every 15 s and the client
    // kicks after ~30 s of silence. Without this a parked-but-live
    // connection looks dead to vanilla even while the pump is healthy.
    let mut keepalive = tokio::time::interval(std::time::Duration::from_secs(15));
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Skip the immediate first tick: the join burst just spoke.
    keepalive.reset();
    let mut challenge = 0_i64;
    let mut pending = None;
    let mut reply_deadline = tokio::time::Instant::now();
    let mut batch: Vec<u8> = Vec::with_capacity(WRITE_BATCH_BYTES);
    loop {
        let mut reliable = false;
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(reply_deadline), if pending.is_some() => {
                return ("keepalive-timeout", None);
            }
            reply = keepalive_rx.recv(), if !keepalive_rx.is_closed() => {
                if reply.is_none() || reply != pending {
                    return ("keepalive-invalid-reply", None);
                }
                pending = None;
                continue;
            }
            _ = keepalive.tick() => {
                if pending.is_some() {
                    continue;
                }
                challenge = challenge.wrapping_add(1);
                pending = Some(challenge);
                reply_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
                let packet = mc_protocol::packets::play::ClientboundKeepAlive {
                    id: challenge,
                };
                if let Err(error) = writer.send_packet(&packet).await {
                    warn!(%error, connection = id.0, "keepalive write failed");
                    return ("keepalive-write-failed", None);
                }
                continue;
            }
            msg = reliable_rx.recv() => {
                let Some(bytes) = msg else { return ("pump-closed", None) };
                batch.extend_from_slice(&bytes);
                reliable = true;
            }
            msg = world_rx.recv() => {
                let Some(bytes) = msg else { return ("pump-closed", None) };
                batch.extend_from_slice(&bytes);
            }
            msg = dh_rx.recv(), if !dh_rx.is_closed() || !dh_rx.is_empty() => {
                if let Some(message) = msg {
                    if !message.valid() { continue; }
                    // Never batch DH ahead of queued ordinary gameplay.
                    if let Err(error) = writer.write_bytes(&message.bytes).await {
                        warn!(%error, connection = id.0, "dh write failed");
                        return ("dh-write-failed", None);
                    }
                    net_counters.bytes_written.fetch_add(message.bytes.len() as u64, Ordering::Relaxed);
                }
                continue;
            }
        }
        while batch.len() < WRITE_BATCH_BYTES {
            if let Ok(bytes) = reliable_rx.try_recv() {
                batch.extend_from_slice(&bytes);
                reliable = true;
            } else if let Ok(bytes) = world_rx.try_recv() {
                batch.extend_from_slice(&bytes);
            } else {
                break;
            }
        }
        if let Err(error) = writer.write_bytes(&batch).await {
            warn!(%error, connection = id.0, reliable, bytes = batch.len(), "play write failed");
            return (
                if reliable {
                    "reliable-write-failed"
                } else {
                    "world-write-failed"
                },
                None,
            );
        }
        net_counters
            .bytes_written
            .fetch_add(batch.len() as u64, Ordering::Relaxed);
        batch.clear();
        // Core-ordered kick: Disconnect is always the last change, so once
        // the reliable queue is empty the kick packet has been flushed.
        if reliable
            && reliable_rx.is_empty()
            && let Some(reason) = connections.take_disconnect_reason(id)
        {
            return ("core-disconnect", Some(reason));
        }
    }
}

/// Bytes of queued frames joined into one socket write.
const WRITE_BATCH_BYTES: usize = 64 * 1024;

/// Independent, cheap telemetry consumer: world fanout may lag without losing
/// timing samples. Its bounded broadcast still reports any actual loss.
fn spawn_load_metrics(server: &Arc<EdgeServer>) -> tokio::task::JoinHandle<()> {
    let mut rx = server.hub.subscribe_tick_times();
    let dashboard = Arc::clone(&server.dashboard);
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok((tick, times)) => dashboard.record_load_tick(tick, times),
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    dashboard.record_load_skipped(skipped);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

/// Single pump task: subscribes to the core, records tick stats, drains the
/// reliable outbox FIRST, then publishes the world snapshot and routes both.
/// `Lagged` = world-skip (viewers reconcile on the next output) but the outbox is still
/// drained: reliable results are never skipped. Fanout runs on its own
/// rayon pool: on the global pool the core tick's parallel phases queued
/// behind whole publishes.
fn spawn_pump(server: Arc<EdgeServer>) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let mut rx = server.hub.subscribe();
    let pool = rayon::ThreadPoolBuilder::new()
        .thread_name(|i| format!("edge-fanout-{i}"))
        .build()?;
    // Hide enclosed ores in encoded chunks per `[anti_xray]` (built from the
    // core's exact light/opacity oracle, shared by every viewer via the cache).
    let anti_xray = crate::anti_xray::AntiXray::new(
        &server.wire.blocks,
        &server.hub.registry().light,
        &server.cfg.anti_xray,
    )?
    .map(Arc::new);
    Ok(tokio::spawn(async move {
        // Viewer world-view state keyed by connection: what each client
        // holds, reconciled against every newest snapshot.
        let mut viewers: HashMap<ConnectionId, crate::plan::ViewerState> = HashMap::new();
        // Encoded chunk packets shared across viewers: light compute costs
        // ~28 ms/chunk, so a full 81-chunk re-plan (2.3 s) always outruns
        // the 32-tick broadcast window (1.6 s) and the pump would lag,
        // resync, and recompute forever without ever routing. The cache
        // makes re-plans O(1) per chunk; block changes evict their chunk.
        let mut chunk_cache = crate::plan::ChunkPacketCache::with_anti_xray(anti_xray);
        loop {
            let out = match rx.recv().await {
                Ok(out) => out,
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    // World-skip, not data loss: the next received output
                    // is reconciled against each viewer's held state. The
                    // reliable outbox is drained regardless (never
                    // skipped). A lag is never fatal: the next `recv`
                    // either returns the newest output or reports a further
                    // lag handled here again.
                    server.dashboard.record_pump_skipped(skipped);
                    for viewer in viewers.values_mut() {
                        viewer.skipped = true;
                    }
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    // The core stopped itself (journal or plugin failure,
                    // fail closed). Its terminal reliable `Disconnect{reason}`
                    // for every connection is still queued in the outbox:
                    // route it so clients and the log see why, instead of
                    // a bare socket close.
                    let (mut connections, _) = server.connections.routing_snapshot();
                    route_outbox(&server, &mut viewers, &mut connections);
                    tracing::error!(
                        connections = connections.len(),
                        "core tick broadcast closed; core halted or stopped"
                    );
                    break;
                }
            };
            server.tick_stats.record(out.tick);
            let (mut connections, roster) = server.connections.routing_snapshot();
            route_outbox(&server, &mut viewers, &mut connections);
            server.console_state.drained(out.tick);
            let publish_start = std::time::Instant::now();
            publish_and_route(
                &server,
                &pool,
                &mut viewers,
                &out,
                &mut chunk_cache,
                &mut connections,
            )
            .await;
            let publish_us = publish_start
                .elapsed()
                .as_micros()
                .min(u128::from(u64::MAX)) as u64;
            server
                .dashboard
                .record_pump_tick(crate::dashboard::PumpTick {
                    tick: out.tick,
                    core_us: out.times.total_us,
                    publish_us,
                    bytes_written: server.net_counters.bytes_written.load(Ordering::Relaxed),
                    reliable_drops: server.net_counters.reliable_drops.load(Ordering::Relaxed),
                    chunks: DashboardChunks {
                        loaded_total: out.snapshot.chunks.len(),
                    },
                    entities: DashboardEntities {
                        total: out.snapshot.entities.len(),
                    },
                    players: roster.map(|names| crate::dashboard::DashboardPlayers {
                        count: names.len(),
                        max: server.cfg.server.max_players,
                        names,
                    }),
                });
            tracing::debug!(
                tick = out.tick,
                viewers = viewers.len(),
                elapsed_us = publish_us,
                "pump published tick"
            );
            // Yield after every publish. Routing wakes connection tasks into
            // this worker's unstealable LIFO slot, and `rx.recv()` returns
            // Ready while outputs are queued, so a catching-up pump never
            // reached an await point: a Java client's writer sat queued
            // behind it with 512 routed chunks for 5+ s (stuck loading).
            tokio::task::yield_now().await;
        }
    }))
}

/// Drain `Core::try_drain_outbox` and route reliable changes FIRST (before
/// the world publish). Each `PlayerChange` is mapped to framed reliable
/// packets via `plan::plan_player_changes` (single-sourced with the world
/// plan; includes `GameModeChanged`/cursor). A full reliable queue
/// disconnects; changes for unknown/gone connections are dropped with the
/// connection.
fn configured_viewer(
    server: &EdgeServer,
    connection: ConnectionId,
    entity: mc_core::boundary::EntityId,
    position: [f64; 3],
    dimension: mc_core::dimensions::Dimension,
) -> crate::plan::ViewerState {
    let mut viewer = crate::plan::ViewerState::new(connection, entity, position);
    viewer.dimension = dimension;
    viewer.view_distance = server
        .connections
        .inner
        .lock()
        .view_distances
        .get(&connection)
        .copied();
    viewer.dimension_type_ids = server.wire.dimension_type_ids;
    viewer.advancements = Arc::clone(&server.recipes.advancements);
    viewer
}

fn route_outbox(
    server: &Arc<EdgeServer>,
    viewers: &mut HashMap<ConnectionId, crate::plan::ViewerState>,
    connections: &mut HashMap<ConnectionId, ConnectionHandles>,
) {
    // Ensure every live connection has a viewer entry before planning.
    // The entity id comes from the stashed `Joined` when available (the
    // core allocates it; ids diverge from connection ids in ordinary
    // operation). Otherwise fall back to spawn position [0.5,64,0.5] with
    // the entity still unknown — `plan_player_changes` adopts it from the
    // first `Joined` it plans.
    for &id in connections.keys() {
        viewers.entry(id).or_insert_with(|| {
            let (entity, position, dimension) = server
                .connections
                .take_join(id)
                .map(|(entity, position, _, dimension)| (entity, position, dimension))
                .unwrap_or((
                    mc_core::boundary::EntityId(id.0 as i32),
                    [0.5, 64.0, 0.5],
                    Default::default(),
                ));
            configured_viewer(server, id, entity, position, dimension)
        });
    }
    for (connection, changes) in server.hub.try_drain_outbox() {
        if connection == mc_core::command_console::CONNECTION {
            crate::console::output(&server.console_state, &changes);
            continue;
        }
        // Core-ordered kick: stash the reason so the play loop can log
        // peer+reason after flushing the kick packet. The packet itself
        // is planned below (reliable, last) and routed in order.
        for change in changes.iter() {
            if let mc_core::boundary::PlayerChange::Disconnect { reason, .. } = change {
                server
                    .connections
                    .stash_disconnect_reason(connection, reason.clone());
                break;
            }
        }
        // Join race: the pump may drain before the connection registered
        // its queues (join-wait runs pre-registration). Stash the Joined
        // for `wait_for_join` AND the whole ordered reliable list for
        // replay at registration — dropping the tail would lose the
        // authoritative MenuContents/Health/Experience burst.
        // A registration can race the tick snapshot: a newcomer's handles are
        // looked up live, and if it is still unregistered its whole ordered
        // list (Joined first, for `wait_join`) is stashed for replay at
        // registration, atomically with that lookup.
        let mut changes = changes;
        if let std::collections::hash_map::Entry::Vacant(slot) = connections.entry(connection) {
            let Some(handles) = server
                .connections
                .handles_or_stash(connection, &mut changes)
            else {
                continue;
            };
            slot.insert(handles);
            let (entity, position, dimension) = server
                .connections
                .take_join(connection)
                .map(|(entity, position, _, dimension)| (entity, position, dimension))
                .unwrap_or((
                    mc_core::boundary::EntityId(connection.0 as i32),
                    [0.5, 64.0, 0.5],
                    Default::default(),
                ));
            viewers.entry(connection).or_insert_with(|| {
                configured_viewer(server, connection, entity, position, dimension)
            });
        }
        let Some(handles) = connections.get(&connection) else {
            continue;
        };
        let Some(viewer) = viewers.get_mut(&connection) else {
            continue;
        };
        // The authenticated roster is also the edge's unlock notification.
        // Restore the private operator command tree once, not on every tick.
        if !viewer.player_profiles.contains_key(&viewer.entity_id)
            && let Some(uuid) = changes.iter().find_map(|change| match change {
                mc_core::boundary::PlayerChange::Roster { players, .. } => players
                    .iter()
                    .find(|player| player.connection == connection)
                    .map(|player| player.uuid),
                _ => None,
            })
            && server
                .send(
                    connection,
                    &crate::command_tree::tree(
                        server.admin.level(uuid),
                        server.hub.plugin_commands(),
                    ),
                )
                .is_err()
        {
            connections.remove(&connection);
            viewers.remove(&connection);
            continue;
        }
        for planned in crate::plan::plan_player_changes(&changes, viewer) {
            if handles.reliable_tx.try_send(planned.data).is_err() {
                server.connections.remove(connection);
                connections.remove(&connection);
                server
                    .net_counters
                    .reliable_drops
                    .fetch_add(1, Ordering::Relaxed);
                viewers.remove(&connection);
                break;
            }
        }
    }
}

/// Publish one tick to all viewers and route the addressed packets.
/// Reliable-overflow victims disconnect.
async fn publish_and_route(
    server: &Arc<EdgeServer>,
    pool: &rayon::ThreadPool,
    viewers: &mut HashMap<ConnectionId, crate::plan::ViewerState>,
    out: &TickOutput,
    chunk_cache: &mut crate::plan::ChunkPacketCache,
    connections: &mut HashMap<ConnectionId, ConnectionHandles>,
) {
    viewers.retain(|id, _| connections.contains_key(id));
    // Only the pump sends world packets; available slots can only grow
    // between this credit read and routing.
    {
        let registry = server.connections.inner.lock();
        for (id, viewer) in viewers.iter_mut() {
            viewer.view_distance = registry.view_distances.get(id).copied();
            viewer.world_credit = connections
                .get(id)
                .map_or(0, |handles| handles.world_tx.capacity());
        }
    }

    let view_distance = i32::from(server.cfg.server.view_distance);
    // Real sidecar registries: the `publish` single-biome fallback would
    // fail every non-plains chunk encode and send no terrain at all.
    let registries = crate::plan::PlanRegistries {
        biomes: &server.wire.biomes,
        blocks: &server.wire.blocks,
        items: &server.wire.items,
        block_entity_types: &server.wire.block_entity_types,
    };
    let packets = pool.install(|| {
        crate::fanout::publish_with_registries(
            out,
            viewers.values_mut(),
            view_distance,
            &registries,
            chunk_cache,
        )
    });
    // Reliable overflow takes precedence: disconnect victims first.
    for victim in crate::fanout::reliable_overflow(&packets) {
        server.connections.remove(victim);
        connections.remove(&victim);
        server
            .net_counters
            .reliable_drops
            .fetch_add(1, Ordering::Relaxed);
        viewers.remove(&victim);
    }
    for packet in packets {
        let Some(handles) = connections.get(&packet.connection) else {
            continue;
        };
        match packet.priority {
            crate::fanout::Priority::Reliable => {
                // In-order, never dropped: a full reliable queue
                // disconnects the connection.
                if handles.reliable_tx.try_send(packet.data).is_err() {
                    server.connections.remove(packet.connection);
                    connections.remove(&packet.connection);
                    server
                        .net_counters
                        .reliable_drops
                        .fetch_add(1, Ordering::Relaxed);
                    viewers.remove(&packet.connection);
                }
            }
            crate::fanout::Priority::BestEffort => {
                // Planned within the queue's free slots: this fails only
                // when the connection has closed.
                let _ = handles.world_tx.try_send(packet.data);
            }
        }
    }
}

fn effective_view_distance(client: i8, server: u8) -> i32 {
    i32::from(client).max(2).min(i32::from(server))
}

/// Wait (bounded) for our [`PlayerChange::Joined`]. Only the pump drains
/// the core outbox; this waiter never touches it. Fast path: the pump may
/// have stashed our Joined already. Otherwise register a one-shot waiter
/// (fired by the pump's [`stash_join`][ConnectionRegistry::stash_join])
/// and await it — no polling, no draining, no other connection's changes.
async fn wait_for_join(
    server: &Arc<EdgeServer>,
    id: ConnectionId,
    _name: &str,
    client_view_distance: i8,
    client_language: &str,
) -> anyhow::Result<JoinedSpawn> {
    {
        let mut inner = server.connections.inner.lock();
        inner.view_distances.insert(
            id,
            effective_view_distance(client_view_distance, server.cfg.server.view_distance),
        );
        // Already submitted before Join: remember it so the first Play
        // ClientInformation does not repeat the same locale.
        inner.locales.insert(id, client_language.to_owned());
    }
    // Fast path: Joined arrived before we registered interest.
    if let Some(joined) = server.connections.take_join(id) {
        return Ok(joined);
    }
    let receiver = server.connections.wait_join(id);
    // Re-check after registering: the pump may have stashed between the
    // fast-path check and `wait_join` (stash fires waiters, but ours was
    // registered after the stash — `take_join` covers that window).
    if let Some(joined) = server.connections.take_join(id) {
        return Ok(joined);
    }
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(10));
    tokio::pin!(deadline);
    tokio::select! {
        biased;
        _ = &mut deadline => anyhow::bail!("timed out waiting for join"),
        result = receiver => {
            result.map_err(|_| anyhow::anyhow!("join waiter dropped before Joined"))
        }
    }
}

/// Newest tick output available without blocking (for the join burst).
fn latest_output(server: &Arc<EdgeServer>) -> TickOutput {
    let mut rx = server.hub.subscribe();
    // A fresh broadcast subscriber only sees outputs published after
    // subscribing; the join lands on the next tick, so fall back to an
    // empty output and let the burst carry position data.
    match rx.try_recv() {
        Ok(out) => out,
        Err(_) => empty_output(),
    }
}

fn empty_output() -> TickOutput {
    TickOutput {
        tick: 0,
        snapshot: Arc::new(mc_core::boundary::WorldSnapshot {
            tick: 0,
            day_time: 0,
            chunks: Arc::default(),
            day_time_rate: 1.0,
            ..Default::default()
        }),
        changes: Arc::new(ChangeSet::default()),
        times: Default::default(),
    }
}

/// Resolve and validate the cache once before building core configuration.
/// This is the same discovery used by `content verify`; it never downloads.
pub fn resolve_content_config(cfg: &mut EdgeConfig) -> anyhow::Result<()> {
    let search = crate::content_cache::ContentSearch::for_config(cfg)?;
    resolve_content_config_from_search(cfg, &search)
}

pub(crate) fn resolve_content_config_from_search(
    cfg: &mut EdgeConfig,
    search: &crate::content_cache::ContentSearch,
) -> anyhow::Result<()> {
    let cache = crate::content_cache::discover_content_cache(search)?;
    cfg.data.vanilla_data_dir = cache.root().to_path_buf();
    Ok(())
}

/// Build [`CoreConfig`] for the real core from the edge config.
/// Call [`resolve_content_config`] first so all core loaders use that same root.
pub fn core_config(cfg: &EdgeConfig) -> mc_core::config::CoreConfig {
    let mut core = mc_core::config::CoreConfig::new(cfg.data.world_dir.clone(), cfg.data.seed);
    core.view_distance = cfg.server.view_distance;
    core.simulation_distance = cfg.server.simulation_distance;
    // Each admitted session reserves one cleanup slot without consuming the
    // ordinary-input budget at the configured player limit.
    core.input_queue_cap = core
        .input_queue_cap
        .saturating_add(cfg.server.max_players as usize);
    // Full item facts/tags/recipes/loot/mining come from the content cache;
    // without this the core keeps embedded subsets with no tool rules and
    // drops ignore the correct-tool gate.
    core.vanilla_data_dir = Some(cfg.data.vanilla_data_dir.clone());
    core.plugin_directory = cfg.plugins.directory.clone();
    core.require_plugin_login = cfg.auth.require_plugin_login;
    core.max_airtime_ticks = cfg.anti_cheat.max_airtime_ticks;
    core.pause_when_empty_seconds =
        u32::try_from(cfg.server.pause_when_empty_seconds.max(0)).unwrap_or(0);
    core.enable_command_block = cfg.server.enable_command_block;
    core.input_log = cfg.server.input_log;
    core.mob_griefing = mc_core::config::MobGriefingRules {
        creeper_block_damage: cfg.mob_griefing.creeper_block_damage,
        ghast_block_damage: cfg.mob_griefing.ghast_block_damage,
        wither_block_damage: cfg.mob_griefing.wither_block_damage,
        villager_farming: cfg.mob_griefing.villager_farming,
        sheep_grazing: cfg.mob_griefing.sheep_grazing,
    };
    core.block_log = mc_core::block_log::Settings {
        enabled: cfg.block_log.enabled,
        retention_secs: u64::from(cfg.block_log.retention_days) * 24 * 60 * 60,
        max_records: cfg.block_log.max_records,
    };
    core
}

/// Load protocol data from the same discovered cache used by verification.
/// Call [`resolve_content_config`] first, as for [`core_config`]. Unresolved
/// library callers may discover a cache here; resolved startup does not repeat
/// the full gameplay-data validation.
pub fn load_vanilla_data(cfg: &EdgeConfig) -> anyhow::Result<Arc<VanillaData>> {
    let data = if cfg.data.vanilla_data_dir.as_os_str().is_empty() {
        let search = crate::content_cache::ContentSearch::for_config(cfg)?;
        let cache = crate::content_cache::discover_content_cache(&search)?;
        crate::startup_data::load_effective_protocol_data(cache.root())?
    } else {
        crate::startup_data::load_effective_protocol_data(&cfg.data.vanilla_data_dir)?
    };
    Ok(Arc::new(data))
}

/// Load the resolved vanilla tag set for the Configuration tag packet.
/// Fails startup with a clear error: without tags the client kicks itself
/// with "Unbound tags" on `FinishConfiguration`.
/// Call [`resolve_content_config`] first. For an unresolved library caller,
/// use the root already selected by [`load_vanilla_data`], never rediscover it.
pub fn load_tags(
    cfg: &EdgeConfig,
    data: &VanillaData,
) -> anyhow::Result<Arc<mc_data::tags::TagsData>> {
    let root = if cfg.data.vanilla_data_dir.as_os_str().is_empty() {
        data.root()
    } else {
        &cfg.data.vanilla_data_dir
    };
    let items = mc_data::items::solaris_required_items();
    let tags = crate::startup_data::load_effective_tags(root, data, &items)?;
    Ok(Arc::new(tags))
}

/// Build the dashboard state with fixed world facts from config.
fn dashboard_state(cfg: &EdgeConfig) -> Arc<DashboardState> {
    let dashboard = Arc::new(DashboardState::new(DashboardWorld {
        name: cfg.server.name.clone(),
        motd: cfg.server.motd.clone(),
        seed: cfg.data.seed,
        view_distance: cfg.server.view_distance,
        simulation_distance: cfg.server.simulation_distance,
        max_players: cfg.server.max_players,
    }));
    dashboard.set_metrics_bearer_token(cfg.dashboard.metrics_bearer_token.clone());
    dashboard
}

/// Whether the dashboard may bind its configured address: loopback always,
/// remote only with `allow_remote = true`.
pub fn dashboard_allowed(cfg: &EdgeConfig) -> bool {
    let addr = format!("{}:{}", cfg.dashboard.bind_address, cfg.dashboard.port);
    let is_loopback = addr
        .parse::<std::net::SocketAddr>()
        .map(|s| s.ip().is_loopback())
        .unwrap_or_else(|_| cfg.dashboard.bind_address == "localhost");
    is_loopback || cfg.dashboard.allow_remote
}

/// Whether MSMP may bind its configured address: loopback always, remote only
/// when explicitly allowed alongside its mandatory bearer token.
pub fn msmp_allowed(cfg: &EdgeConfig) -> bool {
    let addr = format!("{}:{}", cfg.msmp.bind_address, cfg.msmp.port);
    let is_loopback = addr
        .parse::<std::net::SocketAddr>()
        .map(|address| address.ip().is_loopback())
        .unwrap_or_else(|_| cfg.msmp.bind_address == "localhost");
    is_loopback || cfg.msmp.allow_remote
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_queues_aggregate_real_slots_and_expose_unavailable_metrics() {
        let registry = ConnectionRegistry::new();
        assert!(registry.admit(ConnectionId(1), 2, |_| None).is_none());
        assert!(registry.admit(ConnectionId(2), 2, |_| None).is_none());
        let (reliable_tx, mut reliable_rx) = mpsc::channel(4);
        let (world_tx, _world_rx) = mpsc::channel(4);
        reliable_tx
            .try_send(bytes::Bytes::from_static(b"abc"))
            .unwrap();
        let reserved = world_tx.try_reserve().unwrap();
        registry
            .register(
                ConnectionId(1),
                ConnectionHandles {
                    reliable_tx: reliable_tx.clone(),
                    world_tx: world_tx.clone(),
                },
            )
            .unwrap();
        let (reliable_tx2, _reliable_rx2) = mpsc::channel(2);
        let (world_tx2, _world_rx2) = mpsc::channel(2);
        world_tx2
            .try_send(bytes::Bytes::from_static(b"longer payload"))
            .unwrap();
        registry
            .register(
                ConnectionId(2),
                ConnectionHandles {
                    reliable_tx: reliable_tx2,
                    world_tx: world_tx2,
                },
            )
            .unwrap();
        let snapshot = registry.load_queues().unwrap();
        assert_eq!(snapshot.viewers, 2);
        assert_eq!(snapshot.reliable.capacity_slots, 6);
        assert_eq!(snapshot.world.capacity_slots, 6);
        assert_eq!(snapshot.reliable.queued_or_reserved_slots, 1);
        assert_eq!(snapshot.world.queued_or_reserved_slots, 2);
        assert_eq!(snapshot.world.max_viewer_queued_or_reserved_slots, 1);
        assert!(snapshot.capabilities.queued_or_reserved_slots);
        assert!(!snapshot.capabilities.queued_slots);
        assert!(!snapshot.capabilities.reserved_slots);
        assert!(!snapshot.capabilities.queued_bytes);
        assert!(!snapshot.capabilities.reserved_bytes);
        assert!(snapshot.world.queued_bytes.is_none());
        assert!(snapshot.world.reserved_bytes.is_none());
        assert!(snapshot.world.queued_slots.is_none());
        assert!(snapshot.world.reserved_slots.is_none());
        drop(reserved);
        reliable_rx.try_recv().unwrap();
        let snapshot = registry.load_queues().unwrap();
        assert_eq!(snapshot.reliable.queued_or_reserved_slots, 0);
        assert_eq!(snapshot.world.queued_or_reserved_slots, 1);
    }

    #[test]
    fn registry_insert_remove_roster() {
        let registry = ConnectionRegistry::new();
        let id = registry.joining_id();
        let (reliable_tx, _) = mpsc::channel(2);
        let (world_tx, _) = mpsc::channel(2);
        registry
            .register(
                id,
                ConnectionHandles {
                    reliable_tx,
                    world_tx,
                },
            )
            .unwrap();
        assert_eq!(registry.len(), 1);
        registry.set_name(id, "Alex".to_owned());
        assert_eq!(registry.roster(), vec!["Alex".to_owned()]);
        registry.remove(id);
        assert!(registry.is_empty());
    }

    #[test]
    fn dashboard_gate_requires_allow_remote() {
        let mut cfg = EdgeConfig::default();
        cfg.dashboard.bind_address = "0.0.0.0".to_owned();
        assert!(!dashboard_allowed(&cfg));
        cfg.dashboard.allow_remote = true;
        assert!(dashboard_allowed(&cfg));
        cfg.dashboard.bind_address = "127.0.0.1".to_owned();
        cfg.dashboard.allow_remote = false;
        assert!(dashboard_allowed(&cfg));
    }

    #[test]
    fn core_config_carries_distances_and_airtime_limit() {
        let mut cfg = EdgeConfig::default();
        cfg.server.view_distance = 4;
        cfg.server.simulation_distance = 3;
        cfg.anti_cheat.max_airtime_ticks = 9;
        let core = core_config(&cfg);
        assert_eq!(core.view_distance, 4);
        assert_eq!(core.simulation_distance, 3);
        assert_eq!(core.max_airtime_ticks, 9);
    }

    #[test]
    fn core_config_carries_pause_and_mob_griefing_switches() {
        let mut cfg = EdgeConfig::default();
        assert_eq!(core_config(&cfg).pause_when_empty_seconds, 60);
        cfg.server.pause_when_empty_seconds = 5;
        cfg.mob_griefing.creeper_block_damage = false;
        cfg.mob_griefing.villager_farming = false;
        let core = core_config(&cfg);
        assert_eq!(core.pause_when_empty_seconds, 5);
        assert!(!core.mob_griefing.creeper_block_damage);
        assert!(!core.mob_griefing.villager_farming);
        assert!(core.mob_griefing.ghast_block_damage);
        assert!(core.mob_griefing.wither_block_damage);
        assert!(core.mob_griefing.sheep_grazing);
    }

    /// Two simultaneous joins both resolve; a late waiter sees the stash.
    #[tokio::test]
    async fn simultaneous_joins_both_resolve() {
        let registry = ConnectionRegistry::new();
        let a = registry.joining_id();
        let b = registry.joining_id();
        // Both register interest BEFORE either Joined arrives (simultaneous).
        let rx_a = registry.wait_join(a);
        let rx_b = registry.wait_join(b);
        // Pump routes A's Joined: fires A's waiter, stashes for fast path.
        let joined_a = (
            mc_core::boundary::EntityId(11),
            [1.0, 2.0, 3.0],
            0u8,
            Default::default(),
        );
        registry.stash_join(a, joined_a);
        // B's waiter must NOT fire from A's Joined.
        assert!(registry.take_join(b).is_none());
        // Pump routes B's Joined.
        let joined_b = (
            mc_core::boundary::EntityId(22),
            [4.0, 5.0, 6.0],
            1u8,
            Default::default(),
        );
        registry.stash_join(b, joined_b);
        assert_eq!(rx_a.await.expect("A resolves"), joined_a);
        assert_eq!(rx_b.await.expect("B resolves"), joined_b);
        // Stash fast path still holds both for `take_join` callers.
        assert_eq!(registry.take_join(a), Some(joined_a));
        assert_eq!(registry.take_join(b), Some(joined_b));
    }

    /// Fast path: Joined stashed before interest registered resolves
    /// immediately (covers the pump-beat-us-to-it window).
    #[tokio::test]
    async fn late_waiter_sees_stash() {
        let registry = ConnectionRegistry::new();
        let id = registry.joining_id();
        let joined = (
            mc_core::boundary::EntityId(7),
            [8.0, 80.0, 8.0],
            0u8,
            Default::default(),
        );
        registry.stash_join(id, joined);
        // wait_for_join's fast path.
        assert_eq!(registry.take_join(id), Some(joined));
    }

    /// P1: the whole pre-registration reliable burst is stashed in arrival
    /// order (not just the Joined), so registration can replay it.
    #[test]
    fn pending_reliable_keeps_burst_order() {
        use mc_core::boundary::PlayerChange;
        let registry = ConnectionRegistry::new();
        let id = registry.joining_id();
        let burst = vec![
            PlayerChange::Joined {
                entity: mc_core::boundary::EntityId(41),
                dimension: Default::default(),
                position: [8.0, 64.0, 8.0],
                yaw: 0.0,
                pitch: 0.0,
                game_mode: 0,
            },
            PlayerChange::MenuContents {
                window: 0,
                state_id: 1,
                slots: Vec::new(),
                cursor: None,
            },
            PlayerChange::Health {
                health: 20.0,
                food: 20,
                saturation: 5.0,
            },
            PlayerChange::Experience {
                progress: 0.0,
                level: 0,
                total: 0,
            },
        ];
        let mut first = burst.clone();
        assert!(registry.handles_or_stash(id, &mut first).is_none());
        // A second pre-registration drain appends in order (never replaces).
        let mut late = vec![PlayerChange::Chat {
            message: "late".to_owned(),
        }];
        assert!(registry.handles_or_stash(id, &mut late).is_none());
        let mut expected = burst;
        expected.push(PlayerChange::Chat {
            message: "late".to_owned(),
        });
        let (reliable_tx, _reliable_rx) = mpsc::channel(1);
        let (world_tx, _world_rx) = mpsc::channel(1);
        let handles = ConnectionHandles {
            reliable_tx,
            world_tx,
        };
        assert_eq!(registry.register(id, handles.clone()).unwrap(), expected);
        // Once registered the pump routes instead of stashing.
        let mut routed = vec![expected[1].clone()];
        assert!(registry.handles_or_stash(id, &mut routed).is_some());
        assert_eq!(routed.len(), 1);
        // Remove also clears a stash.
        registry.remove(id);
        assert!(registry.handles_or_stash(id, &mut routed).is_none());
        registry.remove(id);
        assert!(registry.register(id, handles).is_err());
    }

    /// P1: the stashed Joined is the viewer entity source, so repeated
    /// `take_join` calls keep returning it (registration + pump ensures).
    #[test]
    fn stashed_join_survives_repeated_takes() {
        let registry = ConnectionRegistry::new();
        let id = registry.joining_id();
        let joined = (
            mc_core::boundary::EntityId(99),
            [1.0, 2.0, 3.0],
            1u8,
            Default::default(),
        );
        registry.stash_join(id, joined);
        assert_eq!(registry.take_join(id), Some(joined));
        assert_eq!(registry.take_join(id), Some(joined));
    }

    /// The join-burst replay must skip the stale `Joined` sync itself: the
    /// burst already sent teleport id 1 + empty inventory/health, and the
    /// `Joined` arm would re-emit all four (second teleport + reset to
    /// empty). Only the authoritative tail replays.
    #[test]
    fn join_replay_skips_stale_joined_sync() {
        use mc_core::boundary::PlayerChange;
        let id = ConnectionId(3);
        let stashed = vec![
            PlayerChange::Joined {
                entity: mc_core::boundary::EntityId(41),
                dimension: Default::default(),
                position: [8.0, 80.0, 8.0],
                yaw: 0.0,
                pitch: 0.0,
                game_mode: 0,
            },
            PlayerChange::MenuContents {
                window: 0,
                state_id: 1,
                slots: Vec::new(),
                cursor: None,
            },
            PlayerChange::Health {
                health: 20.0,
                food: 20,
                saturation: 5.0,
            },
        ];
        // Mirror of the registration replay: drop through the Joined.
        let tail: Vec<PlayerChange> = stashed
            .into_iter()
            .skip_while(|change| !matches!(change, PlayerChange::Joined { .. }))
            .skip(1)
            .collect();
        assert_eq!(tail.len(), 2);
        let mut viewer =
            crate::plan::ViewerState::new(id, mc_core::boundary::EntityId(41), [8.0, 80.0, 8.0]);
        use mc_protocol::packets::Packet as _;
        use mc_protocol::packets::play::{
            ClientboundContainerSetContent, ClientboundSetHealth, SynchronizePlayerPosition,
        };
        let planned = crate::plan::plan_player_changes(&tail, &mut viewer);
        assert!(
            planned
                .iter()
                .all(|p| p.id != SynchronizePlayerPosition::ID),
            "replay must not re-send the stale join teleport"
        );
        assert!(
            planned
                .iter()
                .any(|p| p.id == ClientboundContainerSetContent::ID)
        );
        assert!(planned.iter().any(|p| p.id == ClientboundSetHealth::ID));
    }

    #[test]
    fn native_settlements_c_binding_decodes_to_its_declared_owner() {
        use mc_protocol::packets::CustomPayload;
        use mc_protocol::packets::Packet;
        use mc_protocol::packets::play::ServerboundCustomPayload;

        let manifest =
            mc_loader::LoaderManifest::from_script_bundles(&[native_settlements_bundle().unwrap()])
                .unwrap();
        let packet = ServerboundCustomPayload {
            payload: CustomPayload::Unknown {
                channel: mc_loader::loader_view_action_channel().clone(),
                payload: br#"{"protocol":3,"message":"view_request","request_kind":"solaris-settlements:overview"}"#.to_vec(),
            },
        };
        let mut body = Vec::new();
        packet.encode(&mut body).unwrap();

        let LoaderViewIngress::Request(PlayerInput::LoaderViewRequest {
            connection,
            owner,
            request_kind,
        }) = loader_view_ingress(&manifest, ConnectionId(41), &body).unwrap()
        else {
            panic!("C binding must route a declared view request");
        };
        assert_eq!(connection, ConnectionId(41));
        assert_eq!(owner, NATIVE_SETTLEMENTS_OWNER);
        assert_eq!(request_kind.as_str(), "solaris-settlements:overview");
    }

    #[test]
    fn native_settlements_bundle_is_optional_and_offered_to_loader_clients_only() {
        let manifest = loader_manifest(&[])
            .unwrap()
            .expect("native bundle manifest");
        assert_eq!(
            manifest.bundles.len(),
            1,
            "only the native settlements bundle"
        );
        assert!(
            !native_settlements_bundle().unwrap().required(),
            "the native bundle never forces the Loader on a vanilla client"
        );
        assert!(
            !manifest.bundles[0]
                .loaders
                .contains(&mc_loader::LoaderPlatform::Native),
            "the native client must not parse Java Loader input bindings"
        );
    }
    #[test]
    fn packet_limiter_is_per_type_refills_and_can_be_disabled() {
        let config = crate::config::AntiCheatSection {
            packet_rate_per_second: 2,
            packet_burst: 2,
            max_airtime_ticks: 80,
        };
        let mut limiter = PacketLimiter::new(&config);
        let now = Instant::now();
        assert!(limiter.allow_at(5, now));
        assert!(limiter.allow_at(5, now));
        assert!(!limiter.allow_at(5, now));
        assert!(
            limiter.allow_at(6, now),
            "packet ids have independent buckets"
        );
        assert!(limiter.allow_at(5, now + std::time::Duration::from_millis(500)));

        let disabled = crate::config::AntiCheatSection {
            packet_rate_per_second: 0,
            packet_burst: 0,
            max_airtime_ticks: 80,
        };
        let mut limiter = PacketLimiter::new(&disabled);
        assert!(limiter.allow_at(5, now));
        assert!(limiter.allow_at(5, now));
    }
}

#[cfg(test)]
#[path = "admin_tests.rs"]
mod admin_tests;

#[cfg(test)]
#[path = "server_lifecycle_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
#[path = "server_lock_tests.rs"]
mod lock_tests;

#[cfg(test)]
#[path = "core_halt_tests.rs"]
mod core_halt_tests;

#[cfg(test)]
#[path = "server_view_distance_tests.rs"]
mod view_distance_tests;

#[cfg(test)]
#[path = "auth_edge_fence_tests.rs"]
mod auth_edge_fence_tests;

#[cfg(test)]
#[path = "transfer_tests.rs"]
mod transfer_tests;

#[cfg(test)]
#[path = "server_leave_tests.rs"]
mod leave_tests;

#[cfg(test)]
#[path = "server_protocol_admission_tests.rs"]
mod protocol_admission_tests;

#[cfg(test)]
#[path = "server_session_security_tests.rs"]
mod session_security_tests;
