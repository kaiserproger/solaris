//! Edge v2 server configuration: TOML `--config server.toml` for `solaris-v2`.
//!
//! Reads the server configuration shared with the harness, restricted to the
//! keys the edge actually uses. Unknown keys are ignored with a one-line
//! `tracing::warn!` per file (never a startup error).

use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::warn;

/// Full edge configuration, loaded from one TOML file.
#[derive(Debug, Clone, Deserialize)]
pub struct EdgeConfig {
    /// World identity: name, MOTD, player caps, view distances.
    #[serde(default)]
    pub server: ServerSection,
    /// Listener address and port.
    #[serde(default)]
    pub network: NetworkSection,
    /// World directory + seed for the interim dev backend.
    #[serde(default)]
    pub data: DataSection,
    /// Offline-mode flag (online-mode verification is a later milestone).
    #[serde(default)]
    pub auth: AuthSection,
    /// Optional read-only operator dashboard.
    #[serde(default)]
    pub dashboard: DashboardSection,
    /// Optional authenticated Minecraft Server Management Protocol endpoint.
    #[serde(default)]
    pub msmp: MsmpSection,
    /// Strict component package deployment; absent means no plugins.
    #[serde(default)]
    pub plugins: PluginsSection,
    /// Anti-xray: hide fully enclosed ores from chunk packets.
    #[serde(default)]
    pub anti_xray: AntiXraySection,
    /// Play packet and native movement abuse protection.
    #[serde(default)]
    pub anti_cheat: AntiCheatSection,
    /// Finer block-damage switches under the vanilla `mobGriefing` gamerule.
    #[serde(default)]
    pub mob_griefing: MobGriefingSection,
    /// Attributed block/container history and `/co` rollback.
    #[serde(default)]
    pub block_log: BlockLogSection,
    /// Access lists live beside the explicit config, otherwise in the cwd.
    #[serde(skip, default = "default_server_dir")]
    pub(crate) server_dir: PathBuf,
    /// Upstream proxy trust: Velocity modern forwarding.
    #[serde(default)]
    pub proxy: ProxySection,
    /// Far-terrain LOD for the native Solaris client.
    #[serde(default)]
    pub lod: LodSection,
    /// Optional Distant Horizons protocol-16 service for Java clients.
    #[serde(default)]
    pub distant_horizons: DistantHorizonsSection,
    /// Opt-in operator-only in-game terminal.
    #[serde(default)]
    pub computer: crate::computer::ComputerConfig,
    /// Proximity voice relay for native clients.
    #[serde(default)]
    pub voice: crate::voice::VoiceConfig,
    /// Startup-only import; explicit TOML leaves override imported properties.
    #[serde(default)]
    pub properties: PropertiesSection,
    #[serde(default)]
    pub rcon: RconSection,
    #[serde(default)]
    pub query: QuerySection,
    #[serde(default)]
    pub resource_pack: ResourcePackSection,
}

