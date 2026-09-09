//! # mc-server
//!
//! Main server binary that ties the Solaris engine together.
//!
//! Part of the Solaris engine.

use std::collections::BTreeSet;
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, bail};
use mc_data::VanillaData;
use mc_data::biomes::BiomeSpawnRules;
use mc_data::block_facts::BlockFactsTable;
use mc_data::entity_types::EntityTypeRegistry;
use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_data::loot::LootTables;
use mc_data::recipes::Recipe;
use mc_data::tags::TagsData;
use mc_net::WorldHandle;
use mc_world::BlockRegistry;
use serde::{Deserialize, Serialize};

pub mod dashboard;
pub mod dashboard_stats;
#[cfg(test)]
#[path = "dashboard_tests.rs"]
mod dashboard_tests;
pub mod startup_data;

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const MAX_ACCESS_CONTROL_FILE_BYTES: u64 = 1024 * 1024;
const MAX_ACCESS_CONTROL_FILE_ENTRIES: usize = 4096;

/// Top-level server configuration loaded from a TOML file at startup.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub server: ServerSection,
    pub network: NetworkSection,
    #[serde(default)]
    pub data: DataSection,
    #[serde(default)]
    pub chunk_pipeline: ChunkPipelineSection,
    #[serde(default)]
    pub simulation: SimulationSection,
    #[serde(default)]
    pub admin: AdminSection,
    #[serde(default)]
    pub auth: AuthSection,
    #[serde(default)]
    pub autoscale: AutoscaleSection,
    #[serde(default)]
    pub plugins: PluginSection,
    #[serde(default)]
    pub dashboard: DashboardSection,
}

/// Identity-level server settings.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerSection {
    pub name: String,
    pub motd: String,
    #[serde(default = "default_max_players")]
    pub max_players: u32,
    #[serde(default = "default_view_distance")]
    pub view_distance: i32,
    #[serde(default = "default_view_distance")]
    pub simulation_distance: i32,
}

/// Network-level server settings.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkSection {
    pub bind_address: String,
    pub port: u16,
}

/// Optional first-party operator dashboard settings. Default-off and
/// loopback-bound; exposing it beyond localhost requires an explicit
/// acknowledgement because the dashboard is read-only and unauthenticated.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DashboardSection {
    pub enabled: bool,
    pub bind_address: String,
    pub port: u16,
    pub allow_remote: bool,
}

impl Default for DashboardSection {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_address: "127.0.0.1".to_owned(),
            port: 8080,
            allow_remote: false,
        }
    }
}

impl DashboardSection {
    /// Validate and normalize the dashboard listener endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error when `bind_address` is not a valid IP address or a
    /// non-loopback bind was requested without `allow_remote = true`.
    pub fn validate(&self) -> Result<SocketAddr, String> {
        let ip: IpAddr = self.bind_address.parse().map_err(|_| {
            format!(
                "dashboard.bind_address `{}` is not a valid IP address",
                self.bind_address
            )
        })?;
        if !ip.is_loopback() && !self.allow_remote {
            return Err(format!(
                "dashboard.bind_address {ip} is not loopback; set dashboard.allow_remote = true to deliberately expose the unauthenticated dashboard"
            ));
        }
        Ok(SocketAddr::new(ip, self.port))
    }
}

/// `world_dir` is the on-disk world save the server reads chunks from
/// at runtime. The library keeps it optional for synthetic network tests,
/// but the `mc-server` binary requires it for both `--check` and `serve`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataSection {
    #[serde(default)]
    pub world_dir: Option<PathBuf>,
    /// Optional local vanilla data sidecar root. Mojang-owned files stay outside
    /// the repo; when set, Solaris treats the sidecar as authoritative and
    /// requires supported registries, tags, reports, and simple loot data.
    #[serde(default)]
    pub vanilla_data_dir: Option<PathBuf>,
    /// World seed for the M7 terrain generator. Defaults to `0` —
    /// every run starts on the same terrain unless this is overridden.
    /// Operators bumping this between runs will see fresh terrain in
    /// previously-unflushed chunks; chunks already written to `.mca`
    /// keep their old contents (the on-disk slot wins).
    #[serde(default)]
    pub seed: i64,
    #[serde(default)]
    pub worldgen_mode: WorldgenMode,
    /// Lowest generated world Y, inclusive.
    #[serde(default = "default_dimension_min_y")]
    pub min_y: i32,
    /// Generated world height in blocks.
    #[serde(default = "default_dimension_height")]
    pub height: i32,
}

impl Default for DataSection {
    fn default() -> Self {
        Self {
            world_dir: None,
            vanilla_data_dir: None,
            seed: 0,
            worldgen_mode: WorldgenMode::default(),
            min_y: default_dimension_min_y(),
            height: default_dimension_height(),
        }
    }
}

