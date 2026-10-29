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
mod registry;
pub use registry::*;
mod connection;
use connection::*;
mod pump;
use pump::*;

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
    /// 26.1.2 (protocol 775) client profile, when its content cache exists.
    pub legacy: Option<Arc<crate::legacy::Profile>>,
    /// Chunk-encoding registries (built once at startup; the tick pump
    /// must encode against the real biome/block/item tables, never the
    /// single-biome fallback inside `fanout::publish`).
    pub wire: WireRegistries,
    /// Server configuration.
    pub cfg: EdgeConfig,
    /// Live connection registry (for pump routing + dashboard roster).
    pub connections: Arc<ConnectionRegistry>,
    pre_play_addresses: crate::conn_limit::AddressLimiter,
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
    /// `solaris:screen` relay (ADR 0018 "Screen sharing").
    pub(crate) screens: crate::screen_service::Service,
    /// `solaris:voice` proximity relay (ADR 0019).
    pub(crate) voice: crate::voice_service::Service,
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
        cfg.voice.validate()?;
        let voice = crate::voice_service::Service::new(&cfg.voice);
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
        anyhow::ensure!(
            hub.submit(PlayerInput::SpawnProtectionOperators {
                operators: admin.operator_ids()
            }),
            "spawn protection operator snapshot queue is full"
        );
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
        let legacy = crate::legacy::Profile::discover(&data);
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
            screens: Default::default(),
            voice,
            recipes,
            hub,
            data,
            tags,
            legacy,
            wire,
            cfg,
            connections,
            pre_play_addresses: crate::conn_limit::AddressLimiter::new(0, 8),
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
        let mut body = Vec::new();
        mc_protocol::packets::play::PlayDisconnect { reason_nbt }.encode(&mut body)?;
        let mut frame = bytes::BytesMut::new();
        mc_protocol::frame::append_frame(
            &mut frame,
            mc_protocol::packets::play::PlayDisconnect::ID,
            &body,
            mc_protocol::frame::Compression::Threshold(256),
        )?;
        // Publish the terminal frame and reason atomically with the writer's
        // terminal-queue check, not before enqueueing the frame.
        let mut inner = self.connections.inner.lock();
        let handles = inner
            .handles
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("connection closed before disconnect"))?;
        if handles.reliable_tx.try_send(frame.freeze()).is_err() {
            drop(inner);
            self.connections.remove(id);
            anyhow::bail!("reliable command queue full");
        }
        inner.disconnect_reasons.insert(id, reason.to_string());
        Ok(())
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
        let (target, arguments) = if matches!(root, "kick" | "ban" | "pardon") {
            let parsed = match Core::moderation_command_words(command) {
                Ok(parsed) => parsed,
                Err(error) => {
                    self.feedback(id, &format!("Command failed: {error}"))?;
                    return Ok(true);
                }
            };
            (
                parsed.get(1).copied(),
                parsed
                    .get(2)
                    .map(|rest| rest.split_whitespace().collect::<Vec<_>>())
                    .unwrap_or_default(),
            )
        } else {
            (words.next(), words.collect::<Vec<_>>())
        };
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
        if matches!(root, "kick" | "ban" | "pardon") {
            let targets: Vec<(Option<ConnectionId>, uuid::Uuid, String)> =
                if target.starts_with('@') {
                    let selected = match self
                        .hub
                        .resolve_moderation_selector(id, target.to_owned())
                        .await
                    {
                        Ok(selected) => selected,
                        Err(error) => {
                            self.feedback(id, &format!("Command failed: {error}"))?;
                            return Ok(true);
                        }
                    };
                    if id != mc_core::command_console::CONNECTION && self.hub.session_locked(id) {
                        return Ok(false);
                    }
                    selected
                        .into_iter()
                        .map(|(connection, uuid, name)| (Some(connection), uuid, name))
                        .collect()
                } else if let Some((connection, uuid, name, _)) = live.as_ref() {
                    vec![(Some(*connection), *uuid, name.clone())]
                } else if root == "kick" {
                    Vec::new()
                } else if let Some((uuid, name)) = self.admin.profile(target).or_else(|| {
                    (!self.cfg.auth.online_mode)
                        .then(|| (crate::crypto::offline_uuid(target), target.into()))
                }) {
                    vec![(None, uuid, name)]
                } else {
                    self.feedback(id, "Unknown player")?;
                    return Ok(true);
                };
            let banned = if root == "kick" {
                Vec::new()
            } else {
                self.admin.bans().0
            };
            let mut changed = 0;
            for (connection, uuid, name) in targets {
                if let Some(connection) = connection
                    && self.admin.uuid_of(connection) != Some(uuid)
                {
                    continue;
                }
                if root == "kick" {
                    if let Some(connection) = connection {
                        self.kick(
                            connection,
                            if reason.is_empty() {
                                serde_json::json!({"translate":"multiplayer.disconnect.kicked"})
                            } else {
                                serde_json::json!({"text":reason.as_str()})
                            },
                        )?;
                        changed += 1;
                    }
                    continue;
                }
                let suffix = format!(" ({uuid})");
                let already_banned = banned.iter().any(|entry| entry.ends_with(&suffix));
                if already_banned == (root == "ban") {
                    continue;
                }
                if let Err(error) = self.admin.ban_player_for(
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
                ) {
                    self.feedback(id, &format!("Command failed: {error}"))?;
                    return Ok(true);
                }
                if root == "ban"
                    && let Some(connection) = connection
                {
                    self.kick(
                        connection,
                        serde_json::json!({"translate":"multiplayer.disconnect.banned"}),
                    )?;
                }
                changed += 1;
            }
            if changed == 0 {
                self.feedback(
                    id,
                    if root == "kick" {
                        "No player was found"
                    } else {
                        "Command failed: no applicable player profiles"
                    },
                )?;
            } else {
                let expiry = duration.map_or_else(String::new, |value| {
                    format!(" (expires in {}s)", value.as_secs())
                });
                self.feedback(
                    id,
                    &format!("Command {root} completed for {target}{expiry}"),
                )?;
            }
            return Ok(true);
        }
        let result =
            if root == "ban-ip" || root == "pardon-ip" {
                let valid_word = target.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'+' | b'.' | b'_')
                });
                let ip = if valid_word {
                    target
                        .parse::<std::net::Ipv4Addr>()
                        .ok()
                        .map(std::net::IpAddr::V4)
                        .or_else(|| {
                            if root == "ban-ip" {
                                live.as_ref().map(|session| session.3)
                            } else {
                                None
                            }
                        })
                } else {
                    None
                };
                if let Some(ip) = ip {
                    let ip_text = ip.to_string();
                    let already_banned = self.admin.bans().1.iter().any(|entry| entry == &ip_text);
                    if already_banned == (root == "ban-ip") {
                        self.feedback(id, "Command failed: no applicable IP address")?;
                        return Ok(true);
                    }
                    let result = self.admin.ban_ip(
                        &ip_text,
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
                        if result.is_ok() {
                            let snapshot = PlayerInput::SpawnProtectionOperators {
                                operators: self.admin.operator_ids(),
                            };
                            let mut sheds = 0;
                            while !self.hub.submit(snapshot.clone()) {
                                sheds += 1;
                                anyhow::ensure!(
                                    sheds < MAX_SUBMIT_SHEDS,
                                    "spawn protection operator snapshot queue is full"
                                );
                                tokio::task::yield_now().await;
                            }
                        }
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
    let screen_ticks = crate::screen_service::spawn_tick_task(Arc::clone(&server));
    let voice_ticks = crate::voice_service::spawn_tick_task(Arc::clone(&server));
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
                screen_ticks.abort();
                voice_ticks.abort();
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
    core.spawn_protection = cfg.server.spawn_protection;
    core.input_log = cfg.server.input_log;
    core.worldgen_gpu = cfg.server.worldgen_gpu;
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

fn supports_screen_sharing(platform: Option<mc_loader::LoaderPlatform>) -> bool {
    matches!(
        platform,
        Some(mc_loader::LoaderPlatform::Native | mc_loader::LoaderPlatform::Fabric)
    )
}

#[cfg(test)]
#[path = "server/tests.rs"]
mod tests;

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