impl Default for EdgeConfig {
    fn default() -> Self {
        toml::from_str("").expect("empty TOML parses into all-default sections")
    }
}
fn default_server_dir() -> PathBuf {
    PathBuf::from(".")
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PropertiesSection {
    pub file: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RconSection {
    pub enabled: bool,
    pub bind_address: String,
    pub port: u16,
    pub password: String,
}

impl Default for RconSection {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_address: default_bind_address(),
            port: 25575,
            password: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct QuerySection {
    pub enabled: bool,
    pub port: u16,
}

impl Default for QuerySection {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 25565,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ResourcePackSection {
    pub url: String,
    pub sha1: String,
    pub required: bool,
    /// JSON text component; empty omits the prompt.
    pub prompt: String,
    pub id: Option<uuid::Uuid>,
}

/// `[distant_horizons]`: server-sampled terrain, independent of native `[lod]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DistantHorizonsSection {
    pub enabled: bool,
    /// Generation and synchronization distance, in chunks.
    pub radius_chunks: u16,
}

impl Default for DistantHorizonsSection {
    fn default() -> Self {
        Self {
            enabled: false,
            radius_chunks: 128,
        }
    }
}

/// `[server]`: world identity and player/distance caps.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerSection {
    /// Display name shown on the status ping and dashboard.
    #[serde(default = "default_name")]
    pub name: String,
    /// Message of the day shown on the status ping and dashboard.
    #[serde(default = "default_motd")]
    pub motd: String,
    /// Maximum concurrent Play sessions admitted.
    #[serde(default = "default_max_players")]
    pub max_players: u32,
    /// Upper bound on each client's negotiated chunk view radius.
    #[serde(default = "default_view_distance")]
    pub view_distance: u8,
    /// Gameplay simulation radius (mob ticking, random ticks, spawning).
    /// Residency follows view (+margin) ∪ active ∪ tickets ∪ dirty pins;
    /// simulation only decides which resident chunks are *active*.
    #[serde(default = "default_simulation_distance")]
    pub simulation_distance: u8,
    /// Vanilla `accepts-transfers`: admit clients that arrive with the
    /// handshake intent `Transfer` (a previous server sent them here).
    #[serde(default)]
    pub accepts_transfers: bool,
    /// Vanilla `pause-when-empty-seconds`: stop advancing world simulation
    /// after this many seconds with no online player. Zero or negative never
    /// pauses, as in the vanilla dedicated server. Network, console and saves
    /// keep running, and the first join resumes ticking.
    #[serde(default = "default_pause_when_empty_seconds")]
    pub pause_when_empty_seconds: i32,
    /// Vanilla enable-command-block execution gate.
    #[serde(default)]
    pub enable_command_block: bool,
    /// Vanilla Overworld protected spawn radius; inactive when ops.json is empty.
    #[serde(default = "default_spawn_protection")]
    pub spawn_protection: u32,
    /// Opt-in per-tick input log under `<world_dir>/input-log/` for replay
    /// from a checkpoint (see `docs/OPERATING.md`). Off by default.
    #[serde(default)]
    pub input_log: bool,
    /// Evaluate Overworld chunk density on the GPU, batched across chunk
    /// workers. Needs a build with GPU worldgen (feature `gpu`) and a
    /// hardware adapter; otherwise startup logs why and the CPU does it.
    #[serde(default)]
    pub worldgen_gpu: bool,
    /// Initial enforcement policy when whitelist.enabled has not been persisted.
    #[serde(default)]
    pub whitelist: Option<bool>,
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            name: default_name(),
            motd: default_motd(),
            max_players: default_max_players(),
            view_distance: default_view_distance(),
            simulation_distance: default_simulation_distance(),
            accepts_transfers: false,
            pause_when_empty_seconds: default_pause_when_empty_seconds(),
            enable_command_block: false,
            spawn_protection: default_spawn_protection(),
            input_log: false,
            worldgen_gpu: false,
            whitelist: None,
        }
    }
}

/// `[network]`: TCP listener.
#[derive(Debug, Clone, Deserialize)]
pub struct NetworkSection {
    /// IP to bind (default loopback; `0.0.0.0` accepts remote IPv4).
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    /// TCP port for Minecraft clients.
    #[serde(default = "default_port")]
    pub port: u16,
    /// Minimum milliseconds between two accepted connections from one address.
    /// `0` (default) disables the throttle.
    #[serde(default)]
    pub connection_throttle_ms: u64,
    /// Maximum concurrent connections from one address. `0` (default) is
    /// unlimited.
    #[serde(default)]
    pub max_connections_per_ip: u32,
}

impl Default for NetworkSection {
    fn default() -> Self {
        Self {
            bind_address: default_bind_address(),
            port: default_port(),
            connection_throttle_ms: 0,
            max_connections_per_ip: 0,
        }
    }
}

/// `[data]`: world storage for the interim dev backend.
#[derive(Debug, Clone, Deserialize)]
pub struct DataSection {
    /// World directory (`region/`, `players/`, `journal/` live here).
    #[serde(default = "default_world_dir")]
    pub world_dir: PathBuf,
    /// World seed for chunk generation.
    #[serde(default)]
    pub seed: i64,
    /// Content-cache root; empty discovers XDG/home or checkout data/vanilla.
    #[serde(default = "default_vanilla_data_dir")]
    pub vanilla_data_dir: PathBuf,
    /// Directory receiving console `backup` copies of the running world;
    /// the command is refused while unset.
    #[serde(default)]
    pub backup_dir: Option<PathBuf>,
}

impl Default for DataSection {
    fn default() -> Self {
        Self {
            world_dir: default_world_dir(),
            seed: 0,
            vanilla_data_dir: default_vanilla_data_dir(),
            backup_dir: None,
        }
    }
}