impl DataSection {
    /// Validate the configured vertical range against the chunk format.
    ///
    /// # Errors
    ///
    /// Returns an error when the range is not section-aligned, is empty,
    /// overflows, or cannot be represented by the current heightmap format.
    pub fn chunk_geometry(&self) -> Result<mc_world::ChunkGeometry, String> {
        mc_world::ChunkGeometry::new(self.min_y, self.height).ok_or_else(|| {
            format!(
                "data.min_y ({}) and data.height ({}) must define a positive, 16-block-aligned range supported by the chunk heightmap format",
                self.min_y, self.height
            )
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorldgenMode {
    VanillaLike,
    #[default]
    TellusLike,
}

impl WorldgenMode {
    #[must_use]
    pub fn to_worldgen(self) -> mc_worldgen::WorldgenMode {
        match self {
            Self::VanillaLike => mc_worldgen::WorldgenMode::VanillaLike,
            Self::TellusLike => {
                mc_worldgen::WorldgenMode::TellusLike(mc_worldgen::TellusWorldgenSettings::default())
            }
        }
    }
}

/// Chunk preparation, worker, and cache policy. M13 moves chunk work out
/// of the Play socket task in stages; these settings are the stable
/// operator-facing surface for that pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChunkPipelineSection {
    #[serde(default = "default_chunk_send_rate")]
    pub chunk_send_rate: u32,
    #[serde(default = "default_chunk_load_rate")]
    pub chunk_load_rate: u32,
    #[serde(default = "default_chunk_generate_rate")]
    pub chunk_generate_rate: u32,
    #[serde(default)]
    pub chunk_prepare_budget_ms: u64,
    #[serde(default = "default_chunk_prepare_batch_size")]
    pub chunk_prepare_batch_size: usize,
    #[serde(default = "default_chunk_result_queue_size")]
    pub chunk_result_queue_size: usize,
    #[serde(default = "default_region_cache_size")]
    pub region_cache_size: usize,
    #[serde(default = "default_compression_threshold")]
    pub compression_threshold: i32,
    #[serde(default)]
    pub compression_level: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulationSection {
    #[serde(default = "default_random_tick_speed")]
    pub random_tick_speed: u32,
    #[serde(default = "default_save_interval_ticks")]
    pub save_interval_ticks: u64,
    #[serde(default = "default_friendly_spawn_interval_ticks")]
    pub friendly_spawn_interval_ticks: u64,
    #[serde(default = "default_hostile_spawn_interval_ticks")]
    pub hostile_spawn_interval_ticks: u64,
    #[serde(default = "default_friendly_spawn_chunk_budget")]
    pub friendly_spawn_chunk_budget: usize,
    #[serde(default = "default_hostile_spawn_chunk_budget")]
    pub hostile_spawn_chunk_budget: usize,
}
/// Optional external Luau plugins, loaded from a deployed plugin directory.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSection {
    #[serde(default)]
    pub directory: Option<PathBuf>,
    #[serde(default)]
    pub strict: bool,
    #[serde(default)]
    pub expected: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSection {
    #[serde(default)]
    pub operators: Vec<String>,
    #[serde(default)]
    pub operators_file: Option<PathBuf>,
    #[serde(default = "default_allow_local_dev_operators")]
    pub allow_local_dev_operators: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AuthSection {
    #[serde(default)]
    pub online_mode: bool,
    #[serde(default)]
    pub prevent_proxy_connections: bool,
    #[serde(default)]
    pub whitelist_enabled: bool,
    #[serde(default)]
    pub whitelist: Vec<String>,
    #[serde(default)]
    pub whitelist_file: Option<PathBuf>,
    #[serde(default)]
    pub banned_players: Vec<String>,
    #[serde(default)]
    pub banned_players_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccessControlLoadReport {
    pub files_loaded: usize,
    pub operator_identities: usize,
    pub whitelist_identities: usize,
    pub banned_identities: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperatorFileOperation {
    Add(String),
    Remove(String),
    List,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorFileResult {
    pub changed: bool,
    pub identities: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct AccessControlProfileEntry {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    uuid: Option<String>,
}

impl ServerConfig {
    /// Merge optional file-backed access-control profiles into the inline TOML policy.
    ///
    /// Relative paths are resolved from the directory containing `config_path`.
    /// When `admin.operators_file` is omitted, an existing `ops.json` beside the
    /// config is used for the operator profile. File entries use vanilla-style
    /// JSON objects with `name` and/or `uuid`; extra fields such as operator level
    /// or ban metadata are ignored deliberately.
    pub fn load_access_control_files(
        &mut self,
        config_path: &Path,
    ) -> anyhow::Result<AccessControlLoadReport> {
        let mut report = AccessControlLoadReport::default();

        let operators_file = self.admin.operators_file.clone().or_else(|| {
            let default = Path::new("ops.json");
            resolve_config_relative_path(config_path, default)
                .is_file()
                .then(|| default.to_path_buf())
        });
        if let Some(path) = operators_file {
            let entries = load_access_control_file(config_path, &path, "admin.operators_file")?;
            report.files_loaded += 1;
            report.operator_identities = entries.len();
            self.admin.operators.extend(entries);
        }
        if let Some(path) = self.auth.whitelist_file.clone() {
            let entries = load_access_control_file(config_path, &path, "auth.whitelist_file")?;
            report.files_loaded += 1;
            report.whitelist_identities = entries.len();
            self.auth.whitelist.extend(entries);
        }
        if let Some(path) = self.auth.banned_players_file.clone() {
            let entries = load_access_control_file(config_path, &path, "auth.banned_players_file")?;
            report.files_loaded += 1;
            report.banned_identities = entries.len();
            self.auth.banned_players.extend(entries);
        }

        Ok(report)
    }

    /// Add, remove, or list identities in the configured vanilla-style operator file.
    ///
    /// When `admin.operators_file` is absent, the management caller may supply
    /// the default `ops.json` path in memory; startup auto-loads that file when
    /// it exists beside the selected config. Add/remove preserve unknown profile
    /// metadata and normalize duplicate identities while writing deterministic JSON.
    pub fn manage_operator_file(
        &self,
        config_path: &Path,
        operation: OperatorFileOperation,
    ) -> anyhow::Result<OperatorFileResult> {
        let configured_path = self.admin.operators_file.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "operator management requires admin.operators_file in {}",
                config_path.display()
            )
        })?;
        let path = resolve_config_relative_path(config_path, configured_path);
        let requested_identity = match &operation {
            OperatorFileOperation::Add(raw) | OperatorFileOperation::Remove(raw) => {
                Some(normalize_operator_identity(raw)?)
            }
            OperatorFileOperation::List => None,
        };
        if !path.exists() && matches!(&operation, OperatorFileOperation::Add(_)) {
            write_operator_profiles(&path, &[])?;
        }
        let (_, values) =
            read_access_control_values(config_path, configured_path, "admin.operators_file")?;
        let (mut values, mut identities) =
            canonicalize_operator_profiles(values, "admin.operators_file", &path)?;

        match operation {
            OperatorFileOperation::List => {
                return Ok(OperatorFileResult {
                    changed: false,
                    identities: identities.into_iter().collect(),
                });
            }
            OperatorFileOperation::Add(_) => {
                let identity = requested_identity
                    .as_deref()
                    .expect("add operation has a normalized identity");
                if identities.insert(identity.to_owned()) {
                    let mut profile = serde_json::Map::new();
                    if uuid::Uuid::parse_str(identity).is_ok() {
                        profile.insert(
                            "uuid".to_owned(),
                            serde_json::Value::String(identity.to_owned()),
                        );
                    } else {
                        profile.insert(
                            "name".to_owned(),
                            serde_json::Value::String(identity.to_owned()),
                        );
                    }
                    values.push(serde_json::Value::Object(profile));
                } else {
                    return Ok(OperatorFileResult {
                        changed: false,
                        identities: identities.into_iter().collect(),
                    });
                }
            }
            OperatorFileOperation::Remove(_) => {
                let identity = requested_identity
                    .as_deref()
                    .expect("remove operation has a normalized identity");
                let mut removed = false;
                let mut retained = Vec::with_capacity(values.len());
                for mut value in values {
                    let Some(profile) = value.as_object_mut() else {
                        retained.push(value);
                        continue;
                    };
                    for key in ["name", "uuid"] {
                        let matches = profile
                            .get(key)
                            .and_then(serde_json::Value::as_str)
                            .and_then(|value| normalize_profile_identity(key, value).ok())
                            .is_some_and(|value| value == identity);
                        if matches {
                            profile.remove(key);
                            removed = true;
                        }
                    }
                    if profile.contains_key("name") || profile.contains_key("uuid") {
                        retained.push(value);
                    }
                }
                values = retained;
                identities.remove(identity);
                if !removed {
                    return Ok(OperatorFileResult {
                        changed: false,
                        identities: identities.into_iter().collect(),
                    });
                }
            }
        }

        write_operator_profiles(&path, &values)?;
        Ok(OperatorFileResult {
            changed: true,
            identities: identities.into_iter().collect(),
        })
    }
}

fn load_access_control_file(
    config_path: &Path,
    configured_path: &Path,
    field: &'static str,
) -> anyhow::Result<Vec<String>> {
    let path = resolve_config_relative_path(config_path, configured_path);
    let values = read_access_control_values(config_path, configured_path, field)?.1;
    let mut entries = BTreeSet::new();
    for (index, value) in values.into_iter().enumerate() {
        let profile = parse_access_control_profile(value, field, &path, index)?;
        let mut populated = false;
        if let Some(name) = profile.name {
            entries.insert(validate_access_control_name(&name, field, &path, index)?);
            populated = true;
        }
        if let Some(raw_uuid) = profile.uuid {
            let raw_uuid = raw_uuid.trim();
            if raw_uuid.is_empty() {
                bail!(
                    "{field} entry {index} from {} contains an empty uuid",
                    path.display()
                );
            }
            let uuid = uuid::Uuid::parse_str(raw_uuid).map_err(|_| {
                anyhow::anyhow!(
                    "{field} entry {index} from {} contains an invalid uuid",
                    path.display()
                )
            })?;
            entries.insert(uuid.to_string());
            populated = true;
        }
        if !populated {
            bail!(
                "{field} entry {index} from {} must contain name and/or uuid",
                path.display()
            );
        }
    }
    Ok(entries.into_iter().collect())
}

fn read_access_control_values(
    config_path: &Path,
    configured_path: &Path,
    field: &'static str,
) -> anyhow::Result<(PathBuf, Vec<serde_json::Value>)> {
    let path = resolve_config_relative_path(config_path, configured_path);
    let file = std::fs::File::open(&path)
        .with_context(|| format!("opening {field} from {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("reading {field} metadata from {}", path.display()))?;
    if !metadata.is_file() {
        bail!("{field} must point to a regular file: {}", path.display());
    }
    if metadata.len() > MAX_ACCESS_CONTROL_FILE_BYTES {
        bail!(
            "{field} exceeds the {} byte limit: {}",
            MAX_ACCESS_CONTROL_FILE_BYTES,
            path.display()
        );
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len().min(MAX_ACCESS_CONTROL_FILE_BYTES)).unwrap_or(0),
    );
    file.take(MAX_ACCESS_CONTROL_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading bounded {field} from {}", path.display()))?;
    if bytes.len() as u64 > MAX_ACCESS_CONTROL_FILE_BYTES {
        bail!(
            "{field} grew beyond the {} byte limit while reading: {}",
            MAX_ACCESS_CONTROL_FILE_BYTES,
            path.display()
        );
    }
    let values: Vec<serde_json::Value> = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing {field} JSON from {}", path.display()))?;
    if values.len() > MAX_ACCESS_CONTROL_FILE_ENTRIES {
        bail!(
            "{field} from {} contains {} entries; maximum is {}",
            path.display(),
            values.len(),
            MAX_ACCESS_CONTROL_FILE_ENTRIES
        );
    }
    Ok((path, values))
}

fn parse_access_control_profile(
    value: serde_json::Value,
    field: &'static str,
    path: &Path,
    index: usize,
) -> anyhow::Result<AccessControlProfileEntry> {
    serde_json::from_value(value)
        .with_context(|| format!("parsing {field} JSON entry {index} from {}", path.display()))
}

fn canonicalize_operator_profiles(
    values: Vec<serde_json::Value>,
    field: &'static str,
    path: &Path,
) -> anyhow::Result<(Vec<serde_json::Value>, BTreeSet<String>)> {
    let mut identities = BTreeSet::new();
    let mut canonical = Vec::with_capacity(values.len());
    for (index, mut value) in values.into_iter().enumerate() {
        let profile = parse_access_control_profile(value.clone(), field, path, index)?;
        let mut populated = false;
        if let Some(name) = profile.name.as_deref() {
            let identity = validate_access_control_name(name, field, path, index)?;
            populated = true;
            if !identities.insert(identity) {
                value
                    .as_object_mut()
                    .expect("access-control profile must be a JSON object")
                    .remove("name");
            }
        }
        if let Some(raw_uuid) = profile.uuid.as_deref() {
            let identity = normalize_profile_identity("uuid", raw_uuid).map_err(|message| {
                anyhow::anyhow!("{field} entry {index} from {} {message}", path.display())
            })?;
            populated = true;
            if !identities.insert(identity) {
                value
                    .as_object_mut()
                    .expect("access-control profile must be a JSON object")
                    .remove("uuid");
            }
        }
        if !populated {
            bail!(
                "{field} entry {index} from {} must contain name and/or uuid",
                path.display()
            );
        }
        let object = value
            .as_object()
            .expect("access-control profile must be a JSON object");
        if object.contains_key("name") || object.contains_key("uuid") {
            canonical.push(value);
        }
    }
    Ok((canonical, identities))
}

fn normalize_profile_identity(key: &str, raw: &str) -> Result<String, String> {
    if key == "uuid" {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err("contains an empty uuid".to_owned());
        }
        return uuid::Uuid::parse_str(raw)
            .map(|uuid| uuid.to_string())
            .map_err(|_| "contains an invalid uuid".to_owned());
    }
    let name = raw.trim();
    if !(3..=16).contains(&name.len())
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(
            "contains an invalid Minecraft username; expected 3..=16 ASCII letters, digits, or `_`"
                .to_owned(),
        );
    }
    Ok(name.to_ascii_lowercase())
}

fn normalize_operator_identity(raw: &str) -> anyhow::Result<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("operator identity cannot be empty; provide a Minecraft username or UUID");
    }
    if let Ok(uuid) = uuid::Uuid::parse_str(raw) {
        return Ok(uuid.to_string());
    }
    normalize_profile_identity("name", raw)
        .map_err(|message| anyhow::anyhow!("invalid operator identity `{raw}`: {message}"))
}

fn write_operator_profiles(path: &Path, values: &[serde_json::Value]) -> anyhow::Result<()> {
    let mut rendered = serde_json::to_vec_pretty(values).context("rendering operator file JSON")?;
    rendered.push(b'\n');
    std::fs::write(path, rendered)
        .with_context(|| format!("writing operator file {}", path.display()))
}

fn resolve_config_relative_path(config_path: &Path, configured_path: &Path) -> PathBuf {
    if configured_path.is_absolute() {
        return configured_path.to_path_buf();
    }
    config_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(configured_path)
}

fn validate_access_control_name(
    raw: &str,
    field: &'static str,
    path: &Path,
    index: usize,
) -> anyhow::Result<String> {
    normalize_profile_identity("name", raw).map_err(|message| {
        anyhow::anyhow!("{field} entry {index} from {} {message}", path.display())
    })
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutoscaleProfile {
    LowEnd,
    #[default]
    Balanced,
    HighEnd,
}

impl AutoscaleProfile {
    #[must_use]
    pub fn to_network(self) -> mc_net::AutoscaleProfile {
        match self {
            Self::LowEnd => mc_net::AutoscaleProfile::LowEnd,
            Self::Balanced => mc_net::AutoscaleProfile::Balanced,
            Self::HighEnd => mc_net::AutoscaleProfile::HighEnd,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoscaleSection {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub profile: AutoscaleProfile,
    #[serde(default)]
    pub min_view_distance: Option<i32>,
    #[serde(default)]
    pub max_view_distance: Option<i32>,
    #[serde(default)]
    pub target_tick_ms: Option<u64>,
    #[serde(default)]
    pub target_first_chunk_ms: Option<u64>,
    #[serde(default)]
    pub scale_down_after_ticks: Option<u32>,
    #[serde(default)]
    pub scale_up_after_ticks: Option<u32>,
}

impl Default for AutoscaleSection {
    fn default() -> Self {
        Self {
            enabled: true,
            profile: AutoscaleProfile::Balanced,
            min_view_distance: None,
            max_view_distance: None,
            target_tick_ms: None,
            target_first_chunk_ms: None,
            scale_down_after_ticks: None,
            scale_up_after_ticks: None,
        }
    }
}

impl AutoscaleSection {
    #[must_use]
    pub fn to_policy(&self, chunk_pipeline: &ChunkPipelineSection) -> mc_net::AutoscalePolicy {
        let mut policy = mc_net::AutoscalePolicy::for_profile(self.profile.to_network());
        if let Some(value) = self.min_view_distance {
            policy.min_view_distance = value;
        }
        if let Some(value) = self.max_view_distance {
            policy.max_view_distance = value;
        }
        if let Some(value) = self.target_tick_ms {
            policy.target_tick_ms = value;
        }
        if let Some(value) = self.target_first_chunk_ms {
            policy.target_first_chunk_ms = value;
        }
        if let Some(value) = self.scale_down_after_ticks {
            policy.scale_down_after_ticks = value;
        }
        if let Some(value) = self.scale_up_after_ticks {
            policy.scale_up_after_ticks = value;
        }

        policy.min_chunk_send_rate = policy
            .min_chunk_send_rate
            .min(chunk_pipeline.chunk_send_rate.max(1));
        policy.max_chunk_send_rate = policy
            .max_chunk_send_rate
            .max(chunk_pipeline.chunk_send_rate.max(1));
        policy.min_chunk_load_rate = policy
            .min_chunk_load_rate
            .min(chunk_pipeline.chunk_load_rate.max(1));
        policy.max_chunk_load_rate = policy
            .max_chunk_load_rate
            .max(chunk_pipeline.chunk_load_rate.max(1));
        policy.min_chunk_generate_rate = policy
            .min_chunk_generate_rate
            .min(chunk_pipeline.chunk_generate_rate.max(1));
        policy.max_chunk_generate_rate = policy
            .max_chunk_generate_rate
            .max(chunk_pipeline.chunk_generate_rate.max(1));
        policy.normalized()
    }

    #[must_use]
    pub fn initial_limits(
        &self,
        server: &ServerSection,
        chunk_pipeline: &ChunkPipelineSection,
    ) -> mc_net::RuntimeControlLimits {
        mc_net::RuntimeControlLimits {
            view_distance: server.view_distance,
            chunk_send_rate: chunk_pipeline.chunk_send_rate.max(1),
            chunk_load_rate: chunk_pipeline.chunk_load_rate.max(1),
            chunk_generate_rate: chunk_pipeline.chunk_generate_rate.max(1),
        }
        .bounded(self.to_policy(chunk_pipeline))
    }
}

impl Default for AdminSection {
    fn default() -> Self {
        Self {
            operators: Vec::new(),
            operators_file: None,
            allow_local_dev_operators: default_allow_local_dev_operators(),
        }
    }
}

impl Default for SimulationSection {
    fn default() -> Self {
        let policy = mc_net::RandomTickPolicy::default();
        Self {
            random_tick_speed: policy.random_tick_speed,
            save_interval_ticks: policy.save_interval_ticks,
            friendly_spawn_interval_ticks: policy.friendly_spawn_interval_ticks,
            hostile_spawn_interval_ticks: policy.hostile_spawn_interval_ticks,
            friendly_spawn_chunk_budget: policy.friendly_spawn_chunk_budget,
            hostile_spawn_chunk_budget: policy.hostile_spawn_chunk_budget,
        }
    }
}

impl SimulationSection {
    #[must_use]
    pub fn to_network(&self, seed: i64, simulation_distance: i32) -> mc_net::RandomTickPolicy {
        let defaults = mc_net::RandomTickPolicy::default();
        mc_net::RandomTickPolicy {
            simulation_distance: simulation_distance
                .clamp(mc_net::MIN_VIEW_DISTANCE, mc_net::MAX_VIEW_DISTANCE),
            random_tick_speed: self.random_tick_speed,
            chunk_budget: defaults.chunk_budget,
            fluid_tick_budget: defaults.fluid_tick_budget,
            save_interval_ticks: self.save_interval_ticks.max(1),
            friendly_spawn_interval_ticks: self.friendly_spawn_interval_ticks,
            hostile_spawn_interval_ticks: self.hostile_spawn_interval_ticks,
            friendly_spawn_chunk_budget: self.friendly_spawn_chunk_budget,
            hostile_spawn_chunk_budget: self.hostile_spawn_chunk_budget,
            seed: seed as u64,
        }
        .normalized()
    }
}

impl Default for ChunkPipelineSection {
    fn default() -> Self {
        let policy = mc_net::ChunkPipelinePolicy::default();
        Self {
            chunk_send_rate: policy.chunk_send_rate,
            chunk_load_rate: policy.chunk_load_rate,
            chunk_generate_rate: policy.chunk_generate_rate,
            chunk_prepare_budget_ms: policy.chunk_prepare_budget_ms,
            chunk_prepare_batch_size: policy.chunk_prepare_batch_size,
            chunk_result_queue_size: policy.chunk_result_queue_size,
            region_cache_size: policy.region_cache_size,
            compression_threshold: policy.compression_threshold,
            compression_level: policy.compression_level,
        }
    }
}

impl ChunkPipelineSection {
    #[must_use]
    pub fn to_network(&self) -> mc_net::ChunkPipelinePolicy {
        let worker_defaults = mc_net::ChunkPipelinePolicy::default();
        mc_net::ChunkPipelinePolicy {
            chunk_send_rate: self.chunk_send_rate.max(1),
            chunk_load_rate: self.chunk_load_rate.max(1),
            chunk_generate_rate: self.chunk_generate_rate.max(1),
            chunk_prepare_budget_ms: self.chunk_prepare_budget_ms,
            chunk_prepare_batch_size: self.chunk_prepare_batch_size.max(1),
            chunk_io_threads: worker_defaults.chunk_io_threads,
            chunk_worker_threads: worker_defaults.chunk_worker_threads,
            chunk_result_queue_size: self.chunk_result_queue_size.max(1),
            region_cache_size: self.region_cache_size.max(1),
            compression_threshold: self.compression_threshold.max(0),
            compression_level: self.compression_level.map(|level| level.min(9)),
            runtime_control: None,
        }
    }
}

fn default_max_players() -> u32 {
    20
}

fn default_view_distance() -> i32 {
    mc_net::DEFAULT_VIEW_DISTANCE
}

fn default_dimension_min_y() -> i32 {
    mc_world::MIN_Y
}

fn default_dimension_height() -> i32 {
    mc_world::MAX_Y - mc_world::MIN_Y
}

fn default_chunk_send_rate() -> u32 {
    mc_net::ChunkPipelinePolicy::default().chunk_send_rate
}

fn default_chunk_load_rate() -> u32 {
    mc_net::ChunkPipelinePolicy::default().chunk_load_rate
}

fn default_chunk_generate_rate() -> u32 {
    mc_net::ChunkPipelinePolicy::default().chunk_generate_rate
}

fn default_chunk_prepare_batch_size() -> usize {
    mc_net::ChunkPipelinePolicy::default().chunk_prepare_batch_size
}

fn default_chunk_result_queue_size() -> usize {
    mc_net::ChunkPipelinePolicy::default().chunk_result_queue_size
}

fn default_region_cache_size() -> usize {
    mc_net::ChunkPipelinePolicy::default().region_cache_size
}

fn default_compression_threshold() -> i32 {
    mc_net::ChunkPipelinePolicy::default().compression_threshold
}

fn default_random_tick_speed() -> u32 {
    mc_net::RandomTickPolicy::default().random_tick_speed
}

fn default_save_interval_ticks() -> u64 {
    mc_net::RandomTickPolicy::default().save_interval_ticks
}

fn default_friendly_spawn_interval_ticks() -> u64 {
    mc_net::RandomTickPolicy::default().friendly_spawn_interval_ticks
}

fn default_hostile_spawn_interval_ticks() -> u64 {
    mc_net::RandomTickPolicy::default().hostile_spawn_interval_ticks
}

fn default_friendly_spawn_chunk_budget() -> usize {
    mc_net::RandomTickPolicy::default().friendly_spawn_chunk_budget
}

fn default_hostile_spawn_chunk_budget() -> usize {
    mc_net::RandomTickPolicy::default().hostile_spawn_chunk_budget
}

fn default_allow_local_dev_operators() -> bool {
    false
}

impl ServerConfig {
    /// Convert a parsed TOML config into the network-layer
    /// [`mc_net::ServerConfig`], using the pre-loaded vanilla data,
    /// block registry, and (optionally) a shared world handle.
    #[allow(clippy::too_many_arguments)]
    pub fn to_network(
        &self,
        data: Arc<VanillaData>,
        blocks: Arc<BlockRegistry>,
        world: Option<WorldHandle>,
        tags: Arc<TagsData>,
        recipes: Arc<Vec<Recipe>>,
        loot: Arc<LootTables>,
        block_light: Option<Arc<mc_data::block_light::BlockLightTable>>,
        items: Arc<ItemRegistry>,
        item_facts: Arc<ItemFactsTable>,
        block_facts: Arc<BlockFactsTable>,
        entity_types: Arc<EntityTypeRegistry>,
        biome_spawns: Arc<BiomeSpawnRules>,
    ) -> anyhow::Result<mc_net::ServerConfig> {
        let geometry = self.data.chunk_geometry().map_err(anyhow::Error::msg)?;
        validate_loaded_chunk_geometry(world.as_ref(), geometry)?;
        let ip: IpAddr = self.network.bind_address.parse().with_context(|| {
            format!(
                "invalid network.bind_address {:?}",
                self.network.bind_address
            )
        })?;
        let mut chunk_pipeline = self.chunk_pipeline.to_network();
        if self.autoscale.enabled {
            chunk_pipeline.runtime_control = Some(mc_net::RuntimeControlConfig {
                policy: self.autoscale.to_policy(&self.chunk_pipeline),
                initial_limits: self
                    .autoscale
                    .initial_limits(&self.server, &self.chunk_pipeline),
            });
        }
        Ok(mc_net::ServerConfig {
            bind_address: SocketAddr::new(ip, self.network.port),
            motd: self.server.motd.clone(),
            max_players: self.server.max_players,
            view_distance: self
                .server
                .view_distance
                .clamp(mc_net::MIN_VIEW_DISTANCE, mc_net::MAX_VIEW_DISTANCE),
            data,
            blocks,
            world,
            tags,
            recipes,
            loot,
            block_light,
            items,
            item_facts,
            block_facts,
            entity_types,
            biome_spawns,
            chunk_pipeline,
            random_tick: self
                .simulation
                .to_network(self.data.seed, self.server.simulation_distance),
            command_permissions: mc_net::CommandPermissionConfig::new(
                self.admin.operators.clone(),
                self.admin.allow_local_dev_operators,
            )
            .with_login_access(
                mc_net::LoginAccessConfig::normalized(
                    self.auth.online_mode,
                    self.auth.whitelist_enabled,
                    self.auth.whitelist.clone(),
                    self.auth.banned_players.clone(),
                )
                .with_prevent_proxy_connections(self.auth.prevent_proxy_connections),
            ),
            loader_manifest: None,
            shutdown: mc_net::ShutdownHandle::default(),
        })
    }
}

fn validate_loaded_chunk_geometry(
    world: Option<&WorldHandle>,
    configured: mc_world::ChunkGeometry,
) -> anyhow::Result<()> {
    let Some(world) = world else {
        return Ok(());
    };
    let storage = world.try_lock().map_err(|_| {
        anyhow::anyhow!("cannot validate loaded chunk geometry: world storage is busy")
    })?;
    for (position, chunk) in storage.resident_chunk_snapshots() {
        let loaded = chunk.geometry();
        if loaded != configured {
            bail!(
                "loaded chunk ({}, {}) has geometry {}..{}, but data config requires {}..{}",
                position.x,
                position.z,
                loaded.min_y(),
                loaded.max_y(),
                configured.min_y(),
                configured.max_y(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashboard_section_is_default_off_loopback_and_optional() {
        let config: ServerConfig = toml::from_str(
            r#"
            [server]
            name = "s"
            motd = "m"
            [network]
            bind_address = "127.0.0.1"
            port = 25565
            "#,
        )
        .expect("minimal config parses without a dashboard section");
        assert!(!config.dashboard.enabled);
        assert_eq!(config.dashboard.bind_address, "127.0.0.1");
        assert_eq!(config.dashboard.port, 8080);
        assert!(!config.dashboard.allow_remote);
        let socket = config
            .dashboard
            .validate()
            .expect("loopback default validates");
        assert!(socket.ip().is_loopback());
    }

    #[test]
    fn dashboard_section_rejects_remote_bind_without_explicit_allow() {
        let config: ServerConfig = toml::from_str(
            r#"
            [server]
            name = "s"
            motd = "m"
            [network]
            bind_address = "127.0.0.1"
            port = 25565
            [dashboard]
            enabled = true
            bind_address = "0.0.0.0"
            port = 8080
            "#,
        )
        .expect("remote dashboard config parses");
        let error = config
            .dashboard
            .validate()
            .expect_err("remote bind refused");
        assert!(error.contains("allow_remote"));

        let allowed: ServerConfig = toml::from_str(
            r#"
            [server]
            name = "s"
            motd = "m"
            [network]
            bind_address = "127.0.0.1"
            port = 25565
            [dashboard]
            enabled = true
            bind_address = "0.0.0.0"
            port = 8080
            allow_remote = true
            "#,
        )
        .expect("explicit remote dashboard parses");
        assert!(allowed.dashboard.validate().is_ok());
    }

    #[test]
    fn dashboard_section_rejects_invalid_bind_address() {
        let config: ServerConfig = toml::from_str(
            r#"
            [server]
            name = "s"
            motd = "m"
            [network]
            bind_address = "127.0.0.1"
            port = 25565
            [dashboard]
            bind_address = "not-an-ip"
            "#,
        )
        .expect("dashboard config parses before validation");
        assert!(config.dashboard.validate().is_err());
    }

    #[test]
    fn parses_playable_profile_as_loopback_survival_spike() {
        let cfg: ServerConfig =
            toml::from_str(include_str!("../../../playable.toml")).expect("parse playable.toml");

        assert_eq!(cfg.server.name, "solaris-playable");
        assert_eq!(cfg.server.motd, "Solaris playable spike");
        assert_eq!(cfg.server.view_distance, 4);
        assert_eq!(cfg.server.simulation_distance, 4);
        assert_eq!(cfg.network.bind_address, "127.0.0.1");
        assert_eq!(cfg.network.port, 25565);
        assert_eq!(
            cfg.data.world_dir,
            Some(std::path::PathBuf::from(".analysis/test-world-v11"))
        );
        assert_eq!(
            cfg.data.vanilla_data_dir,
            Some(std::path::PathBuf::from("data/vanilla"))
        );
        assert_eq!(cfg.data.seed, 0);
        assert_eq!(cfg.data.worldgen_mode, WorldgenMode::TellusLike);
        assert!(!cfg.auth.online_mode);
        assert!(!cfg.auth.prevent_proxy_connections);
        assert!(!cfg.auth.whitelist_enabled);
        assert!(cfg.auth.whitelist.is_empty());
        assert!(cfg.auth.banned_players.is_empty());
        assert!(cfg.admin.operators.is_empty());
        assert!(!cfg.admin.allow_local_dev_operators);
        assert_eq!(cfg.simulation.random_tick_speed, 5);
        assert_eq!(cfg.simulation.save_interval_ticks, 1200);
        assert_eq!(cfg.simulation.friendly_spawn_interval_ticks, 400);
        assert_eq!(cfg.simulation.hostile_spawn_interval_ticks, 20);
        assert_eq!(cfg.simulation.friendly_spawn_chunk_budget, 48);
        assert_eq!(cfg.simulation.hostile_spawn_chunk_budget, 4);
        assert_eq!(cfg.chunk_pipeline.chunk_send_rate, 8);
        assert_eq!(cfg.chunk_pipeline.chunk_load_rate, 16);
        assert_eq!(cfg.chunk_pipeline.chunk_generate_rate, 16);
        assert_eq!(cfg.chunk_pipeline.chunk_result_queue_size, 64);
        assert_eq!(cfg.chunk_pipeline.region_cache_size, 9);
        assert!(cfg.autoscale.enabled);
        assert_eq!(cfg.autoscale.min_view_distance, Some(4));
        assert_eq!(cfg.autoscale.max_view_distance, Some(4));
        let autoscale_policy = cfg.autoscale.to_policy(&cfg.chunk_pipeline);
        assert_eq!(autoscale_policy.min_view_distance, 4);
        assert_eq!(autoscale_policy.max_view_distance, 4);
    }

    #[test]
    fn parses_example_config_shape() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "0.0.0.0"
            port = 25565
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
        assert_eq!(cfg.server.name, "S");
        assert_eq!(cfg.server.max_players, 20);
        assert_eq!(cfg.server.view_distance, 10);
        assert_eq!(cfg.server.simulation_distance, 10);
        assert_eq!(cfg.network.port, 25565);
        assert_eq!(cfg.chunk_pipeline.chunk_prepare_batch_size, 8);
        assert_eq!(cfg.simulation.random_tick_speed, 3);
        assert_eq!(cfg.simulation.save_interval_ticks, 20);
        assert_eq!(cfg.simulation.friendly_spawn_interval_ticks, 400);
        assert_eq!(cfg.simulation.hostile_spawn_interval_ticks, 20);
        assert!(cfg.data.vanilla_data_dir.is_none());
        assert_eq!(cfg.data.worldgen_mode, WorldgenMode::TellusLike);
        assert_eq!(
            cfg.data.chunk_geometry().unwrap(),
            mc_world::OVERWORLD_GEOMETRY
        );
        assert!(!cfg.admin.allow_local_dev_operators);
        assert!(!cfg.auth.online_mode);
        assert!(!cfg.auth.prevent_proxy_connections);
        assert!(!cfg.auth.whitelist_enabled);
        assert!(cfg.autoscale.enabled);
        assert_eq!(cfg.autoscale.profile, AutoscaleProfile::Balanced);
    }

    #[test]
    fn parses_ip_bound_online_authentication() {
        let cfg: ServerConfig = toml::from_str(
            r#"
                [server]
                name = "S"
                motd = "M"

                [network]
                bind_address = "127.0.0.1"
                port = 25565

                [auth]
                online_mode = true
                prevent_proxy_connections = true
            "#,
        )
        .expect("parse IP-bound online authentication");

        assert!(cfg.auth.online_mode);
        assert!(cfg.auth.prevent_proxy_connections);
    }

    #[test]
    fn parses_explicit_chunk_geometry_and_rejects_invalid_ranges() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 25565

            [data]
            min_y = 0
            height = 256
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
        let geometry = cfg.data.chunk_geometry().expect("valid geometry");
        assert_eq!(geometry.min_y(), 0);
        assert_eq!(geometry.height(), 256);

        let invalid = DataSection {
            min_y: 1,
            height: 255,
            ..DataSection::default()
        };
        assert!(invalid.chunk_geometry().is_err());
    }

    #[test]
    fn parses_plugin_directory_without_exposing_runtime_tuning() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 25565

            [plugins]
            directory = "plugins"
            strict = true
            expected = ["basic-economy", "online-roster"]
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");

        assert_eq!(cfg.plugins.directory, Some(PathBuf::from("plugins")));
        assert!(cfg.plugins.strict);
        assert_eq!(
            cfg.plugins.expected,
            ["basic-economy".to_owned(), "online-roster".to_owned()]
        );
    }

    #[test]
    fn example_config_uses_safe_balanced_public_alpha_defaults() {
        let cfg: ServerConfig =
            toml::from_str(include_str!("../../../example.toml")).expect("parse example.toml");
        let policy = cfg.autoscale.to_policy(&cfg.chunk_pipeline);
        let limits = cfg
            .autoscale
            .initial_limits(&cfg.server, &cfg.chunk_pipeline);

        assert!(cfg.autoscale.enabled);
        assert_eq!(cfg.autoscale.profile, AutoscaleProfile::Balanced);
        assert_eq!(cfg.server.view_distance, 8);
        assert_eq!(cfg.server.simulation_distance, 8);
        assert_eq!(cfg.network.bind_address, "127.0.0.1");
        assert_eq!(cfg.data.world_dir, Some(PathBuf::from("world")));
        assert_eq!(policy.min_view_distance, 6);
        assert_eq!(policy.max_view_distance, 10);
        assert_eq!(policy.target_tick_ms, 50);
        assert_eq!(limits.view_distance, 8);
    }

    #[test]
    fn loader_live_gate_config_is_isolated_and_parseable() {
        let cfg: ServerConfig = toml::from_str(include_str!(
            "../../../examples/loader-live-gate/playable.toml"
        ))
        .expect("parse Loader live-gate config");

        assert_eq!(cfg.network.port, 25567);
        assert_eq!(
            cfg.plugins.directory,
            Some(PathBuf::from("examples/loader-live-gate/plugins"))
        );
        assert_eq!(
            cfg.data.world_dir,
            Some(PathBuf::from(".analysis/loader-live-gate/world"))
        );
        assert!(!cfg.auth.online_mode);
        assert!(!cfg.autoscale.enabled);
    }

    #[test]
    fn parses_explicit_auth_and_admin_policy() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 25565

            [admin]
            operators = ["Notch"]
            allow_local_dev_operators = true

            [auth]
            online_mode = false
            whitelist_enabled = true
            whitelist = ["Notch"]
            banned_players = ["BadActor"]
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
        assert_eq!(cfg.admin.operators, ["Notch"]);
        assert!(cfg.admin.allow_local_dev_operators);
        assert!(cfg.auth.whitelist_enabled);
        assert_eq!(cfg.auth.whitelist, ["Notch"]);
        assert_eq!(cfg.auth.banned_players, ["BadActor"]);
    }

    #[test]
    fn parses_optional_vanilla_data_dir() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "0.0.0.0"
            port = 25565

            [data]
            vanilla_data_dir = "data/vanilla"
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
        assert_eq!(
            cfg.data.vanilla_data_dir,
            Some(PathBuf::from("data/vanilla"))
        );
    }

    #[test]
    fn parses_explicit_tellus_like_worldgen_config() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "0.0.0.0"
            port = 25565

            [data]
            worldgen_mode = "tellus_like"
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
        assert_eq!(cfg.data.worldgen_mode, WorldgenMode::TellusLike);
    }

    #[test]
    fn data_section_rejects_removed_vanilla_dir() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "0.0.0.0"
            port = 25565

            [data]
            vanilla_dir = "data/vanilla"
        "#;

        let err = toml::from_str::<ServerConfig>(toml_src).unwrap_err();
        assert!(err.to_string().contains("unknown field `vanilla_dir`"));
    }

    #[test]
    fn server_section_rejects_unknown_fields() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"
            online_mode = false

            [network]
            bind_address = "127.0.0.1"
            port = 25565
        "#;

        let err = toml::from_str::<ServerConfig>(toml_src).unwrap_err();
        assert!(err.to_string().contains("unknown field `online_mode`"));
    }

    #[test]
    fn network_section_rejects_unknown_fields() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 25565
            online_mode = false
        "#;

        let err = toml::from_str::<ServerConfig>(toml_src).unwrap_err();
        assert!(err.to_string().contains("unknown field `online_mode`"));
    }

    #[test]
    fn chunk_pipeline_rejects_removed_worker_percentages() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 25565

            [chunk_pipeline]
            chunk_worker_threads_percent = 75
        "#;

        let err = toml::from_str::<ServerConfig>(toml_src).unwrap_err();
        assert!(
            err.to_string()
                .contains("unknown field `chunk_worker_threads_percent`")
        );
    }

    #[test]
    fn admin_section_rejects_unknown_fields() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 25565

            [admin]
            operators = []
            op_everyone = true
        "#;

        let err = toml::from_str::<ServerConfig>(toml_src).unwrap_err();
        assert!(err.to_string().contains("unknown field `op_everyone`"));
    }

    #[test]
    fn auth_section_rejects_unknown_fields() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 25565

            [auth]
            online_mode = false
            ops = ["Notch"]
        "#;

        let err = toml::from_str::<ServerConfig>(toml_src).unwrap_err();
        assert!(err.to_string().contains("unknown field `ops`"));
    }