/// `[auth]`: authentication mode.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AuthSection {
    /// `false` (default): offline mode, no Mojang verification.
    #[serde(default)]
    pub online_mode: bool,
    /// `false` (default): native sessions play immediately unless an
    /// auth-capable plugin package is deployed, which forces the gate on.
    /// `true`: new native sessions start locked and fail closed until the
    /// auth plugin unlocks them via `unlock_session`; startup fails without one.
    #[serde(default)]
    pub require_plugin_login: bool,
}

/// `[proxy]`: upstream proxy trust for player-info forwarding.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProxySection {
    /// `none` (default) or `velocity` modern forwarding.
    #[serde(default)]
    pub mode: ProxyMode,
    /// Velocity `forwarding.secret` contents, required in velocity mode.
    #[serde(default)]
    pub secret: String,
}

/// Which forwarding scheme the listener trusts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    /// Direct clients; no forwarding data is trusted.
    #[default]
    None,
    /// Velocity modern forwarding (HMAC-SHA256 login plugin message).
    Velocity,
}

/// `[plugins]`: Wasmtime component packages, resolved against the process cwd.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PluginsSection {
    pub directory: Option<PathBuf>,
}

/// `[anti_xray]`: engine mode 1 — ores fully enclosed by opaque blocks are
/// sent as the surrounding rock until a neighbouring block is removed.
/// The End has no ores and is not configurable.
#[derive(Debug, Clone, Deserialize)]
pub struct AntiXraySection {
    /// Hide enclosed overworld ores (default on).
    #[serde(default = "default_true")]
    pub overworld: bool,
    /// Hide enclosed nether gold/quartz ore and ancient debris (default on).
    #[serde(default = "default_true")]
    pub nether: bool,
}

impl Default for AntiXraySection {
    fn default() -> Self {
        Self {
            overworld: true,
            nether: true,
        }
    }
}

/// `[lod]`: far terrain beyond the view distance for the native client.
///
/// The native client keeps summaries of every chunk it received and asks for
/// tiles beyond its view distance: stored chunks answer with their top block
/// (player edits after the last stored image are not shown), never-generated
/// columns with the router surface the generator will produce there.
#[derive(Debug, Clone, Deserialize)]
pub struct LodSection {
    /// Answer native-client tile requests (default on).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Largest horizontal distance, in blocks, from the player to a served
    /// tile (default 8192).
    #[serde(default = "default_lod_max_distance")]
    pub max_distance: u32,
    /// Tiles one connection may have queued or in progress (default 4).
    #[serde(default = "default_lod_inflight")]
    pub max_inflight_tiles: u16,
    /// Blocking threads computing tiles for all connections together (default 1).
    #[serde(default = "default_lod_workers")]
    pub workers: usize,
    /// Send the terrain noise parameters so the client generates far terrain
    /// for never-generated regions on its GPU. Default off. Treat it as
    /// REVEALING THE WORLD SEED to every native client: the parameters are
    /// derived from the seed by a non-cryptographic generator, so recovering
    /// the seed (and with it ore, structure and slime-chunk positions) is a
    /// matter of attacker effort, not impossible.
    #[serde(default)]
    pub share_terrain_keys: bool,
}

impl Default for LodSection {
    fn default() -> Self {
        Self {
            enabled: true,
            max_distance: default_lod_max_distance(),
            max_inflight_tiles: default_lod_inflight(),
            workers: default_lod_workers(),
            share_terrain_keys: false,
        }
    }
}

fn default_lod_max_distance() -> u32 {
    8192
}

fn default_lod_inflight() -> u16 {
    4
}

fn default_lod_workers() -> usize {
    1
}

fn default_true() -> bool {
    true
}

/// `[anti_cheat]`: play packet and authoritative movement abuse limits.
#[derive(Debug, Clone, Deserialize)]
pub struct AntiCheatSection {
    /// Refilled tokens per second for each individual Play packet id.
    #[serde(default = "default_packet_rate_per_second")]
    pub packet_rate_per_second: u32,
    /// Initial and maximum token count for each individual Play packet id.
    #[serde(default = "default_packet_burst")]
    pub packet_burst: u32,
    /// Consecutive unsupported non-descending movement ticks before kick.
    /// Zero disables the native airtime check.
    #[serde(default = "default_max_airtime_ticks")]
    pub max_airtime_ticks: u32,
}

impl Default for AntiCheatSection {
    fn default() -> Self {
        Self {
            packet_rate_per_second: default_packet_rate_per_second(),
            packet_burst: default_packet_burst(),
            max_airtime_ticks: default_max_airtime_ticks(),
        }
    }
}

/// `[mob_griefing]`: per-mob switches layered under the vanilla `mobGriefing`
/// gamerule, which stays the master gate. Vanilla 26.1.2 exposes no finer
/// gamerules, so they are server configuration. Every switch defaults on.
///
/// ```toml
/// [mob_griefing]
/// creeper_block_damage = false   # blast still hurts entities
/// villager_farming = false       # farmers stop harvesting crops
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct MobGriefingSection {
    /// Creeper blast removes blocks.
    #[serde(default = "default_true")]
    pub creeper_block_damage: bool,
    /// Ghast fireball blast removes blocks.
    #[serde(default = "default_true")]
    pub ghast_block_damage: bool,
    /// Wither spawn blast, skull blasts and the hurt-triggered block break.
    #[serde(default = "default_true")]
    pub wither_block_damage: bool,
    /// Villager crop harvesting and replanting.
    #[serde(default = "default_true")]
    pub villager_farming: bool,
    /// Sheared sheep eating grass into dirt while their wool regrows.
    #[serde(default = "default_true")]
    pub sheep_grazing: bool,
}

impl Default for MobGriefingSection {
    fn default() -> Self {
        Self {
            creeper_block_damage: true,
            ghast_block_damage: true,
            wither_block_damage: true,
            villager_farming: true,
            sheep_grazing: true,
        }
    }
}

/// `[block_log]`: CoreProtect-like attributed history behind `/co`.
#[derive(Debug, Clone, Deserialize)]
pub struct BlockLogSection {
    /// Capture player, mob, fire and plugin block changes and container
    /// transactions (default on). Retained history stays queryable when off.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Prune records older than this many days; zero keeps them until
    /// `max_records`.
    #[serde(default = "default_block_log_retention_days")]
    pub retention_days: u32,
    /// Per-dimension record cap; the oldest records are pruned first.
    #[serde(default = "default_block_log_max_records")]
    pub max_records: usize,
}

impl Default for BlockLogSection {
    fn default() -> Self {
        Self {
            enabled: true,
            retention_days: default_block_log_retention_days(),
            max_records: default_block_log_max_records(),
        }
    }
}

fn default_block_log_retention_days() -> u32 {
    30
}
fn default_block_log_max_records() -> usize {
    200_000
}

/// `[dashboard]`: optional operator dashboard and Prometheus scrape endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct DashboardSection {
    /// Master switch (default off).
    #[serde(default)]
    pub enabled: bool,
    /// Bind address (default loopback).
    #[serde(default = "default_dashboard_bind")]
    pub bind_address: String,
    /// HTTP port.
    #[serde(default = "default_dashboard_port")]
    pub port: u16,
    /// Non-loopback binds additionally require this flag; without it the
    /// dashboard refuses to bind a remote address.
    #[serde(default)]
    pub allow_remote: bool,
    /// Optional bearer token required by the Prometheus `/metrics` scrape.
    #[serde(default)]
    pub metrics_bearer_token: Option<String>,
}

impl Default for DashboardSection {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_address: default_dashboard_bind(),
            port: default_dashboard_port(),
            allow_remote: false,
            metrics_bearer_token: None,
        }
    }
}

/// `[msmp]`: authenticated JSON-RPC-over-WebSocket management endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct MsmpSection {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_dashboard_bind")]
    pub bind_address: String,
    #[serde(default = "default_msmp_port")]
    pub port: u16,
    /// Required when enabled. It is checked as an HTTP Bearer token before
    /// accepting the WebSocket upgrade.
    #[serde(default)]
    pub bearer_token: String,
    #[serde(default)]
    pub allow_remote: bool,
}

impl Default for MsmpSection {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_address: default_dashboard_bind(),
            port: default_msmp_port(),
            bearer_token: String::new(),
            allow_remote: false,
        }
    }
}

fn default_msmp_port() -> u16 {
    25585
}