    #[test]
    fn parses_chunk_pipeline_overrides() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "0.0.0.0"
            port = 25565

            [chunk_pipeline]
            chunk_send_rate = 12
            chunk_load_rate = 8
            chunk_generate_rate = 4
            chunk_prepare_budget_ms = 3
            chunk_prepare_batch_size = 2
            chunk_result_queue_size = 9
            region_cache_size = 7
            compression_threshold = 128
            compression_level = 6
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
        assert_eq!(cfg.chunk_pipeline.chunk_send_rate, 12);
        assert_eq!(cfg.chunk_pipeline.chunk_load_rate, 8);
        assert_eq!(cfg.chunk_pipeline.chunk_generate_rate, 4);
        assert_eq!(cfg.chunk_pipeline.chunk_prepare_budget_ms, 3);
        assert_eq!(cfg.chunk_pipeline.chunk_prepare_batch_size, 2);
        assert_eq!(cfg.chunk_pipeline.chunk_result_queue_size, 9);
        assert_eq!(cfg.chunk_pipeline.region_cache_size, 7);
        assert_eq!(cfg.chunk_pipeline.compression_threshold, 128);
        assert_eq!(cfg.chunk_pipeline.compression_level, Some(6));
    }

    #[test]
    fn chunk_pipeline_normalizes_zero_values_for_runtime() {
        let section = ChunkPipelineSection {
            chunk_send_rate: 0,
            chunk_load_rate: 0,
            chunk_generate_rate: 0,
            chunk_prepare_budget_ms: 0,
            chunk_prepare_batch_size: 0,
            chunk_result_queue_size: 0,
            region_cache_size: 0,
            compression_threshold: -1,
            compression_level: Some(99),
        };
        let policy = section.to_network();
        assert_eq!(policy.chunk_send_rate, 1);
        assert_eq!(policy.chunk_load_rate, 1);
        assert_eq!(policy.chunk_generate_rate, 1);
        assert_eq!(policy.chunk_prepare_batch_size, 1);
        let defaults = mc_net::ChunkPipelinePolicy::default();
        assert_eq!(policy.chunk_io_threads, defaults.chunk_io_threads);
        assert_eq!(policy.chunk_worker_threads, defaults.chunk_worker_threads);
        assert_eq!(policy.chunk_result_queue_size, 1);
        assert_eq!(policy.region_cache_size, 1);
        assert_eq!(policy.compression_threshold, 0);
        assert_eq!(policy.compression_level, Some(9));
    }

    #[test]
    fn parses_simulation_overrides() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "0.0.0.0"
            port = 25565

            [simulation]
            random_tick_speed = 7
            save_interval_ticks = 40
            friendly_spawn_interval_ticks = 800
            hostile_spawn_interval_ticks = 0
            friendly_spawn_chunk_budget = 20
            hostile_spawn_chunk_budget = 6
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");

        assert_eq!(cfg.simulation.random_tick_speed, 7);
        assert_eq!(cfg.simulation.save_interval_ticks, 40);
        assert_eq!(cfg.simulation.friendly_spawn_interval_ticks, 800);
        assert_eq!(cfg.simulation.hostile_spawn_interval_ticks, 0);
        assert_eq!(cfg.simulation.friendly_spawn_chunk_budget, 20);
        assert_eq!(cfg.simulation.hostile_spawn_chunk_budget, 6);
    }

    #[test]
    fn parses_autoscale_overrides_and_builds_bounded_policy() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"
            view_distance = 8

            [network]
            bind_address = "0.0.0.0"
            port = 25565

            [chunk_pipeline]
            chunk_send_rate = 8
            chunk_load_rate = 16
            chunk_generate_rate = 16

            [autoscale]
            enabled = true
            profile = "low_end"
            min_view_distance = 3
            max_view_distance = 6
            target_tick_ms = 45
            target_first_chunk_ms = 1200
            scale_down_after_ticks = 2
            scale_up_after_ticks = 7
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
        let policy = cfg.autoscale.to_policy(&cfg.chunk_pipeline);
        let limits = cfg
            .autoscale
            .initial_limits(&cfg.server, &cfg.chunk_pipeline);

        assert!(cfg.autoscale.enabled);
        assert_eq!(cfg.autoscale.profile, AutoscaleProfile::LowEnd);
        assert_eq!(policy.min_view_distance, 3);
        assert_eq!(policy.max_view_distance, 6);
        assert_eq!(policy.target_tick_ms, 45);
        assert_eq!(policy.target_first_chunk_ms, 1200);
        assert_eq!(policy.scale_down_after_ticks, 2);
        assert_eq!(policy.scale_up_after_ticks, 7);
        assert_eq!(limits.view_distance, 6);
        assert_eq!(limits.chunk_send_rate, 8);
        assert_eq!(limits.chunk_load_rate, 16);
        assert_eq!(limits.chunk_generate_rate, 16);

        let net = cfg
            .to_network(
                Arc::new(mc_data::testing::stub()),
                stub_blocks(),
                None,
                stub_tags(),
                Arc::new(Vec::new()),
                Arc::new(LootTables::default()),
                None,
                Arc::new(ItemRegistry::default()),
                Arc::new(ItemFactsTable::default()),
                Arc::new(BlockFactsTable::default()),
                Arc::new(mc_data::entity_types::solaris_required_entity_types()),
                Arc::new(BiomeSpawnRules::default()),
            )
            .unwrap();
        let runtime = net
            .chunk_pipeline
            .runtime_control
            .expect("enabled autoscale wires runtime control");
        assert_eq!(runtime.policy, policy);
        assert_eq!(runtime.initial_limits, limits);
    }

    #[test]
    fn autoscale_section_rejects_unknown_fields() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "0.0.0.0"
            port = 25565

            [autoscale]
            enabled = false
            unexpected = true
        "#;

        let err = toml::from_str::<ServerConfig>(toml_src).unwrap_err();
        assert!(err.to_string().contains("unknown field `unexpected`"));
    }

    #[test]
    fn simulation_normalizes_runtime_budget() {
        let section = SimulationSection {
            random_tick_speed: 0,
            save_interval_ticks: 0,
            friendly_spawn_interval_ticks: 0,
            hostile_spawn_interval_ticks: 0,
            friendly_spawn_chunk_budget: 0,
            hostile_spawn_chunk_budget: usize::MAX,
        };
        let policy = section.to_network(42, 5);

        assert_eq!(policy.simulation_distance, 5);
        assert_eq!(policy.random_tick_speed, 0);
        assert_eq!(policy.chunk_budget, 64);
        assert_eq!(policy.fluid_tick_budget, 256);
        assert_eq!(policy.save_interval_ticks, 1);
        assert_eq!(policy.friendly_spawn_interval_ticks, 0);
        assert_eq!(policy.hostile_spawn_interval_ticks, 0);
        assert_eq!(policy.friendly_spawn_chunk_budget, 1);
        assert_eq!(
            policy.hostile_spawn_chunk_budget,
            mc_net::MAX_NATURAL_SPAWN_CHUNK_BUDGET
        );
        assert_eq!(policy.seed, 42);
    }

    #[test]
    fn simulation_rejects_removed_manual_work_budgets() {
        let error = toml::from_str::<ServerConfig>(
            r#"
                [simulation]
                random_tick_chunk_budget = 11
                scheduled_fluid_tick_budget = 13
            "#,
        )
        .expect_err("runtime work budgets must belong to autoscale");

        let message = error.to_string();
        assert!(
            message.contains("random_tick_chunk_budget")
                || message.contains("scheduled_fluid_tick_budget")
        );
    }

    #[test]
    fn file_backed_access_control_resolves_relative_paths_and_merges_identities() {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("server.toml");
        std::fs::write(
            temp.path().join("ops.json"),
            r#"[
                {"name":"FileOp","uuid":"11111111-1111-1111-1111-111111111111","level":4,"bypassesPlayerLimit":false},
                {"name":"SecondOp"}
            ]"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("whitelist.json"),
            r#"[{"name":"Allowed","uuid":"22222222-2222-2222-2222-222222222222"}]"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("banned-players.json"),
            r#"[{"name":"Banned","uuid":"33333333-3333-3333-3333-333333333333","reason":"test"}]"#,
        )
        .unwrap();

        let mut cfg: ServerConfig = toml::from_str(
            r#"
                [server]
                name = "S"
                motd = "M"

                [network]
                bind_address = "127.0.0.1"
                port = 25565

                [admin]
                operators = ["InlineOp"]
                operators_file = "ops.json"

                [auth]
                whitelist_enabled = true
                whitelist = ["InlineAllowed"]
                whitelist_file = "whitelist.json"
                banned_players = ["InlineBan"]
                banned_players_file = "banned-players.json"
            "#,
        )
        .unwrap();

        let report = cfg.load_access_control_files(&config_path).unwrap();
        assert_eq!(report.files_loaded, 3);
        assert_eq!(report.operator_identities, 3);
        assert_eq!(report.whitelist_identities, 2);
        assert_eq!(report.banned_identities, 2);
        assert!(cfg.admin.operators.iter().any(|entry| entry == "InlineOp"));
        assert!(cfg.admin.operators.iter().any(|entry| entry == "fileop"));
        assert!(
            cfg.admin
                .operators
                .iter()
                .any(|entry| entry == "11111111-1111-1111-1111-111111111111")
        );
        assert!(cfg.auth.whitelist.iter().any(|entry| entry == "allowed"));
        assert!(
            cfg.auth
                .whitelist
                .iter()
                .any(|entry| entry == "22222222-2222-2222-2222-222222222222")
        );
        assert!(
            cfg.auth
                .banned_players
                .iter()
                .any(|entry| entry == "banned")
        );
        assert!(
            cfg.auth
                .banned_players
                .iter()
                .any(|entry| entry == "33333333-3333-3333-3333-333333333333")
        );
    }

    #[test]
    fn file_backed_access_control_fails_closed_for_bad_files() {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("server.toml");
        let access_path = temp.path().join("whitelist.json");
        let config_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 25565

            [auth]
            whitelist_enabled = true
            whitelist_file = "whitelist.json"
        "#;

        let mut cfg: ServerConfig = toml::from_str(config_src).unwrap();
        assert!(cfg.load_access_control_files(&config_path).is_err());

        std::fs::write(&access_path, b"{}").unwrap();
        let mut cfg: ServerConfig = toml::from_str(config_src).unwrap();
        assert!(cfg.load_access_control_files(&config_path).is_err());

        std::fs::write(&access_path, br#"[{}]"#).unwrap();
        let mut cfg: ServerConfig = toml::from_str(config_src).unwrap();
        let error = cfg
            .load_access_control_files(&config_path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("must contain name and/or uuid"));
        assert!(error.contains(&access_path.display().to_string()));

        std::fs::write(&access_path, br#"[{"name":"bad name"}]"#).unwrap();
        let mut cfg: ServerConfig = toml::from_str(config_src).unwrap();
        let error = cfg
            .load_access_control_files(&config_path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid Minecraft username"));
        assert!(error.contains(&access_path.display().to_string()));
        assert!(!error.contains("bad name"));

        std::fs::write(&access_path, br#"[{"uuid":"not-a-uuid"}]"#).unwrap();
        let mut cfg: ServerConfig = toml::from_str(config_src).unwrap();
        let error = cfg
            .load_access_control_files(&config_path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid uuid"));
        assert!(error.contains(&access_path.display().to_string()));
        assert!(!error.contains("not-a-uuid"));

        let too_many = (0..=MAX_ACCESS_CONTROL_FILE_ENTRIES)
            .map(|index| format!(r#"{{"name":"User{index:04}"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        std::fs::write(&access_path, format!("[{too_many}]")).unwrap();
        let mut cfg: ServerConfig = toml::from_str(config_src).unwrap();
        let error = cfg
            .load_access_control_files(&config_path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("maximum is 4096"));
        assert!(error.contains(&access_path.display().to_string()));

        std::fs::write(
            &access_path,
            vec![b' '; usize::try_from(MAX_ACCESS_CONTROL_FILE_BYTES).unwrap() + 1],
        )
        .unwrap();
        let mut cfg: ServerConfig = toml::from_str(config_src).unwrap();
        assert!(
            cfg.load_access_control_files(&config_path)
                .unwrap_err()
                .to_string()
                .contains("exceeds")
        );
    }

    #[test]
    fn file_backed_access_control_reloads_on_fresh_server_config() {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("server.toml");
        let whitelist_path = temp.path().join("whitelist.json");
        let config_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 25565

            [auth]
            whitelist_enabled = true
            whitelist_file = "whitelist.json"
        "#;

        std::fs::write(&whitelist_path, br#"[{"name":"FirstUser"}]"#).unwrap();
        let mut first: ServerConfig = toml::from_str(config_src).unwrap();
        first.load_access_control_files(&config_path).unwrap();
        assert!(
            first
                .auth
                .whitelist
                .iter()
                .any(|entry| entry == "firstuser")
        );

        std::fs::write(&whitelist_path, br#"[{"name":"SecondUser"}]"#).unwrap();
        let mut restarted: ServerConfig = toml::from_str(config_src).unwrap();
        restarted.load_access_control_files(&config_path).unwrap();
        assert!(
            !restarted
                .auth
                .whitelist
                .iter()
                .any(|entry| entry == "firstuser")
        );
        assert!(
            restarted
                .auth
                .whitelist
                .iter()
                .any(|entry| entry == "seconduser")
        );
    }

    #[test]
    fn default_operator_file_loads_for_fresh_server_config() {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("server.toml");
        let config_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 25565
        "#;
        let mut configured: ServerConfig = toml::from_str(config_src).unwrap();
        configured.admin.operators_file = Some(PathBuf::from("ops.json"));
        configured
            .manage_operator_file(
                &config_path,
                OperatorFileOperation::Add("FreshOp".to_owned()),
            )
            .unwrap();

        let mut restarted: ServerConfig = toml::from_str(config_src).unwrap();
        let report = restarted.load_access_control_files(&config_path).unwrap();

        assert_eq!(report.operator_identities, 1);
        assert_eq!(restarted.admin.operators, vec!["freshop".to_owned()]);
    }
    fn stub_blocks() -> Arc<BlockRegistry> {
        Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"))
    }

    fn stub_tags() -> Arc<TagsData> {
        Arc::new(TagsData::default())
    }

    #[test]
    fn translates_to_network_config() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "Howdy"
            max_players = 50
            view_distance = 7
            simulation_distance = 5

            [network]
            bind_address = "127.0.0.1"
            port = 25000

            [data]
            world_dir = "/tmp/world"
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).unwrap();
        let data = Arc::new(mc_data::testing::stub());
        let net = cfg
            .to_network(
                data,
                stub_blocks(),
                None,
                stub_tags(),
                Arc::new(Vec::new()),
                Arc::new(LootTables::default()),
                None,
                Arc::new(ItemRegistry::default()),
                Arc::new(ItemFactsTable::default()),
                Arc::new(BlockFactsTable::default()),
                Arc::new(mc_data::entity_types::solaris_required_entity_types()),
                Arc::new(BiomeSpawnRules::default()),
            )
            .unwrap();
        assert_eq!(net.motd, "Howdy");
        assert_eq!(net.max_players, 50);
        assert_eq!(net.view_distance, 7);
        assert_eq!(net.random_tick.simulation_distance, 5);
        assert_eq!(net.bind_address.port(), 25000);
        assert!(net.world.is_none());
        assert_eq!(net.chunk_pipeline.region_cache_size, 4);
        let runtime = net
            .chunk_pipeline
            .runtime_control
            .expect("default config wires runtime autoscale");
        assert_eq!(runtime.initial_limits.view_distance, 7);
        assert_eq!(cfg.data.world_dir, Some(PathBuf::from("/tmp/world")));
    }

    #[test]
    fn invalid_bind_address_is_rejected() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = ""

            [network]
            bind_address = "not-an-ip"
            port = 25565
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).unwrap();
        let data = Arc::new(mc_data::testing::stub());
        assert!(
            cfg.to_network(
                data,
                stub_blocks(),
                None,
                stub_tags(),
                Arc::new(Vec::new()),
                Arc::new(LootTables::default()),
                None,
                Arc::new(ItemRegistry::default()),
                Arc::new(ItemFactsTable::default()),
                Arc::new(BlockFactsTable::default()),
                Arc::new(mc_data::entity_types::solaris_required_entity_types()),
                Arc::new(BiomeSpawnRules::default())
            )
            .is_err()
        );
    }
    #[test]
    fn operator_file_mutations_persist_and_deduplicate() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("server.toml");
        let operator_path = dir.path().join("ops.json");
        std::fs::write(
            &operator_path,
            r#"[{"name":"Builder","level":4},{"name":"builder"},{"name":"Other","note":"keep"}]"#,
        )
        .unwrap();
        let config: ServerConfig = toml::from_str(
            r#"
                [server]
                name = "Test"
                motd = "Test"
                [network]
                bind_address = "127.0.0.1"
                port = 25565
                [admin]
                operators_file = "ops.json"
            "#,
        )
        .unwrap();

        let listed = config
            .manage_operator_file(&config_path, OperatorFileOperation::List)
            .unwrap();
        assert_eq!(listed.identities, vec!["builder", "other"]);
        let added = config
            .manage_operator_file(&config_path, OperatorFileOperation::Add("Alice".to_owned()))
            .unwrap();
        assert!(added.changed);
        let persisted = std::fs::read_to_string(&operator_path).unwrap();
        assert!(persisted.contains(r#""level": 4"#));
        assert!(persisted.contains(r#""note": "keep""#));
        let removed = config
            .manage_operator_file(
                &config_path,
                OperatorFileOperation::Remove("builder".to_owned()),
            )
            .unwrap();
        assert!(removed.changed);
        assert!(
            !std::fs::read_to_string(&operator_path)
                .unwrap()
                .contains("builder")
        );
    }

    #[test]
    fn invalid_operator_add_has_no_file_side_effect() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("server.toml");
        let config: ServerConfig = toml::from_str(
            r#"
                [server]
                name = "Test"
                motd = "Test"
                [network]
                bind_address = "127.0.0.1"
                port = 25565
            "#,
        )
        .unwrap();
        let config = ServerConfig {
            admin: AdminSection {
                operators_file: Some(PathBuf::from("ops.json")),
                ..config.admin
            },
            ..config
        };

        let error = config
            .manage_operator_file(&config_path, OperatorFileOperation::Add("no".to_owned()))
            .unwrap_err();

        assert!(error.to_string().contains("invalid operator identity"));
        assert!(!dir.path().join("ops.json").exists());
    }
}