fn default_name() -> String {
    "solaris-edge".to_owned()
}
fn default_motd() -> String {
    "Solaris edge v2".to_owned()
}
fn default_max_players() -> u32 {
    20
}
fn default_view_distance() -> u8 {
    8
}
fn default_simulation_distance() -> u8 {
    6
}
fn default_packet_rate_per_second() -> u32 {
    500
}
fn default_packet_burst() -> u32 {
    1_000
}
fn default_max_airtime_ticks() -> u32 {
    80
}
fn default_pause_when_empty_seconds() -> i32 {
    // Vanilla 26.1.2 `server.properties` default.
    60
}
fn default_spawn_protection() -> u32 {
    16
}
fn default_bind_address() -> String {
    "127.0.0.1".to_owned()
}
fn default_port() -> u16 {
    25565
}
fn default_world_dir() -> PathBuf {
    PathBuf::from("world")
}
fn default_vanilla_data_dir() -> PathBuf {
    PathBuf::new()
}
fn default_dashboard_bind() -> String {
    "127.0.0.1".to_owned()
}
fn default_dashboard_port() -> u16 {
    8080
}

impl EdgeConfig {
    /// Load from a TOML file. Unknown keys are ignored with a warning naming
    /// the key path; missing sections take the defaults above. Relative
    /// `data.world_dir` / `data.vanilla_data_dir` resolve against the
    /// process working directory. The harness launches with `cwd` = repo root,
    /// so `data/vanilla` means the checkout's sidecar regardless of where the
    /// config file lives.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        crate::server_properties::load(&text, path)
    }

    /// Parse TOML text; `origin` names the file in log lines (or `"config"`).
    /// Unknown keys are collected via a flatten catch-all and warned about.
    pub fn from_toml(text: &str, origin: Option<&Path>) -> anyhow::Result<Self> {
        let origin_name = origin.map_or_else(|| "config".to_owned(), |p| p.display().to_string());
        #[derive(Deserialize)]
        struct WithUnknown {
            #[serde(flatten)]
            known: EdgeConfig,
            #[serde(flatten)]
            unknown: toml::Table,
        }
        // `toml::Table` swallows every top-level key, so subtract the known
        // section names to find genuinely unknown ones.
        let parsed: WithUnknown = toml::from_str(text)?;
        for key in parsed.unknown.keys() {
            if !KNOWN_SECTIONS.contains(&key.as_str()) {
                warn!(origin = %origin_name, key = %key, "ignoring unknown config section");
            }
        }
        // Nested unknown keys: re-parse each known section strictly would
        // reject; instead walk the raw table and warn on unexpected leaves.
        if let Ok(table) = toml::from_str::<toml::Table>(text) {
            log_unknown_leaves(&origin_name, &table);
        }
        let mut cfg = parsed.known;
        cfg.server_dir = origin
            .and_then(Path::parent)
            .filter(|path| !path.as_os_str().is_empty())
            .map_or_else(default_server_dir, Path::to_path_buf);
        // Relative data paths resolve against the process working directory.
        // Absolute paths pass through untouched.
        let _ = origin;
        if cfg.server.view_distance == 0 {
            cfg.server.view_distance = default_view_distance();
        }
        if cfg.server.simulation_distance == 0 {
            cfg.server.simulation_distance = default_simulation_distance();
        }
        if cfg.proxy.mode == ProxyMode::Velocity {
            if cfg.proxy.secret.is_empty() {
                anyhow::bail!(
                    "proxy.mode = \"velocity\" requires a non-empty proxy.secret \
                     (the contents of Velocity's forwarding.secret)"
                );
            }
            if cfg.auth.online_mode {
                anyhow::bail!(
                    "proxy.mode = \"velocity\" requires auth.online_mode = false: \
                     the proxy authenticates players"
                );
            }
        }
        if cfg.msmp.enabled && cfg.msmp.bearer_token.is_empty() {
            anyhow::bail!("msmp.enabled = true requires a non-empty msmp.bearer_token");
        }
        anyhow::ensure!(
            !cfg.rcon.enabled || !cfg.rcon.password.is_empty(),
            "rcon.enabled requires a non-empty rcon.password"
        );
        anyhow::ensure!(
            !cfg.rcon.password.contains('\0'),
            "rcon.password cannot contain NUL"
        );
        crate::resource_pack::validate(&cfg.resource_pack)?;
        anyhow::ensure!(
            (1..=128).contains(&cfg.distant_horizons.radius_chunks),
            "distant_horizons.radius_chunks must be in 1..=128"
        );
        Ok(cfg)
    }

    /// Socket address the Minecraft listener binds.
    pub fn bind_addr(&self) -> String {
        format!("{}:{}", self.network.bind_address, self.network.port)
    }
}

/// Every top-level section the parser understands.
const KNOWN_SECTIONS: &[&str] = &[
    "server",
    "network",
    "data",
    "auth",
    "proxy",
    "block_log",
    "mob_griefing",
    "dashboard",
    "msmp",
    "plugins",
    "anti_xray",
    "anti_cheat",
    "lod",
    "distant_horizons",
    "computer",
    "voice",
    "properties",
    "rcon",
    "query",
    "resource_pack",
];

/// Unknown leaf keys (`section.key`) under a known section, depth-first.
fn unknown_leaves(table: &toml::Table) -> Vec<String> {
    let mut unknown = Vec::new();
    const KNOWN: &[(&str, &[&str])] = &[
        (
            "server",
            &[
                "name",
                "motd",
                "max_players",
                "view_distance",
                "simulation_distance",
                "accepts_transfers",
                "pause_when_empty_seconds",
                "enable_command_block",
                "spawn_protection",
                "input_log",
                "worldgen_gpu",
                "whitelist",
            ],
        ),
        (
            "network",
            &[
                "bind_address",
                "port",
                "connection_throttle_ms",
                "max_connections_per_ip",
            ],
        ),
        (
            "data",
            &["world_dir", "seed", "vanilla_data_dir", "backup_dir"],
        ),
        ("auth", &["online_mode", "require_plugin_login"]),
        ("proxy", &["mode", "secret"]),
        ("plugins", &["directory"]),
        ("anti_xray", &["overworld", "nether"]),
        (
            "computer",
            &["enabled", "local_only", "command", "working_directory"],
        ),
        ("voice", &["enabled", "range_blocks"]),
        (
            "anti_cheat",
            &[
                "packet_rate_per_second",
                "packet_burst",
                "max_airtime_ticks",
            ],
        ),
        (
            "mob_griefing",
            &[
                "creeper_block_damage",
                "ghast_block_damage",
                "wither_block_damage",
                "villager_farming",
                "sheep_grazing",
            ],
        ),
        ("block_log", &["enabled", "retention_days", "max_records"]),
        (
            "dashboard",
            &[
                "enabled",
                "bind_address",
                "port",
                "allow_remote",
                "metrics_bearer_token",
            ],
        ),
        (
            "msmp",
            &[
                "enabled",
                "bind_address",
                "port",
                "bearer_token",
                "allow_remote",
            ],
        ),
        (
            "lod",
            &[
                "enabled",
                "max_distance",
                "max_inflight_tiles",
                "workers",
                "share_terrain_keys",
            ],
        ),
        ("distant_horizons", &["enabled", "radius_chunks"]),
        ("properties", &["file"]),
        ("rcon", &["enabled", "bind_address", "port", "password"]),
        ("query", &["enabled", "port"]),
        (
            "resource_pack",
            &["url", "sha1", "required", "prompt", "id"],
        ),
    ];
    for (section, keys) in KNOWN {
        let Some(toml::Value::Table(inner)) = table.get(*section) else {
            continue;
        };
        for key in inner.keys() {
            if !keys.contains(&key.as_str()) {
                unknown.push(format!("{section}.{key}"));
            }
        }
    }
    unknown
}

/// Warn once per unknown leaf key under a known section.
fn log_unknown_leaves(origin: &str, table: &toml::Table) {
    for key in unknown_leaves(table) {
        warn!(origin = %origin, key = %key, "ignoring unknown config key");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_takes_defaults() {
        let cfg = EdgeConfig::from_toml("", None).unwrap();
        assert_eq!(cfg.server.name, "solaris-edge");
        assert_eq!(cfg.network.port, 25565);
        assert!(!cfg.auth.online_mode);
        assert!(!cfg.auth.require_plugin_login);
        assert!(!cfg.dashboard.enabled);
    }

    #[test]
    fn pause_when_empty_defaults_to_vanilla_sixty_and_parses() {
        let default = EdgeConfig::from_toml("", None).unwrap();
        assert_eq!(default.server.pause_when_empty_seconds, 60);
        let disabled =
            EdgeConfig::from_toml("[server]\npause_when_empty_seconds = 0\n", None).unwrap();
        assert_eq!(disabled.server.pause_when_empty_seconds, 0);
        let short =
            EdgeConfig::from_toml("[server]\npause_when_empty_seconds = 5\n", None).unwrap();
        assert_eq!(short.server.pause_when_empty_seconds, 5);
        let negative =
            EdgeConfig::from_toml("[server]\npause_when_empty_seconds = -1\n", None).unwrap();
        assert_eq!(negative.server.pause_when_empty_seconds, -1);
    }

    #[test]
    fn input_log_defaults_off_and_parses() {
        assert!(!EdgeConfig::from_toml("", None).unwrap().server.input_log);
        let on = EdgeConfig::from_toml("[server]\ninput_log = true\n", None).unwrap();
        assert!(on.server.input_log);
    }

    #[test]
    fn mob_griefing_switches_default_on_and_parse_off() {
        let default = EdgeConfig::from_toml("", None).unwrap();
        assert!(default.mob_griefing.creeper_block_damage);
        assert!(default.mob_griefing.ghast_block_damage);
        assert!(default.mob_griefing.wither_block_damage);
        assert!(default.mob_griefing.villager_farming);
        assert!(default.mob_griefing.sheep_grazing);

        let configured = EdgeConfig::from_toml(
            "[mob_griefing]\ncreeper_block_damage = false\nvillager_farming = false\n",
            None,
        )
        .unwrap();
        assert!(!configured.mob_griefing.creeper_block_damage);
        assert!(!configured.mob_griefing.villager_farming);
        assert!(configured.mob_griefing.ghast_block_damage);
        assert!(configured.mob_griefing.wither_block_damage);
        assert!(configured.mob_griefing.sheep_grazing);
    }

    #[test]
    fn anti_cheat_defaults_and_overrides_parse() {
        let default = EdgeConfig::from_toml("", None).unwrap();
        assert_eq!(default.anti_cheat.packet_rate_per_second, 500);
        assert_eq!(default.anti_cheat.packet_burst, 1_000);
        assert_eq!(default.anti_cheat.max_airtime_ticks, 80);

        let configured = EdgeConfig::from_toml(
            "[anti_cheat]\npacket_rate_per_second = 20\npacket_burst = 40\nmax_airtime_ticks = 8\n",
            None,
        )
        .unwrap();
        assert_eq!(configured.anti_cheat.packet_rate_per_second, 20);
        assert_eq!(configured.anti_cheat.packet_burst, 40);
        assert_eq!(configured.anti_cheat.max_airtime_ticks, 8);
    }

    #[test]
    fn require_plugin_login_defaults_off_and_parses_on() {
        let off = EdgeConfig::from_toml("[auth]\nonline_mode = false\n", None).unwrap();
        assert!(!off.auth.require_plugin_login);
        let on = EdgeConfig::from_toml("[auth]\nrequire_plugin_login = true\n", None).unwrap();
        assert!(on.auth.require_plugin_login);
    }

    #[test]
    fn proxy_defaults_to_none_and_velocity_requires_a_secret() {
        let default = EdgeConfig::from_toml("", None).unwrap();
        assert_eq!(default.proxy.mode, ProxyMode::None);
        assert!(default.proxy.secret.is_empty());

        let velocity =
            EdgeConfig::from_toml("[proxy]\nmode = \"velocity\"\nsecret = \"abc\"\n", None)
                .unwrap();
        assert_eq!(velocity.proxy.mode, ProxyMode::Velocity);
        assert_eq!(velocity.proxy.secret, "abc");

        let missing = EdgeConfig::from_toml("[proxy]\nmode = \"velocity\"\n", None);
        assert!(
            missing.is_err(),
            "velocity without a secret must fail closed"
        );

        let online = EdgeConfig::from_toml(
            "[auth]\nonline_mode = true\n[proxy]\nmode = \"velocity\"\nsecret = \"abc\"\n",
            None,
        );
        assert!(online.is_err(), "velocity authenticates instead of Mojang");

        assert!(EdgeConfig::from_toml("[proxy]\nmode = \"bungee\"\n", None).is_err());
    }

    #[test]
    fn accepts_transfers_defaults_off_and_parses_on() {
        let off = EdgeConfig::from_toml("", None).unwrap();
        assert!(!off.server.accepts_transfers);
        let on = EdgeConfig::from_toml("[server]\naccepts_transfers = true\n", None).unwrap();
        assert!(on.server.accepts_transfers);
    }

    #[test]
    fn known_keys_parse() {
        let cfg = EdgeConfig::from_toml(
            "[server]\nname = \"x\"\nmax_players = 5\nview_distance = 4\nsimulation_distance = 3\n\
             [network]\nport = 25566\n[data]\nseed = 81\n[auth]\nonline_mode = false\n\
             [dashboard]\nenabled = true\nport = 8090\n",
            None,
        )
        .unwrap();
        assert_eq!(cfg.server.name, "x");
        assert_eq!(cfg.server.max_players, 5);
        assert_eq!(cfg.server.view_distance, 4);
        assert_eq!(cfg.data.seed, 81);
        assert!(cfg.dashboard.enabled);
        assert_eq!(cfg.dashboard.port, 8090);
    }

    #[test]
    fn lod_section_parses_and_shares_no_keys_by_default() {
        let default = EdgeConfig::from_toml("", None).unwrap().lod;
        assert!(default.enabled && !default.share_terrain_keys);
        assert_eq!(
            (
                default.max_distance,
                default.max_inflight_tiles,
                default.workers
            ),
            (8192, 4, 1)
        );
        let cfg = EdgeConfig::from_toml(
            "[lod]\nenabled = false\nmax_distance = 1024\nmax_inflight_tiles = 2\n\
             workers = 3\nshare_terrain_keys = true\n",
            None,
        )
        .unwrap()
        .lod;
        assert!(!cfg.enabled && cfg.share_terrain_keys);
        assert_eq!(
            (cfg.max_distance, cfg.max_inflight_tiles, cfg.workers),
            (1024, 2, 3)
        );
    }

    #[test]
    fn enabled_msmp_requires_a_bearer_token() {
        assert!(EdgeConfig::from_toml("[msmp]\nenabled = true\n", None).is_err());
        let cfg = EdgeConfig::from_toml(
            "[msmp]\nenabled = true\nport = 25586\nbearer_token = \"secret\"\n",
            None,
        )
        .unwrap();
        assert!(cfg.msmp.enabled);
        assert_eq!(cfg.msmp.port, 25586);
        assert_eq!(cfg.msmp.bearer_token, "secret");
    }

    #[test]
    fn distant_horizons_defaults_off_and_radius_is_server_bounded() {
        let cfg = EdgeConfig::from_toml("", None).unwrap();
        assert!(!cfg.distant_horizons.enabled);
        assert_eq!(cfg.distant_horizons.radius_chunks, 128);
        let cfg = EdgeConfig::from_toml("[distant_horizons]\nenabled=true\nradius_chunks=64", None)
            .unwrap();
        assert!(cfg.distant_horizons.enabled);
        assert_eq!(cfg.distant_horizons.radius_chunks, 64);
        for radius in [0, 129, 65535] {
            assert!(
                EdgeConfig::from_toml(&format!("[distant_horizons]\nradius_chunks={radius}"), None)
                    .is_err()
            );
        }
    }

    #[test]
    fn unknown_keys_ignored_not_rejected() {
        let cfg = EdgeConfig::from_toml(
            "[server]\nname = \"x\"\nfuture_flag = 1\n[plugins]\ndirectory = \"p\"\n",
            None,
        )
        .unwrap();
        assert_eq!(cfg.server.name, "x");
    }

    #[test]
    fn network_address_limits_parse_and_example_names_only_known_keys() {
        let cfg = EdgeConfig::from_toml(
            "[network]\nconnection_throttle_ms = 750\nmax_connections_per_ip = 2\n",
            None,
        )
        .unwrap();
        assert_eq!(cfg.network.connection_throttle_ms, 750);
        assert_eq!(cfg.network.max_connections_per_ip, 2);

        let text =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../example.toml"))
                .unwrap();
        let table: toml::Table = toml::from_str(&text).unwrap();
        assert_eq!(
            unknown_leaves(&table),
            Vec::<String>::new(),
            "example.toml must document only known keys"
        );
        for section in table.keys() {
            assert!(
                KNOWN_SECTIONS.contains(&section.as_str()),
                "example.toml documents unknown section {section}"
            );
        }
        EdgeConfig::from_toml(&text, None).unwrap();
    }

    #[test]
    fn relative_paths_resolve_against_process_cwd_like_v1() {
        // v1 parity: relative data paths are NOT joined onto the config
        // file's directory; they resolve against the process cwd.
        let cfg = EdgeConfig::from_toml(
            "[data]\nworld_dir = \"w\"\n",
            Some(Path::new("/srv/mc/server.toml")),
        )
        .unwrap();
        assert_eq!(cfg.data.world_dir, PathBuf::from("w"));
        assert_eq!(cfg.server_dir, PathBuf::from("/srv/mc"));
        assert_eq!(EdgeConfig::default().server_dir, PathBuf::from("."));
    }
}
