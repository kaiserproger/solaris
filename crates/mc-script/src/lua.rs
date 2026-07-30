use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io::Read;
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use mlua::{Function, Lua, LuaOptions, LuaString, StdLib, Table, Value, VmState};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

#[cfg(test)]
mod loader_tests;
#[cfg(test)]
mod worldgen_tests;

use crate::{
    CommandBatch, CommandBatchError, CommandCapabilities, HostCommandAdmission,
    MAX_ONLINE_PLAYER_QUERY_LIMIT, MAX_SCRIPT_CHAT_MESSAGE_BYTES, MAX_SCRIPT_CONSOLE_COMMAND_BYTES,
    MAX_SCRIPT_DISCONNECT_REASON_BYTES, PlayerCommandRegistrationError, RuntimeContext,
    RuntimeError, RuntimeResult, ScriptApiVersion, ScriptAxisAlignedZone,
    ScriptBatchSubmissionError, ScriptBoundary, ScriptCommand, ScriptDtoError, ScriptEvent,
    ScriptEventKind, ScriptHostEndpoint, ScriptInventoryMenu, ScriptInventoryMenuItem,
    ScriptInventoryMenuSlot, ScriptInventoryResourceDelta, ScriptInventoryStorageTransaction,
    ScriptOnlinePlayersRequest, ScriptPlayerId, ScriptPlayerInventoryTransaction,
    ScriptPlayerTeleportRequest, ScriptPluginManifest, ScriptPluginStorageCompareAndSwapRequest,
    ScriptPluginStorageDeleteRequest, ScriptPluginStorageGetRequest, ScriptPosition, ScriptRuntime,
    ScriptStorageMutation, ScriptVillagerBindingRequest, ScriptVillagerGoal,
    ScriptVillagerGoalRequest, ScriptZoneProtection, ValidatedScriptPluginManifest,
    script_boundary_pair,
};

const EVENT_QUEUE_CAPACITY: usize = 1_024;
const COMMAND_QUEUE_CAPACITY: usize = 256;
const COMMANDS_PER_EVENT: usize = 32;
const MEMORY_BYTES_PER_PLUGIN: usize = 16 * 1024 * 1024;
const INSTRUCTIONS_PER_EVENT: u64 = 100_000;
const LUAU_INTERRUPT_FUEL_COST: u64 = 2;
const MAX_PLUGIN_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_PLUGIN_CONFIG_BYTES: usize = 64 * 1024;
const MAX_PLUGIN_CONFIG_DEPTH: usize = 8;
const MAX_PLUGIN_CONFIG_CONTAINER_ENTRIES: usize = 128;
const MAX_PLUGIN_CONFIG_KEY_BYTES: usize = 128;
const MAX_PLUGIN_CONFIG_STRING_BYTES: usize = 4 * 1024;
const MAX_PLUGIN_SOURCE_BYTES: usize = 1024 * 1024;
const MAX_PLUGIN_DIRECTORIES: usize = 128;
const MAX_API_VERSION_BYTES: usize = 16;
const MAX_SETTLEMENT_BUILDINGS: usize = 3;
const MAX_SETTLEMENT_INHABITANTS: usize = 16;
const MAX_SETTLEMENT_EXTENSIONS: usize = 16;
const MAX_SETTLEMENT_DESCRIPTOR_ID_BYTES: usize = 48;
const CLIENT_MANIFEST_SCHEMA: u16 = 1;
const MAX_CLIENT_BUNDLES_PER_PLUGIN: usize = 8;
const MAX_CLIENT_BUNDLE_ID_BYTES: usize = 48;
const MAX_CLIENT_BUNDLE_VERSION_BYTES: usize = 32;
const MAX_CLIENT_ARTIFACT_PATH_BYTES: usize = 160;
const MAX_CLIENT_BUNDLE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PENDING_PLUGIN_TIMERS: usize = 256;
const MAX_PLUGIN_TIMER_CALLBACKS_PER_TICK: usize = 8;
const MAX_PLUGIN_TIMER_DELAY_TICKS: u64 = 630_720_000;
const SOLARIS_LUAU_PRELUDE: &str = "local solaris: any = nil :: any\n";

/// One server-embedded Luau plugin package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BundledLuauPlugin {
    directory_name: &'static str,
    manifest: &'static str,
    config: Option<&'static str>,
    source: &'static str,
}

impl BundledLuauPlugin {
    #[must_use]
    pub const fn new(
        directory_name: &'static str,
        manifest: &'static str,
        source: &'static str,
    ) -> Self {
        Self {
            directory_name,
            manifest,
            config: None,
            source,
        }
    }

    #[must_use]
    pub const fn with_config(mut self, config: &'static str) -> Self {
        self.config = Some(config);
        self
    }
}

/// Filesystem configuration for the built-in Luau plugin host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaHostConfig {
    plugins_dir: PathBuf,
}

impl LuaHostConfig {
    pub fn new(plugins_dir: impl Into<PathBuf>) -> Self {
        Self {
            plugins_dir: plugins_dir.into(),
        }
    }

    pub fn plugins_dir(&self) -> &Path {
        &self.plugins_dir
    }
}

/// Startup error for the Luau host itself. Individual broken plugins are skipped.
#[derive(Debug)]
#[non_exhaustive]
pub enum LuaHostError {
    Io {
        path: PathBuf,
        message: String,
    },
    ThreadSpawn {
        message: String,
    },
    StartupChannelClosed,
    PluginIdConflict {
        id: String,
    },
    WorldgenConflict {
        kind: &'static str,
        first: String,
        second: String,
    },
    InvalidStartupPlugin {
        path: PathBuf,
        message: String,
    },
}

impl fmt::Display for LuaHostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(formatter, "{}: {message}", path.display()),
            Self::ThreadSpawn { message } => {
                write!(formatter, "starting Luau host thread: {message}")
            }
            Self::StartupChannelClosed => formatter.write_str("Luau host startup channel closed"),
            Self::PluginIdConflict { id } => {
                write!(formatter, "plugin id {id:?} is declared more than once")
            }
            Self::WorldgenConflict {
                kind,
                first,
                second,
            } => write!(
                formatter,
                "plugins {first} and {second} both declare a worldgen {kind} profile"
            ),
            Self::InvalidStartupPlugin { path, message } => write!(
                formatter,
                "startup-declarative plugin {} is invalid: {message}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for LuaHostError {}

/// Join handle and startup report for one running Lua host thread.
#[derive(Debug)]
pub struct LuaHost {
    loaded_plugins: usize,
    thread: thread::JoinHandle<()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LuaWorldgenOreProfile {
    GeologicalDeposits,
}

impl LuaWorldgenOreProfile {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::GeologicalDeposits => "geological_deposits",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LuaWorldgenSettlementProfile {
    PlainsVillagePrototype,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LuaClientLoader {
    Fabric,
    NeoForge,
    Forge,
}

impl LuaClientLoader {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Fabric => "fabric",
            Self::NeoForge => "neoforge",
            Self::Forge => "forge",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LuaClientContentKind {
    Blocks,
    Items,
    Screens,
    Assets,
    Interactions,
}

impl LuaClientContentKind {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Blocks => "blocks",
            Self::Items => "items",
            Self::Screens => "screens",
            Self::Assets => "assets",
            Self::Interactions => "interactions",
        }
    }

    const fn required_permission(self) -> LuaClientPermission {
        match self {
            Self::Blocks => LuaClientPermission::RegisterBlocks,
            Self::Items => LuaClientPermission::RegisterItems,
            Self::Screens => LuaClientPermission::OpenScreens,
            Self::Assets => LuaClientPermission::LoadAssets,
            Self::Interactions => LuaClientPermission::SendInteractions,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LuaClientPermission {
    RegisterBlocks,
    RegisterItems,
    OpenScreens,
    LoadAssets,
    SendInteractions,
}

impl LuaClientPermission {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::RegisterBlocks => "register_blocks",
            Self::RegisterItems => "register_items",
            Self::OpenScreens => "open_screens",
            Self::LoadAssets => "load_assets",
            Self::SendInteractions => "send_interactions",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaClientBundle {
    owner_plugin_id: String,
    id: String,
    version: String,
    artifact: String,
    sha256: String,
    size_bytes: u64,
    artifact_path: PathBuf,
    loaders: Vec<LuaClientLoader>,
    content: Vec<LuaClientContentKind>,
    permissions: Vec<LuaClientPermission>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LuaPluginDeployment {
    ServerOnly,
    ServerAndClient,
}

impl LuaPluginDeployment {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::ServerOnly => "server_only",
            Self::ServerAndClient => "server_and_client",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LuaPluginDiscovery<'a> {
    id: &'a str,
    deployment: LuaPluginDeployment,
}

impl<'a> LuaPluginDiscovery<'a> {
    #[must_use]
    pub const fn id(self) -> &'a str {
        self.id
    }

    #[must_use]
    pub const fn deployment(self) -> LuaPluginDeployment {
        self.deployment
    }
}

impl LuaClientBundle {
    #[must_use]
    pub fn owner_plugin_id(&self) -> &str {
        &self.owner_plugin_id
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    #[must_use]
    pub fn artifact(&self) -> &str {
        &self.artifact
    }

    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    #[must_use]
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    #[must_use]
    pub fn artifact_path(&self) -> &Path {
        &self.artifact_path
    }

    #[must_use]
    pub fn loaders(&self) -> &[LuaClientLoader] {
        &self.loaders
    }

    #[must_use]
    pub fn content(&self) -> &[LuaClientContentKind] {
        &self.content
    }

    #[must_use]
    pub fn permissions(&self) -> &[LuaClientPermission] {
        &self.permissions
    }

    #[must_use]
    pub fn cache_key(&self) -> String {
        format!(
            "{}:{}/{}/{}",
            self.owner_plugin_id, self.id, self.version, self.sha256
        )
    }
}

impl LuaWorldgenSettlementProfile {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::PlainsVillagePrototype => "plains_village_prototype",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LuaSettlementBuildingTemplate {
    PlainsFountain,
    PlainsSmallHouse,
    PlainsToolsmith,
}

impl LuaSettlementBuildingTemplate {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::PlainsFountain => "plains_fountain",
            Self::PlainsSmallHouse => "plains_small_house",
            Self::PlainsToolsmith => "plains_toolsmith",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LuaSettlementBuildingRole {
    MeetingPoint,
    Home,
    Workplace,
}

impl LuaSettlementBuildingRole {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::MeetingPoint => "meeting_point",
            Self::Home => "home",
            Self::Workplace => "workplace",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LuaSettlementInhabitantKind {
    Villager,
}

impl LuaSettlementInhabitantKind {
    #[must_use]
    pub const fn entity_type(self) -> &'static str {
        match self {
            Self::Villager => "minecraft:villager",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LuaSettlementJob {
    Unemployed,
    Toolsmith,
}

impl LuaSettlementJob {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Unemployed => "unemployed",
            Self::Toolsmith => "toolsmith",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaSettlementBuilding {
    id: String,
    template: LuaSettlementBuildingTemplate,
    role: LuaSettlementBuildingRole,
}

impl LuaSettlementBuilding {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub const fn template(&self) -> LuaSettlementBuildingTemplate {
        self.template
    }

    #[must_use]
    pub const fn role(&self) -> LuaSettlementBuildingRole {
        self.role
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaSettlementInhabitant {
    id: String,
    kind: LuaSettlementInhabitantKind,
    building_id: String,
    job: LuaSettlementJob,
}

impl LuaSettlementInhabitant {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub const fn kind(&self) -> LuaSettlementInhabitantKind {
        self.kind
    }

    #[must_use]
    pub fn building_id(&self) -> &str {
        &self.building_id
    }

    #[must_use]
    pub const fn job(&self) -> LuaSettlementJob {
        self.job
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaSettlementExtension {
    id: String,
    building_id: String,
}

impl LuaSettlementExtension {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn building_id(&self) -> &str {
        &self.building_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaSettlementPlan {
    owner_plugin_id: String,
    profile: LuaWorldgenSettlementProfile,
    buildings: Vec<LuaSettlementBuilding>,
    inhabitants: Vec<LuaSettlementInhabitant>,
    extensions: Vec<LuaSettlementExtension>,
}

impl LuaSettlementPlan {
    #[must_use]
    pub fn plains_village_prototype(owner_plugin_id: impl Into<String>) -> Self {
        Self {
            owner_plugin_id: owner_plugin_id.into(),
            profile: LuaWorldgenSettlementProfile::PlainsVillagePrototype,
            buildings: default_plains_village_buildings(),
            inhabitants: Vec::new(),
            extensions: Vec::new(),
        }
    }

    #[must_use]
    pub fn owner_plugin_id(&self) -> &str {
        &self.owner_plugin_id
    }

    #[must_use]
    pub const fn profile(&self) -> LuaWorldgenSettlementProfile {
        self.profile
    }

    #[must_use]
    pub fn buildings(&self) -> &[LuaSettlementBuilding] {
        &self.buildings
    }

    #[must_use]
    pub fn inhabitants(&self) -> &[LuaSettlementInhabitant] {
        &self.inhabitants
    }

    #[must_use]
    pub fn extensions(&self) -> &[LuaSettlementExtension] {
        &self.extensions
    }

    #[must_use]
    pub fn contract_name(&self) -> String {
        let mut contract = format!(
            "{}|owner={}|buildings=",
            self.profile.contract_name(),
            self.owner_plugin_id
        );
        for building in &self.buildings {
            contract.push_str(&format!(
                "{},{},{};",
                building.id,
                building.template.contract_name(),
                building.role.contract_name()
            ));
        }
        contract.push_str("|inhabitants=");
        for inhabitant in &self.inhabitants {
            contract.push_str(&format!(
                "{},{},{},{};",
                inhabitant.id,
                inhabitant.kind.entity_type(),
                inhabitant.building_id,
                inhabitant.job.contract_name()
            ));
        }
        contract.push_str("|extensions=");
        for extension in &self.extensions {
            contract.push_str(&format!("{},{};", extension.id, extension.building_id));
        }
        contract
    }
}

#[derive(Debug)]
pub struct PreparedLuaPlugins {
    sources: Vec<PluginSource>,
    worldgen_ore_profile: Option<LuaWorldgenOreProfile>,
    worldgen_settlement_plan: Option<LuaSettlementPlan>,
    client_bundles: Vec<LuaClientBundle>,
}

impl PreparedLuaPlugins {
    #[must_use]
    pub const fn worldgen_ore_profile(&self) -> Option<LuaWorldgenOreProfile> {
        self.worldgen_ore_profile
    }

    #[must_use]
    pub const fn worldgen_settlement_profile(&self) -> Option<LuaWorldgenSettlementProfile> {
        match &self.worldgen_settlement_plan {
            Some(plan) => Some(plan.profile()),
            None => None,
        }
    }

    #[must_use]
    pub fn worldgen_settlement_plan(&self) -> Option<&LuaSettlementPlan> {
        self.worldgen_settlement_plan.as_ref()
    }

    #[must_use]
    pub fn client_bundles(&self) -> &[LuaClientBundle] {
        &self.client_bundles
    }

    pub fn discovered_plugins(&self) -> impl ExactSizeIterator<Item = LuaPluginDiscovery<'_>> + '_ {
        self.sources.iter().map(|source| LuaPluginDiscovery {
            id: source.manifest.plugin_id(),
            deployment: if source.client_bundles.is_empty() {
                LuaPluginDeployment::ServerOnly
            } else {
                LuaPluginDeployment::ServerAndClient
            },
        })
    }

    pub fn merge(mut self, other: Self) -> Result<Self, LuaHostError> {
        self.sources.extend(other.sources);
        prepare_plugin_sources(self.sources)
    }
}

impl LuaHost {
    pub fn loaded_plugins(&self) -> usize {
        self.loaded_plugins
    }

    pub fn join(self) -> thread::Result<()> {
        self.thread.join()
    }
}

/// Start one dedicated host thread for all Luau plugins in the configured directory.
pub fn start_lua_host(config: LuaHostConfig) -> Result<(ScriptBoundary, LuaHost), LuaHostError> {
    start_prepared_lua_host(prepare_lua_plugins(config)?)
}

pub fn prepare_lua_plugins(config: LuaHostConfig) -> Result<PreparedLuaPlugins, LuaHostError> {
    fs::create_dir_all(config.plugins_dir()).map_err(|error| LuaHostError::Io {
        path: config.plugins_dir().to_path_buf(),
        message: error.to_string(),
    })?;
    prepare_plugin_sources(discover_plugins(config.plugins_dir())?)
}

pub fn prepare_bundled_luau_plugins(
    plugins: &[BundledLuauPlugin],
) -> Result<PreparedLuaPlugins, LuaHostError> {
    if plugins.is_empty() {
        return prepare_plugin_sources(Vec::new());
    }
    let staging = tempfile::tempdir().map_err(|error| LuaHostError::Io {
        path: std::env::temp_dir(),
        message: format!("creating bundled Luau plugin staging directory: {error}"),
    })?;
    for plugin in plugins {
        validate_settlement_descriptor_id(plugin.directory_name, "bundled plugin directory")
            .map_err(|message| LuaHostError::InvalidStartupPlugin {
                path: PathBuf::from(plugin.directory_name),
                message,
            })?;
        let directory = staging.path().join(plugin.directory_name);
        fs::create_dir(&directory).map_err(|error| LuaHostError::Io {
            path: directory.clone(),
            message: error.to_string(),
        })?;
        for (name, contents) in [
            ("plugin.toml", plugin.manifest),
            ("main.lua", plugin.source),
        ] {
            let path = directory.join(name);
            fs::write(&path, contents).map_err(|error| LuaHostError::Io {
                path,
                message: error.to_string(),
            })?;
        }
        if let Some(config) = plugin.config {
            let path = directory.join("config.toml");
            fs::write(&path, config).map_err(|error| LuaHostError::Io {
                path,
                message: error.to_string(),
            })?;
        }
    }
    prepare_lua_plugins(LuaHostConfig::new(staging.path()))
}

fn prepare_plugin_sources(sources: Vec<PluginSource>) -> Result<PreparedLuaPlugins, LuaHostError> {
    let mut selected_ore = None;
    let mut ore_owner = None;
    let mut selected_settlement = None;
    let mut settlement_owner = None;
    let mut client_bundles = Vec::new();
    let mut plugin_ids = HashSet::new();
    for source in &sources {
        let plugin_id = source.manifest.plugin_id();
        if !plugin_ids.insert(plugin_id) {
            return Err(LuaHostError::PluginIdConflict {
                id: plugin_id.to_owned(),
            });
        }
        if let Some(profile) = source.worldgen_ore_profile {
            if let Some(first) = ore_owner {
                return Err(LuaHostError::WorldgenConflict {
                    kind: "ore",
                    first,
                    second: source.manifest.plugin_id().to_owned(),
                });
            }
            selected_ore = Some(profile);
            ore_owner = Some(source.manifest.plugin_id().to_owned());
        }
        if let Some(plan) = &source.worldgen_settlement_plan {
            if let Some(first) = settlement_owner {
                return Err(LuaHostError::WorldgenConflict {
                    kind: "settlement",
                    first,
                    second: source.manifest.plugin_id().to_owned(),
                });
            }
            selected_settlement = Some(plan.clone());
            settlement_owner = Some(source.manifest.plugin_id().to_owned());
        }
        client_bundles.extend(source.client_bundles.iter().cloned());
    }
    Ok(PreparedLuaPlugins {
        sources,
        worldgen_ore_profile: selected_ore,
        worldgen_settlement_plan: selected_settlement,
        client_bundles,
    })
}

pub fn start_prepared_lua_host(
    prepared: PreparedLuaPlugins,
) -> Result<(ScriptBoundary, LuaHost), LuaHostError> {
    let (boundary, endpoint) = script_boundary_pair(
        NonZeroUsize::new(EVENT_QUEUE_CAPACITY).expect("event queue capacity is non-zero"),
        NonZeroUsize::new(COMMAND_QUEUE_CAPACITY).expect("command queue capacity is non-zero"),
    );
    let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);
    let thread = thread::Builder::new()
        .name("solaris-luau-host".to_owned())
        .spawn(move || run_lua_host(endpoint, prepared.sources, startup_tx))
        .map_err(|error| LuaHostError::ThreadSpawn {
            message: error.to_string(),
        })?;
    let loaded_plugins = match startup_rx.recv() {
        Ok(loaded_plugins) => loaded_plugins,
        Err(_) => {
            let _ = thread.join();
            return Err(LuaHostError::StartupChannelClosed);
        }
    };
    Ok((
        boundary,
        LuaHost {
            loaded_plugins,
            thread,
        },
    ))
}

#[derive(Debug)]
struct PluginSource {
    manifest: ValidatedScriptPluginManifest,
    config: toml::Table,
    source: String,
    source_path: PathBuf,
    worldgen_ore_profile: Option<LuaWorldgenOreProfile>,
    worldgen_settlement_plan: Option<LuaSettlementPlan>,
    client_bundles: Vec<LuaClientBundle>,
}

#[derive(Debug)]
struct PluginSourceError {
    message: String,
    startup_contract_declared: bool,
}

impl PluginSourceError {
    fn new(message: impl Into<String>, startup_contract_declared: bool) -> Self {
        Self {
            message: message.into(),
            startup_contract_declared,
        }
    }

    #[cfg(test)]
    fn contains(&self, needle: &str) -> bool {
        self.message.contains(needle)
    }
}

impl std::fmt::Display for PluginSourceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiskManifest {
    id: String,
    name: String,
    version: String,
    api: String,
    #[serde(default)]
    events: Vec<String>,
    #[serde(default)]
    console_commands: Vec<String>,
    #[serde(default)]
    spawn_entities: Vec<String>,
    #[serde(default)]
    player_commands: Vec<String>,
    #[serde(default)]
    operator_commands: Vec<String>,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    dependencies: Vec<DiskDependency>,
    #[serde(default)]
    permissions: Vec<String>,
    worldgen: Option<DiskWorldgen>,
    client: Option<DiskClient>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiskClient {
    schema: u16,
    #[serde(default)]
    bundles: Vec<DiskClientBundle>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiskClientBundle {
    id: String,
    version: String,
    artifact: String,
    sha256: String,
    size_bytes: u64,
    loaders: Vec<DiskClientLoader>,
    content: Vec<DiskClientContentKind>,
    permissions: Vec<DiskClientPermission>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DiskClientLoader {
    Fabric,
    #[serde(rename = "neoforge")]
    NeoForge,
    Forge,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DiskClientContentKind {
    Blocks,
    Items,
    Screens,
    Assets,
    Interactions,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DiskClientPermission {
    RegisterBlocks,
    RegisterItems,
    OpenScreens,
    LoadAssets,
    SendInteractions,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiskWorldgen {
    #[serde(default)]
    ore_profile: Option<DiskWorldgenOreProfile>,
    #[serde(default)]
    settlement_profile: Option<DiskWorldgenSettlementProfile>,
    #[serde(default)]
    settlement_buildings: Vec<DiskSettlementBuilding>,
    #[serde(default)]
    settlement_inhabitants: Vec<DiskSettlementInhabitant>,
    #[serde(default)]
    settlement_extensions: Vec<DiskSettlementExtension>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DiskWorldgenOreProfile {
    GeologicalDeposits,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DiskWorldgenSettlementProfile {
    PlainsVillagePrototype,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiskSettlementBuilding {
    id: String,
    template: DiskSettlementBuildingTemplate,
    role: DiskSettlementBuildingRole,
}

#[derive(Debug, Deserialize)]
enum DiskSettlementBuildingTemplate {
    #[serde(rename = "plains_fountain")]
    Fountain,
    #[serde(rename = "plains_small_house")]
    SmallHouse,
    #[serde(rename = "plains_toolsmith")]
    Toolsmith,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DiskSettlementBuildingRole {
    MeetingPoint,
    Home,
    Workplace,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiskSettlementInhabitant {
    id: String,
    kind: DiskSettlementInhabitantKind,
    building: String,
    job: DiskSettlementJob,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DiskSettlementInhabitantKind {
    Villager,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DiskSettlementJob {
    Unemployed,
    Toolsmith,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiskSettlementExtension {
    id: String,
    building: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiskDependency {
    id: String,
    relation: DiskDependencyRelation,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DiskDependencyRelation {
    Required,
    Optional,
    LoadBefore,
}

fn discover_plugins(plugins_dir: &Path) -> Result<Vec<PluginSource>, LuaHostError> {
    let entries = fs::read_dir(plugins_dir).map_err(|error| LuaHostError::Io {
        path: plugins_dir.to_path_buf(),
        message: error.to_string(),
    })?;
    let mut directories = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) if entry.file_type().is_ok_and(|kind| kind.is_dir()) => {
                if directories.len() >= MAX_PLUGIN_DIRECTORIES {
                    return Err(LuaHostError::Io {
                        path: plugins_dir.to_path_buf(),
                        message: format!("plugin directory count exceeds {MAX_PLUGIN_DIRECTORIES}"),
                    });
                }
                directories.push(entry.path());
            }
            Ok(_) => {}
            Err(error) => {
                warn!(%error, directory = %plugins_dir.display(), "plugin directory entry ignored");
            }
        }
    }
    directories.sort();

    let mut sources = Vec::new();
    for directory in directories {
        match read_plugin_source(&directory) {
            Ok(source) => sources.push(source),
            Err(error) if error.startup_contract_declared => {
                return Err(LuaHostError::InvalidStartupPlugin {
                    path: directory,
                    message: error.message,
                });
            }
            Err(error) => warn!(
                directory = %directory.display(),
                %error,
                "Lua plugin skipped during discovery"
            ),
        }
    }
    Ok(sources)
}

fn read_plugin_source(directory: &Path) -> Result<PluginSource, PluginSourceError> {
    let manifest_path = directory.join("plugin.toml");
    let raw_manifest = read_utf8_file_limited(&manifest_path, MAX_PLUGIN_MANIFEST_BYTES)
        .map_err(|error| PluginSourceError::new(error, false))?;
    let raw_manifest: toml::Value = toml::from_str(&raw_manifest)
        .map_err(|error| PluginSourceError::new(format!("parsing manifest: {error}"), false))?;
    let startup_contract_declared =
        raw_manifest.get("worldgen").is_some() || raw_manifest.get("client").is_some();
    let disk: DiskManifest = raw_manifest.try_into().map_err(|error| {
        PluginSourceError::new(
            format!("parsing manifest: {error}"),
            startup_contract_declared,
        )
    })?;
    let requested_api_version = parse_api_version(&disk.api)
        .map_err(|error| PluginSourceError::new(error, startup_contract_declared))?;
    let (
        worldgen_ore_profile,
        worldgen_settlement_profile,
        settlement_buildings,
        settlement_inhabitants,
        settlement_extensions,
    ) = match disk.worldgen {
        Some(worldgen)
            if worldgen.ore_profile.is_none()
                && worldgen.settlement_profile.is_none()
                && worldgen.settlement_buildings.is_empty()
                && worldgen.settlement_inhabitants.is_empty()
                && worldgen.settlement_extensions.is_empty() =>
        {
            return Err(PluginSourceError::new(
                "worldgen must declare ore_profile or settlement_profile",
                true,
            ));
        }
        Some(worldgen) => {
            let ore_profile = worldgen.ore_profile.map(|profile| match profile {
                DiskWorldgenOreProfile::GeologicalDeposits => {
                    LuaWorldgenOreProfile::GeologicalDeposits
                }
            });
            let settlement_profile = worldgen.settlement_profile.map(|profile| match profile {
                DiskWorldgenSettlementProfile::PlainsVillagePrototype => {
                    LuaWorldgenSettlementProfile::PlainsVillagePrototype
                }
            });
            (
                ore_profile,
                settlement_profile,
                worldgen.settlement_buildings,
                worldgen.settlement_inhabitants,
                worldgen.settlement_extensions,
            )
        }
        None => (None, None, Vec::new(), Vec::new(), Vec::new()),
    };
    let mut manifest =
        ScriptPluginManifest::new(disk.id, disk.name, disk.version, requested_api_version);
    for event in disk.events {
        manifest = manifest.subscribe_event(event);
    }
    for root in disk.console_commands {
        manifest = manifest.declare_console_command_root(root);
    }
    for entity_type in disk.spawn_entities {
        manifest = manifest.declare_spawn_entity_type(entity_type);
    }
    for root in disk.player_commands {
        manifest = manifest.declare_player_command_root(root);
    }
    for root in disk.operator_commands {
        manifest = manifest.declare_operator_command_root(root);
    }
    for dependency in disk.dependencies {
        let relation = match dependency.relation {
            DiskDependencyRelation::Required => crate::ScriptPluginDependencyRelation::Required,
            DiskDependencyRelation::Optional => crate::ScriptPluginDependencyRelation::Optional,
            DiskDependencyRelation::LoadBefore => crate::ScriptPluginDependencyRelation::LoadBefore,
        };
        manifest = manifest.declare_dependency(dependency.id, relation);
    }
    for permission in disk.permissions {
        manifest = manifest.declare_permission(permission);
    }
    for capability in disk.capabilities {
        manifest = declare_disk_capability(manifest, &capability)
            .map_err(|error| PluginSourceError::new(error, startup_contract_declared))?;
    }
    let manifest = manifest.validate().map_err(|error| {
        PluginSourceError::new(
            format!("invalid manifest: {error:?}"),
            startup_contract_declared,
        )
    })?;
    let worldgen_settlement_plan = materialize_settlement_plan(
        manifest.plugin_id(),
        worldgen_settlement_profile,
        settlement_buildings,
        settlement_inhabitants,
        settlement_extensions,
    )
    .map_err(|error| PluginSourceError::new(error, startup_contract_declared))?;
    let client_bundles =
        materialize_client_bundles(directory, manifest.plugin_id(), disk.client)
            .map_err(|error| PluginSourceError::new(error, startup_contract_declared))?;
    let config = read_plugin_config(directory)
        .map_err(|error| PluginSourceError::new(error, startup_contract_declared))?;
    let source_path = directory.join("main.lua");
    let source = read_utf8_file_limited(&source_path, MAX_PLUGIN_SOURCE_BYTES)
        .map_err(|error| PluginSourceError::new(error, startup_contract_declared))?;
    let strict_source = format!("--!strict\n{SOLARIS_LUAU_PRELUDE}\n{source}");
    luaur::check(&strict_source).map_err(|error| {
        PluginSourceError::new(
            format!("Luau type check failed: {error:?}"),
            startup_contract_declared,
        )
    })?;
    Ok(PluginSource {
        manifest,
        config,
        source,
        source_path,
        worldgen_ore_profile,
        worldgen_settlement_plan,
        client_bundles,
    })
}

fn materialize_client_bundles(
    plugin_directory: &Path,
    owner_plugin_id: &str,
    client: Option<DiskClient>,
) -> Result<Vec<LuaClientBundle>, String> {
    let Some(client) = client else {
        return Ok(Vec::new());
    };
    if client.schema != CLIENT_MANIFEST_SCHEMA {
        return Err(format!(
            "client manifest schema must be {CLIENT_MANIFEST_SCHEMA}, got {}",
            client.schema
        ));
    }
    if client.bundles.is_empty() {
        return Err("client manifest must declare at least one bundle".to_owned());
    }
    if client.bundles.len() > MAX_CLIENT_BUNDLES_PER_PLUGIN {
        return Err(format!(
            "client bundles exceed {MAX_CLIENT_BUNDLES_PER_PLUGIN} entries"
        ));
    }

    let mut bundle_ids = HashSet::new();
    let mut bundles = Vec::with_capacity(client.bundles.len());
    for bundle in client.bundles {
        validate_client_literal(&bundle.id, "client bundle id", MAX_CLIENT_BUNDLE_ID_BYTES)?;
        if !bundle_ids.insert(bundle.id.clone()) {
            return Err(format!("duplicate client bundle id {:?}", bundle.id));
        }
        validate_client_literal(
            &bundle.version,
            "client bundle version",
            MAX_CLIENT_BUNDLE_VERSION_BYTES,
        )?;
        validate_client_artifact_path(&bundle.artifact)?;
        if bundle.sha256.len() != 64
            || !bundle
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(format!(
                "client bundle {:?} sha256 must be 64 lowercase hexadecimal characters",
                bundle.id
            ));
        }
        if bundle.size_bytes == 0 || bundle.size_bytes > MAX_CLIENT_BUNDLE_BYTES {
            return Err(format!(
                "client bundle {:?} size_bytes must be 1..={MAX_CLIENT_BUNDLE_BYTES}",
                bundle.id
            ));
        }

        let loaders = unique_client_values(
            bundle.loaders.into_iter().map(|loader| match loader {
                DiskClientLoader::Fabric => LuaClientLoader::Fabric,
                DiskClientLoader::NeoForge => LuaClientLoader::NeoForge,
                DiskClientLoader::Forge => LuaClientLoader::Forge,
            }),
            "loaders",
            &bundle.id,
        )?;
        let content = unique_client_values(
            bundle.content.into_iter().map(|content| match content {
                DiskClientContentKind::Blocks => LuaClientContentKind::Blocks,
                DiskClientContentKind::Items => LuaClientContentKind::Items,
                DiskClientContentKind::Screens => LuaClientContentKind::Screens,
                DiskClientContentKind::Assets => LuaClientContentKind::Assets,
                DiskClientContentKind::Interactions => LuaClientContentKind::Interactions,
            }),
            "content",
            &bundle.id,
        )?;
        let permissions = unique_client_values(
            bundle
                .permissions
                .into_iter()
                .map(|permission| match permission {
                    DiskClientPermission::RegisterBlocks => LuaClientPermission::RegisterBlocks,
                    DiskClientPermission::RegisterItems => LuaClientPermission::RegisterItems,
                    DiskClientPermission::OpenScreens => LuaClientPermission::OpenScreens,
                    DiskClientPermission::LoadAssets => LuaClientPermission::LoadAssets,
                    DiskClientPermission::SendInteractions => LuaClientPermission::SendInteractions,
                }),
            "permissions",
            &bundle.id,
        )?;
        for content_kind in &content {
            let required = content_kind.required_permission();
            if !permissions.contains(&required) {
                return Err(format!(
                    "client bundle {:?} content {:?} requires permission {:?}",
                    bundle.id,
                    content_kind.contract_name(),
                    required.contract_name()
                ));
            }
        }
        let artifact_path = validate_client_artifact(
            plugin_directory,
            &bundle.artifact,
            bundle.size_bytes,
            &bundle.sha256,
        )?;

        bundles.push(LuaClientBundle {
            owner_plugin_id: owner_plugin_id.to_owned(),
            id: bundle.id,
            version: bundle.version,
            artifact: bundle.artifact,
            sha256: bundle.sha256,
            size_bytes: bundle.size_bytes,
            artifact_path,
            loaders,
            content,
            permissions,
        });
    }
    Ok(bundles)
}

fn validate_client_artifact(
    plugin_directory: &Path,
    relative_path: &str,
    declared_size: u64,
    declared_sha256: &str,
) -> Result<PathBuf, String> {
    let plugin_root = fs::canonicalize(plugin_directory).map_err(|error| {
        format!(
            "canonicalizing plugin directory {}: {error}",
            plugin_directory.display()
        )
    })?;
    let artifact_path = plugin_directory.join(relative_path);
    let canonical = fs::canonicalize(&artifact_path).map_err(|error| {
        format!(
            "opening client artifact {}: {error}",
            artifact_path.display()
        )
    })?;
    if !canonical.starts_with(&plugin_root) {
        return Err(format!(
            "client artifact {} escapes the plugin directory",
            artifact_path.display()
        ));
    }
    let metadata = fs::metadata(&canonical)
        .map_err(|error| format!("reading client artifact metadata: {error}"))?;
    if !metadata.is_file() {
        return Err(format!(
            "client artifact {} is not a regular file",
            artifact_path.display()
        ));
    }
    if metadata.len() != declared_size {
        return Err(format!(
            "client artifact {} has {} bytes, manifest declares {declared_size}",
            artifact_path.display(),
            metadata.len()
        ));
    }
    let mut file = fs::File::open(&canonical).map_err(|error| {
        format!(
            "opening client artifact {}: {error}",
            artifact_path.display()
        )
    })?;
    let mut digest = Sha256::new();
    let copied = std::io::copy(&mut file, &mut digest).map_err(|error| {
        format!(
            "hashing client artifact {}: {error}",
            artifact_path.display()
        )
    })?;
    if copied != declared_size {
        return Err(format!(
            "client artifact {} changed while hashing",
            artifact_path.display()
        ));
    }
    let actual_sha256 = format!("{:x}", digest.finalize());
    if actual_sha256 != declared_sha256 {
        return Err(format!(
            "client artifact {} SHA-256 does not match the manifest",
            artifact_path.display()
        ));
    }
    Ok(canonical)
}

fn unique_client_values<T>(
    values: impl IntoIterator<Item = T>,
    field: &str,
    bundle_id: &str,
) -> Result<Vec<T>, String>
where
    T: Copy + Eq + std::hash::Hash,
{
    let mut unique = HashSet::new();
    let mut result = Vec::new();
    for value in values {
        if !unique.insert(value) {
            return Err(format!(
                "client bundle {bundle_id:?} contains duplicate {field}"
            ));
        }
        result.push(value);
    }
    if result.is_empty() {
        return Err(format!(
            "client bundle {bundle_id:?} must declare at least one {field}"
        ));
    }
    Ok(result)
}

fn validate_client_literal(value: &str, field: &str, max_bytes: usize) -> Result<(), String> {
    if value.is_empty() || value.len() > max_bytes {
        return Err(format!("{field} must contain 1..={max_bytes} bytes"));
    }
    if matches!(value, "." | "..") {
        return Err(format!("{field} cannot be a relative path segment"));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
    {
        return Err(format!("{field} {value:?} contains invalid characters"));
    }
    Ok(())
}

fn validate_client_artifact_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.len() > MAX_CLIENT_ARTIFACT_PATH_BYTES
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
        || !path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_./-".contains(&byte))
    {
        return Err(format!(
            "client artifact path {path:?} must be a bounded relative ASCII path"
        ));
    }
    Ok(())
}

fn materialize_settlement_plan(
    owner_plugin_id: &str,
    profile: Option<LuaWorldgenSettlementProfile>,
    buildings: Vec<DiskSettlementBuilding>,
    inhabitants: Vec<DiskSettlementInhabitant>,
    extensions: Vec<DiskSettlementExtension>,
) -> Result<Option<LuaSettlementPlan>, String> {
    let Some(profile) = profile else {
        if buildings.is_empty() && inhabitants.is_empty() && extensions.is_empty() {
            return Ok(None);
        }
        return Err("settlement descriptors require settlement_profile".to_owned());
    };
    if buildings.len() > MAX_SETTLEMENT_BUILDINGS {
        return Err(format!(
            "settlement_buildings exceeds {MAX_SETTLEMENT_BUILDINGS} entries"
        ));
    }
    if inhabitants.len() > MAX_SETTLEMENT_INHABITANTS {
        return Err(format!(
            "settlement_inhabitants exceeds {MAX_SETTLEMENT_INHABITANTS} entries"
        ));
    }
    if extensions.len() > MAX_SETTLEMENT_EXTENSIONS {
        return Err(format!(
            "settlement_extensions exceeds {MAX_SETTLEMENT_EXTENSIONS} entries"
        ));
    }

    let buildings = if buildings.is_empty() {
        default_plains_village_buildings()
    } else {
        let mut ids = HashSet::new();
        let mut templates = HashSet::new();
        let mut materialized = Vec::with_capacity(buildings.len());
        for building in buildings {
            validate_settlement_descriptor_id(&building.id, "settlement building id")?;
            if !ids.insert(building.id.clone()) {
                return Err(format!(
                    "duplicate settlement building id {:?}",
                    building.id
                ));
            }
            let template = match building.template {
                DiskSettlementBuildingTemplate::Fountain => {
                    LuaSettlementBuildingTemplate::PlainsFountain
                }
                DiskSettlementBuildingTemplate::SmallHouse => {
                    LuaSettlementBuildingTemplate::PlainsSmallHouse
                }
                DiskSettlementBuildingTemplate::Toolsmith => {
                    LuaSettlementBuildingTemplate::PlainsToolsmith
                }
            };
            if !templates.insert(template) {
                return Err(format!(
                    "duplicate settlement building template {:?}",
                    template.contract_name()
                ));
            }
            let role = match building.role {
                DiskSettlementBuildingRole::MeetingPoint => LuaSettlementBuildingRole::MeetingPoint,
                DiskSettlementBuildingRole::Home => LuaSettlementBuildingRole::Home,
                DiskSettlementBuildingRole::Workplace => LuaSettlementBuildingRole::Workplace,
            };
            materialized.push(LuaSettlementBuilding {
                id: building.id,
                template,
                role,
            });
        }
        materialized
    };
    let building_ids = buildings
        .iter()
        .map(|building| building.id.as_str())
        .collect::<HashSet<_>>();

    let mut inhabitant_ids = HashSet::new();
    let mut materialized_inhabitants = Vec::with_capacity(inhabitants.len());
    for inhabitant in inhabitants {
        validate_settlement_descriptor_id(&inhabitant.id, "settlement inhabitant id")?;
        if !inhabitant_ids.insert(inhabitant.id.clone()) {
            return Err(format!(
                "duplicate settlement inhabitant id {:?}",
                inhabitant.id
            ));
        }
        if !building_ids.contains(inhabitant.building.as_str()) {
            return Err(format!(
                "settlement inhabitant {:?} references unknown building {:?}",
                inhabitant.id, inhabitant.building
            ));
        }
        let kind = match inhabitant.kind {
            DiskSettlementInhabitantKind::Villager => LuaSettlementInhabitantKind::Villager,
        };
        let job = match inhabitant.job {
            DiskSettlementJob::Unemployed => LuaSettlementJob::Unemployed,
            DiskSettlementJob::Toolsmith => LuaSettlementJob::Toolsmith,
        };
        materialized_inhabitants.push(LuaSettlementInhabitant {
            id: inhabitant.id,
            kind,
            building_id: inhabitant.building,
            job,
        });
    }

    let mut extension_ids = HashSet::new();
    let mut materialized_extensions = Vec::with_capacity(extensions.len());
    for extension in extensions {
        validate_settlement_descriptor_id(&extension.id, "settlement extension id")?;
        if !extension_ids.insert(extension.id.clone()) {
            return Err(format!(
                "duplicate settlement extension id {:?}",
                extension.id
            ));
        }
        if !building_ids.contains(extension.building.as_str()) {
            return Err(format!(
                "settlement extension {:?} references unknown building {:?}",
                extension.id, extension.building
            ));
        }
        materialized_extensions.push(LuaSettlementExtension {
            id: format!("{owner_plugin_id}:{}", extension.id),
            building_id: extension.building,
        });
    }

    Ok(Some(LuaSettlementPlan {
        owner_plugin_id: owner_plugin_id.to_owned(),
        profile,
        buildings,
        inhabitants: materialized_inhabitants,
        extensions: materialized_extensions,
    }))
}

fn default_plains_village_buildings() -> Vec<LuaSettlementBuilding> {
    [
        (
            "meeting-point",
            LuaSettlementBuildingTemplate::PlainsFountain,
            LuaSettlementBuildingRole::MeetingPoint,
        ),
        (
            "home",
            LuaSettlementBuildingTemplate::PlainsSmallHouse,
            LuaSettlementBuildingRole::Home,
        ),
        (
            "toolsmith",
            LuaSettlementBuildingTemplate::PlainsToolsmith,
            LuaSettlementBuildingRole::Workplace,
        ),
    ]
    .into_iter()
    .map(|(id, template, role)| LuaSettlementBuilding {
        id: id.to_owned(),
        template,
        role,
    })
    .collect()
}

fn validate_settlement_descriptor_id(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > MAX_SETTLEMENT_DESCRIPTOR_ID_BYTES {
        return Err(format!(
            "{field} must contain 1..={MAX_SETTLEMENT_DESCRIPTOR_ID_BYTES} bytes"
        ));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_.-".contains(&byte))
    {
        return Err(format!("{field} {value:?} contains invalid characters"));
    }
    Ok(())
}

fn read_plugin_config(directory: &Path) -> Result<toml::Table, String> {
    let path = directory.join("config.toml");
    match fs::metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(toml::Table::new());
        }
        Err(error) => return Err(format!("reading {} metadata: {error}", path.display())),
    }
    let raw = read_utf8_file_limited(&path, MAX_PLUGIN_CONFIG_BYTES)?;
    let config = toml::from_str(&raw).map_err(|error| format!("parsing config: {error}"))?;
    validate_plugin_config_table(&config, 0)?;
    Ok(config)
}

fn validate_plugin_config_table(config: &toml::Table, depth: usize) -> Result<(), String> {
    if depth > MAX_PLUGIN_CONFIG_DEPTH {
        return Err(format!(
            "config nesting exceeds {MAX_PLUGIN_CONFIG_DEPTH} levels"
        ));
    }
    if config.len() > MAX_PLUGIN_CONFIG_CONTAINER_ENTRIES {
        return Err(format!(
            "config table exceeds {MAX_PLUGIN_CONFIG_CONTAINER_ENTRIES} entries"
        ));
    }
    for (key, value) in config {
        if key.len() > MAX_PLUGIN_CONFIG_KEY_BYTES {
            return Err(format!(
                "config key exceeds {MAX_PLUGIN_CONFIG_KEY_BYTES} bytes"
            ));
        }
        validate_plugin_config_value(value, depth)?;
    }
    Ok(())
}

fn validate_plugin_config_value(value: &toml::Value, depth: usize) -> Result<(), String> {
    match value {
        toml::Value::String(value) if value.len() > MAX_PLUGIN_CONFIG_STRING_BYTES => Err(format!(
            "config string exceeds {MAX_PLUGIN_CONFIG_STRING_BYTES} bytes"
        )),
        toml::Value::String(_) | toml::Value::Integer(_) | toml::Value::Boolean(_) => Ok(()),
        toml::Value::Float(value) if !value.is_finite() => {
            Err("config floating-point values must be finite".to_owned())
        }
        toml::Value::Float(_) => Ok(()),
        toml::Value::Array(values) => {
            if values.len() > MAX_PLUGIN_CONFIG_CONTAINER_ENTRIES {
                return Err(format!(
                    "config array exceeds {MAX_PLUGIN_CONFIG_CONTAINER_ENTRIES} entries"
                ));
            }
            let depth = depth.saturating_add(1);
            if depth > MAX_PLUGIN_CONFIG_DEPTH {
                return Err(format!(
                    "config nesting exceeds {MAX_PLUGIN_CONFIG_DEPTH} levels"
                ));
            }
            for value in values {
                validate_plugin_config_value(value, depth)?;
            }
            Ok(())
        }
        toml::Value::Table(values) => validate_plugin_config_table(values, depth.saturating_add(1)),
        toml::Value::Datetime(_) => Err("config datetime values are unsupported".to_owned()),
    }
}

fn read_utf8_file_limited(path: &Path, max_bytes: usize) -> Result<String, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("reading {} metadata: {error}", path.display()))?;
    if metadata.len() > max_bytes as u64 {
        return Err(format!(
            "{} exceeds {max_bytes} bytes",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("plugin file")
        ));
    }
    let file =
        fs::File::open(path).map_err(|error| format!("opening {}: {error}", path.display()))?;
    let capacity = usize::try_from(metadata.len())
        .unwrap_or(max_bytes)
        .min(max_bytes);
    let mut bytes = Vec::with_capacity(capacity);
    file.take(
        u64::try_from(max_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1),
    )
    .read_to_end(&mut bytes)
    .map_err(|error| format!("reading {}: {error}", path.display()))?;
    if bytes.len() > max_bytes {
        return Err(format!(
            "{} exceeds {max_bytes} bytes",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("plugin file")
        ));
    }
    String::from_utf8(bytes).map_err(|error| format!("{} is not UTF-8: {error}", path.display()))
}

fn declare_disk_capability(
    manifest: ScriptPluginManifest,
    capability: &str,
) -> Result<ScriptPluginManifest, String> {
    match capability {
        "storage" => Ok(manifest.declare_plugin_storage()),
        "inventory_menus" => Ok(manifest.declare_inventory_menus()),
        "inventory_storage_transactions" => Ok(manifest.declare_inventory_storage_transactions()),
        "player_inventory" => Ok(manifest.declare_player_inventory()),
        "zones" => Ok(manifest.declare_zones()),
        "villagers" => Ok(manifest.declare_villagers()),
        "player_teleport" => Ok(manifest.declare_player_teleport()),
        "player_queries" => Ok(manifest.declare_player_queries()),
        _ => Err(format!("unknown plugin capability {capability:?}")),
    }
}

fn parse_api_version(value: &str) -> Result<ScriptApiVersion, String> {
    if value.len() > MAX_API_VERSION_BYTES {
        return Err(format!("api version exceeds {MAX_API_VERSION_BYTES} bytes"));
    }
    let mut parts = value.split('.');
    let (Some(major), Some(minor), Some(patch)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(format!(
            "api version must be MAJOR.MINOR.PATCH, got {value:?}"
        ));
    };
    if parts.next().is_some() {
        return Err(format!(
            "api version must be MAJOR.MINOR.PATCH, got {value:?}"
        ));
    }
    let parse = |part: &str| {
        part.parse::<u16>()
            .map_err(|_| format!("invalid api version {value:?}"))
    };
    Ok(ScriptApiVersion::new(
        parse(major)?,
        parse(minor)?,
        parse(patch)?,
    ))
}

fn run_lua_host(
    mut endpoint: ScriptHostEndpoint,
    sources: Vec<PluginSource>,
    startup: std::sync::mpsc::SyncSender<usize>,
) {
    run_lua_host_inner(&mut endpoint, sources, startup, None);
}

fn run_lua_host_inner(
    endpoint: &mut ScriptHostEndpoint,
    sources: Vec<PluginSource>,
    startup: std::sync::mpsc::SyncSender<usize>,
    progress: Option<std::sync::mpsc::SyncSender<&'static str>>,
) {
    let mut plugins = Vec::new();
    let mut loaded_plugin_ids = HashSet::new();
    for source in sources {
        let plugin_id = source.manifest.plugin_id().to_owned();
        if loaded_plugin_ids.contains(&plugin_id) {
            warn!(plugin = %plugin_id, "Lua plugin skipped because its id is already loaded");
            continue;
        }
        if let Err(error) = endpoint.register_player_commands(&source.manifest) {
            match error {
                PlayerCommandRegistrationError::RootConflict {
                    root,
                    owner_plugin_id,
                } => warn!(
                    plugin = %plugin_id,
                    %root,
                    owner = %owner_plugin_id,
                    "Lua plugin skipped because its player command root is already owned"
                ),
                PlayerCommandRegistrationError::RootLimitExceeded { limit, requested } => warn!(
                    plugin = %plugin_id,
                    limit,
                    requested,
                    "Lua plugin skipped because the aggregate player command limit was exceeded"
                ),
                PlayerCommandRegistrationError::AuthorityPoisoned => {
                    warn!("Lua host disabled because player-command authority was poisoned");
                    let _ = startup.send(0);
                    return;
                }
            }
            continue;
        }
        match LuaPlugin::new(source) {
            Ok(plugin) => {
                info!(plugin = %plugin_id, "Lua plugin loaded");
                loaded_plugin_ids.insert(plugin_id);
                plugins.push(plugin);
            }
            Err(error) => {
                endpoint.unregister_player_commands(&plugin_id);
                warn!(plugin = %plugin_id, %error, "Lua plugin skipped during load");
            }
        }
    }
    let _ = startup.send(plugins.len());

    while let Some(event) = endpoint.recv_event_blocking() {
        for plugin in &mut plugins {
            let batch = match plugin.handle_event(&event) {
                Ok(batch) => batch,
                Err(error) => {
                    warn!(plugin = %plugin.id, ?error, "Lua plugin disabled after handler failure");
                    endpoint.unregister_player_commands(&plugin.id);
                    plugin.disabled = true;
                    continue;
                }
            };
            match endpoint.try_submit_plugin_batch(&plugin.admission, batch) {
                Ok(()) => {}
                Err(ScriptBatchSubmissionError::Full(batch)) => {
                    let command_count = batch.commands().len();
                    warn!(plugin = %plugin.id, command_count, "Lua command batch rejected because the queue is full");
                    if let Err(error) = plugin.notify_batch_rejected("queue_full", command_count) {
                        warn!(plugin = %plugin.id, ?error, "Lua plugin disabled after batch-rejection handler failure");
                        endpoint.unregister_player_commands(&plugin.id);
                        plugin.disabled = true;
                    }
                }
                Err(ScriptBatchSubmissionError::Closed(batch)) => {
                    let command_count = batch.commands().len();
                    let _ = plugin.notify_batch_rejected("queue_closed", command_count);
                    return;
                }
                Err(ScriptBatchSubmissionError::Rejected { error, .. }) => {
                    warn!(plugin = %plugin.id, ?error, "Lua plugin disabled after command admission rejection");
                    endpoint.unregister_player_commands(&plugin.id);
                    plugin.disabled = true;
                }
            }
        }
        if let Some(progress) = &progress
            && progress.send(event.event_name()).is_err()
        {
            return;
        }
    }
}

struct LuaPlugin {
    id: String,
    subscriptions: HashSet<String>,
    admission: HostCommandAdmission,
    runtime: LuaScriptRuntime,
    disabled: bool,
}

impl LuaPlugin {
    fn new(source: PluginSource) -> Result<Self, String> {
        let id = source.manifest.plugin_id().to_owned();
        let subscriptions = source
            .manifest
            .event_subscriptions()
            .iter()
            .map(|subscription| subscription.event_name().to_owned())
            .collect();
        let admission = HostCommandAdmission::from_manifest(&source.manifest);
        let runtime = LuaScriptRuntime::from_source_with_config(
            source.manifest,
            &source.source,
            source.config,
            LuaRuntimeLimits::default(),
        )
        .map_err(|error| format!("{}: {error}", source.source_path.display()))?;
        Ok(Self {
            id,
            subscriptions,
            admission,
            runtime,
            disabled: false,
        })
    }

    fn handle_event(&mut self, event: &ScriptEvent) -> RuntimeResult<CommandBatch> {
        if self.disabled {
            return Ok(empty_lua_command_batch());
        }
        if let Some(target_plugin_id) = event.target_plugin_id() {
            if target_plugin_id != self.id {
                return Ok(empty_lua_command_batch());
            }
        } else if !matches!(event.kind(), ScriptEventKind::ServerTick { .. })
            && !self.subscriptions.contains(event.event_name())
        {
            return Ok(empty_lua_command_batch());
        }
        let controls = crate::RuntimeControls::unrestricted();
        self.runtime.handle_event(
            event,
            RuntimeContext::new(
                &controls,
                NonZeroUsize::new(COMMANDS_PER_EVENT).expect("commands per event is non-zero"),
            ),
        )
    }

    fn notify_batch_rejected(
        &mut self,
        reason: &'static str,
        command_count: usize,
    ) -> RuntimeResult<()> {
        self.runtime.notify_batch_rejected(reason, command_count)
    }
}

fn empty_lua_command_batch() -> CommandBatch {
    CommandBatch::new(
        NonZeroUsize::new(COMMANDS_PER_EVENT).expect("commands per event is non-zero"),
    )
}

#[derive(Debug, Clone, Copy)]
struct LuaRuntimeLimits {
    instructions_per_event: NonZeroU64,
    memory_bytes: NonZeroUsize,
}

impl Default for LuaRuntimeLimits {
    fn default() -> Self {
        Self {
            instructions_per_event: NonZeroU64::new(INSTRUCTIONS_PER_EVENT)
                .expect("instruction limit is non-zero"),
            memory_bytes: NonZeroUsize::new(MEMORY_BYTES_PER_PLUGIN)
                .expect("memory limit is non-zero"),
        }
    }
}

struct InvocationState {
    batch: CommandBatch,
    capabilities: Arc<CommandCapabilities>,
    timers: TimerSchedule,
    current_tick: u64,
}

#[derive(Clone, Default)]
struct TimerSchedule {
    by_id: HashMap<String, u64>,
    by_deadline: BTreeSet<(u64, String)>,
}

impl TimerSchedule {
    fn schedule(&mut self, timer_id: String, deadline: u64) -> Result<(), ()> {
        if !self.by_id.contains_key(&timer_id) && self.by_id.len() >= MAX_PENDING_PLUGIN_TIMERS {
            return Err(());
        }
        if let Some(previous) = self.by_id.insert(timer_id.clone(), deadline) {
            self.by_deadline.remove(&(previous, timer_id.clone()));
        }
        self.by_deadline.insert((deadline, timer_id));
        Ok(())
    }

    fn cancel(&mut self, timer_id: &str) -> bool {
        let Some(deadline) = self.by_id.remove(timer_id) else {
            return false;
        };
        self.by_deadline.remove(&(deadline, timer_id.to_owned()));
        true
    }

    fn take_next_due(&mut self, current_tick: u64) -> Option<(u64, String)> {
        let (deadline, timer_id) = self.by_deadline.first()?.clone();
        if deadline > current_tick {
            return None;
        }
        self.by_deadline.remove(&(deadline, timer_id.clone()));
        self.by_id.remove(&timer_id);
        Some((deadline, timer_id))
    }
}

struct LuaScriptRuntime {
    lua: Lua,
    manifest: ValidatedScriptPluginManifest,
    invocation: Arc<Mutex<Option<InvocationState>>>,
    capabilities: Arc<CommandCapabilities>,
    timers: TimerSchedule,
    current_tick: u64,
    limits: LuaRuntimeLimits,
}

impl LuaScriptRuntime {
    #[cfg(test)]
    fn from_source(
        manifest: ValidatedScriptPluginManifest,
        source: &str,
        limits: LuaRuntimeLimits,
    ) -> Result<Self, String> {
        Self::from_source_with_config(manifest, source, toml::Table::new(), limits)
    }

    fn from_source_with_config(
        manifest: ValidatedScriptPluginManifest,
        source: &str,
        config: toml::Table,
        limits: LuaRuntimeLimits,
    ) -> Result<Self, String> {
        let libraries = StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8;
        let lua = Lua::new_with(libraries, LuaOptions::default()).map_err(lua_error)?;
        lua.set_memory_limit(limits.memory_bytes.get())
            .map_err(lua_error)?;
        let invocation = Arc::new(Mutex::new(None));
        let capabilities = Arc::new(manifest.to_command_capabilities());
        install_solaris_api(&lua, Arc::clone(&invocation), config).map_err(lua_error)?;
        lua.sandbox(true).map_err(lua_error)?;
        run_with_instruction_budget(&lua, limits.instructions_per_event, || {
            lua.load(source).set_name(manifest.plugin_id()).exec()
        })
        .map_err(lua_error)?;
        Ok(Self {
            lua,
            manifest,
            invocation,
            capabilities,
            timers: TimerSchedule::default(),
            current_tick: 0,
            limits,
        })
    }

    #[cfg(test)]
    fn pending_timer_count(&self) -> usize {
        self.timers.by_id.len()
    }

    fn notify_batch_rejected(
        &self,
        reason: &'static str,
        command_count: usize,
    ) -> RuntimeResult<()> {
        let handler = self
            .lua
            .globals()
            .get::<Option<Function>>("on_command_batch_rejected")
            .map_err(runtime_error)?;
        let Some(handler) = handler else {
            return Ok(());
        };
        let result = self.lua.create_table().map_err(runtime_error)?;
        result.set("reason", reason).map_err(runtime_error)?;
        result
            .set("command_count", command_count)
            .map_err(runtime_error)?;
        run_with_instruction_budget(&self.lua, self.limits.instructions_per_event, || {
            handler.call::<()>(result)
        })
        .map_err(runtime_error)
    }

    fn invoke_handler(
        &mut self,
        handler_name: &str,
        event_table: Table,
        batch: CommandBatch,
    ) -> RuntimeResult<CommandBatch> {
        let handler = self
            .lua
            .globals()
            .get::<Option<Function>>(handler_name)
            .map_err(runtime_error)?;
        let Some(handler) = handler else {
            return Ok(batch);
        };
        *lock_invocation(&self.invocation).map_err(runtime_error)? = Some(InvocationState {
            batch,
            capabilities: Arc::clone(&self.capabilities),
            timers: self.timers.clone(),
            current_tick: self.current_tick,
        });
        let result = handler.call::<()>(event_table);
        let invocation = lock_invocation(&self.invocation)
            .map_err(runtime_error)?
            .take()
            .expect("Lua invocation state exists while a handler runs");
        match result {
            Ok(()) => {
                self.timers = invocation.timers;
                Ok(invocation.batch)
            }
            Err(error) => Err(runtime_error(error)),
        }
    }

    fn dispatch_event(
        &mut self,
        event: &ScriptEvent,
        context: RuntimeContext<'_>,
    ) -> RuntimeResult<CommandBatch> {
        if let ScriptEventKind::ServerTick { tick } = event.kind() {
            self.current_tick = self.current_tick.max(*tick);
            let mut batch = context.command_batch();
            for _ in 0..MAX_PLUGIN_TIMER_CALLBACKS_PER_TICK {
                let Some((scheduled_tick, timer_id)) = self.timers.take_next_due(self.current_tick)
                else {
                    break;
                };
                let timer_event =
                    timer_event_table(&self.lua, &timer_id, scheduled_tick, self.current_tick)
                        .map_err(runtime_error)?;
                batch = self.invoke_handler("on_plugin_timer", timer_event, batch)?;
            }
            if self
                .manifest
                .event_subscriptions()
                .iter()
                .any(|subscription| subscription.event_name() == event.event_name())
            {
                let event_table = event_table(&self.lua, event).map_err(runtime_error)?;
                batch = self.invoke_handler(handler_name(event), event_table, batch)?;
            }
            return Ok(batch);
        }
        if let Some(target_plugin_id) = event.target_plugin_id() {
            if target_plugin_id != self.manifest.plugin_id() {
                return Ok(context.command_batch());
            }
        } else if !self
            .manifest
            .event_subscriptions()
            .iter()
            .any(|subscription| subscription.event_name() == event.event_name())
        {
            return Ok(context.command_batch());
        }
        let event_table = event_table(&self.lua, event).map_err(runtime_error)?;
        self.invoke_handler(handler_name(event), event_table, context.command_batch())
    }
}

impl ScriptRuntime for LuaScriptRuntime {
    fn handle_event(
        &mut self,
        event: &ScriptEvent,
        context: RuntimeContext<'_>,
    ) -> RuntimeResult<CommandBatch> {
        if context.controls().shutdown_requested() {
            return Err(RuntimeError::ShutdownRequested);
        }
        let configured_budget = self.limits.instructions_per_event;
        let budget = context.controls().fuel().map_or(configured_budget, |fuel| {
            NonZeroU64::new(fuel.get().min(configured_budget.get()))
                .expect("minimum of non-zero budgets is non-zero")
        });
        install_instruction_budget_hook(&self.lua, budget).map_err(runtime_error)?;
        let result = self.dispatch_event(event, context);
        self.lua.remove_interrupt();
        result
    }
}

fn install_solaris_api(
    lua: &Lua,
    invocation: Arc<Mutex<Option<InvocationState>>>,
    config: toml::Table,
) -> mlua::Result<()> {
    let api = lua.create_table()?;
    api.set(
        "config",
        lua.create_function(move |lua, ()| config_table_to_lua(lua, &config))?,
    )?;
    let schedule_timer_invocation = Arc::clone(&invocation);
    api.set(
        "schedule_timer",
        lua.create_function(
            move |_, (timer_id, delay): (LuaString, Value)| -> mlua::Result<u64> {
                let timer_id = bounded_script_id(timer_id, "timer_id")?;
                let delay = match delay {
                    Value::Integer(delay) => u64::try_from(delay)
                        .ok()
                        .filter(|delay| (1..=MAX_PLUGIN_TIMER_DELAY_TICKS).contains(delay))
                        .ok_or_else(|| lua_input_error("timer_delay_ticks", "range"))?,
                    _ => return Err(lua_input_error("timer_delay_ticks", "type")),
                };
                let mut invocation = lock_invocation(&schedule_timer_invocation)?;
                let invocation = invocation.as_mut().ok_or_else(|| {
                    mlua::Error::runtime("Solaris API called outside an event handler")
                })?;
                let deadline = invocation
                    .current_tick
                    .checked_add(delay)
                    .ok_or_else(|| lua_input_error("timer_delay_ticks", "range"))?;
                invocation
                    .timers
                    .schedule(timer_id, deadline)
                    .map_err(|()| lua_input_error("plugin_timers", "too_many"))?;
                Ok(deadline)
            },
        )?,
    )?;
    let cancel_timer_invocation = Arc::clone(&invocation);
    api.set(
        "cancel_timer",
        lua.create_function(move |_, timer_id: LuaString| {
            let timer_id = bounded_script_id(timer_id, "timer_id")?;
            let mut invocation = lock_invocation(&cancel_timer_invocation)?;
            let invocation = invocation.as_mut().ok_or_else(|| {
                mlua::Error::runtime("Solaris API called outside an event handler")
            })?;
            Ok(invocation.timers.cancel(&timer_id))
        })?,
    )?;
    let send_invocation = Arc::clone(&invocation);
    api.set(
        "send_message",
        lua.create_function(move |_, (player_id, message): (u64, LuaString)| {
            let message = bounded_lua_string(
                message,
                "chat_message",
                MAX_SCRIPT_CHAT_MESSAGE_BYTES,
                false,
            )?;
            push_command(
                &send_invocation,
                ScriptCommand::SendChatMessage {
                    player_id: ScriptPlayerId::new(player_id),
                    message,
                },
            )
        })?,
    )?;
    let broadcast_invocation = Arc::clone(&invocation);
    api.set(
        "broadcast",
        lua.create_function(move |_, message: LuaString| {
            let message = bounded_lua_string(
                message,
                "chat_message",
                MAX_SCRIPT_CHAT_MESSAGE_BYTES,
                false,
            )?;
            push_command(
                &broadcast_invocation,
                ScriptCommand::BroadcastChatMessage { message },
            )
        })?,
    )?;
    let disconnect_invocation = Arc::clone(&invocation);
    api.set(
        "disconnect",
        lua.create_function(move |_, (player_id, reason): (u64, LuaString)| {
            let reason = bounded_lua_string(
                reason,
                "disconnect_reason",
                MAX_SCRIPT_DISCONNECT_REASON_BYTES,
                false,
            )?;
            push_command(
                &disconnect_invocation,
                ScriptCommand::DisconnectPlayer {
                    player_id: ScriptPlayerId::new(player_id),
                    reason,
                },
            )
        })?,
    )?;
    let console_invocation = Arc::clone(&invocation);
    api.set(
        "run_console",
        lua.create_function(move |_, command: LuaString| {
            let command = bounded_lua_string(
                command,
                "console_command",
                MAX_SCRIPT_CONSOLE_COMMAND_BYTES,
                false,
            )?;
            push_command(
                &console_invocation,
                ScriptCommand::RunConsoleCommand { command },
            )
        })?,
    )?;
    let spawn_invocation = Arc::clone(&invocation);
    api.set(
        "spawn_entity",
        lua.create_function(
            move |_, (actor, entity_type, x, y, z): (u64, LuaString, f64, f64, f64)| {
                let entity_type = bounded_lua_string(
                    entity_type,
                    "entity_type",
                    crate::MAX_SCRIPT_RESOURCE_ID_BYTES,
                    false,
                )?;
                crate::validate_script_resource_id(&entity_type)
                    .map_err(|_| lua_input_error("entity_type", "invalid"))?;
                let position = ScriptPosition::try_new(x, y, z)
                    .ok_or_else(|| lua_input_error("spawn_position", "invalid"))?;
                push_command(
                    &spawn_invocation,
                    ScriptCommand::SpawnEntity {
                        actor: ScriptPlayerId::new(actor),
                        entity_type,
                        position,
                    },
                )
            },
        )?,
    )?;
    let storage_get_invocation = Arc::clone(&invocation);
    api.set(
        "storage_get",
        lua.create_function(move |_, (request_id, key): (LuaString, LuaString)| {
            let request_id = bounded_script_id(request_id, "request_id")?;
            let key = bounded_lua_string(
                key,
                "storage_key",
                crate::MAX_PLUGIN_STORAGE_KEY_BYTES,
                false,
            )?;
            let request =
                ScriptPluginStorageGetRequest::try_new(request_id, key).map_err(dto_error)?;
            push_command(
                &storage_get_invocation,
                ScriptCommand::PluginStorageGet { request },
            )
        })?,
    )?;
    let storage_cas_invocation = Arc::clone(&invocation);
    api.set(
        "storage_cas",
        lua.create_function(
            move |_,
                  (request_id, key, expected_version, value): (
                LuaString,
                LuaString,
                Option<u64>,
                LuaString,
            )| {
                let request_id = bounded_script_id(request_id, "request_id")?;
                let key = bounded_lua_string(
                    key,
                    "storage_key",
                    crate::MAX_PLUGIN_STORAGE_KEY_BYTES,
                    false,
                )?;
                let value = bounded_lua_string(
                    value,
                    "storage_value",
                    crate::MAX_PLUGIN_STORAGE_VALUE_BYTES,
                    false,
                )?;
                let request = ScriptPluginStorageCompareAndSwapRequest::try_new(
                    request_id,
                    key,
                    expected_version,
                    value,
                )
                .map_err(dto_error)?;
                push_command(
                    &storage_cas_invocation,
                    ScriptCommand::PluginStorageCompareAndSwap { request },
                )
            },
        )?,
    )?;
    let storage_delete_invocation = Arc::clone(&invocation);
    api.set(
        "storage_delete",
        lua.create_function(
            move |_, (request_id, key, expected_version): (LuaString, LuaString, Option<u64>)| {
                let request_id = bounded_script_id(request_id, "request_id")?;
                let key = bounded_lua_string(
                    key,
                    "storage_key",
                    crate::MAX_PLUGIN_STORAGE_KEY_BYTES,
                    false,
                )?;
                let request =
                    ScriptPluginStorageDeleteRequest::try_new(request_id, key, expected_version)
                        .map_err(dto_error)?;
                push_command(
                    &storage_delete_invocation,
                    ScriptCommand::PluginStorageDelete { request },
                )
            },
        )?,
    )?;
    let open_menu_invocation = Arc::clone(&invocation);
    let open_client_screen_invocation = Arc::clone(&invocation);
    api.set(
        "open_client_screen",
        lua.create_function(move |_, (player_id, screen_id): (u64, LuaString)| {
            let screen_id = bounded_lua_string(
                screen_id,
                "screen_id",
                crate::MAX_SCRIPT_RESOURCE_ID_BYTES,
                false,
            )?;
            crate::validate_script_resource_id(&screen_id)
                .map_err(|_| lua_input_error("screen_id", "invalid"))?;
            push_command(
                &open_client_screen_invocation,
                ScriptCommand::OpenClientScreen {
                    player_id: ScriptPlayerId::new(player_id),
                    screen_id,
                },
            )
        })?,
    )?;
    let place_loader_block_invocation = Arc::clone(&invocation);
    api.set(
        "place_loader_block",
        lua.create_function(move |_, (block_id, x, y, z): (LuaString, i32, i32, i32)| {
            let block_id = bounded_lua_string(
                block_id,
                "block_id",
                crate::MAX_SCRIPT_RESOURCE_ID_BYTES,
                false,
            )?;
            crate::validate_script_resource_id(&block_id)
                .map_err(|_| lua_input_error("block_id", "invalid"))?;
            push_command(
                &place_loader_block_invocation,
                ScriptCommand::PlaceLoaderBlock { block_id, x, y, z },
            )
        })?,
    )?;
    let grant_loader_block_item_invocation = Arc::clone(&invocation);
    api.set(
        "grant_loader_block_item",
        lua.create_function(
            move |_, (player_id, block_id, count): (u64, LuaString, i64)| {
                let block_id = bounded_lua_string(
                    block_id,
                    "block_id",
                    crate::MAX_SCRIPT_RESOURCE_ID_BYTES,
                    false,
                )?;
                crate::validate_script_resource_id(&block_id)
                    .map_err(|_| lua_input_error("block_id", "invalid"))?;
                let count = u8::try_from(count)
                    .ok()
                    .filter(|count| (1..=64).contains(count))
                    .ok_or_else(|| lua_input_error("count", "must be in 1..=64"))?;
                push_command(
                    &grant_loader_block_item_invocation,
                    ScriptCommand::GrantLoaderBlockItem {
                        player_id: ScriptPlayerId::new(player_id),
                        block_id,
                        count,
                    },
                )
            },
        )?,
    )?;
    api.set(
        "open_inventory_menu",
        lua.create_function(
            move |_, (player_id, menu_id, title, slots): (u64, LuaString, LuaString, Table)| {
                let menu = parse_inventory_menu(menu_id, title, slots)?;
                push_command(
                    &open_menu_invocation,
                    ScriptCommand::OpenInventoryMenu {
                        player_id: ScriptPlayerId::new(player_id),
                        menu,
                    },
                )
            },
        )?,
    )?;
    let close_menu_invocation = Arc::clone(&invocation);
    api.set(
        "close_inventory_menu",
        lua.create_function(move |_, (player_id, menu_id): (u64, LuaString)| {
            let menu_id = bounded_script_id(menu_id, "menu_id")?;
            push_command(
                &close_menu_invocation,
                ScriptCommand::CloseInventoryMenu {
                    player_id: ScriptPlayerId::new(player_id),
                    menu_id,
                },
            )
        })?,
    )?;
    let teleport_player_invocation = Arc::clone(&invocation);
    api.set(
        "teleport_player",
        lua.create_function(
            move |_, (request_id, player_id, x, y, z): (LuaString, u64, f64, f64, f64)| {
                let request_id = bounded_script_id(request_id, "request_id")?;
                let position = ScriptPosition::try_new(x, y, z)
                    .ok_or_else(|| lua_input_error("teleport_position", "invalid"))?;
                let request = ScriptPlayerTeleportRequest::try_new(
                    request_id,
                    ScriptPlayerId::new(player_id),
                    position,
                )
                .map_err(dto_error)?;
                push_command(
                    &teleport_player_invocation,
                    ScriptCommand::TeleportPlayer { request },
                )
            },
        )?,
    )?;
    let list_online_players_invocation = Arc::clone(&invocation);
    api.set(
        "list_online_players",
        lua.create_function(move |_, (request_id, limit): (LuaString, Option<usize>)| {
            let request_id = bounded_script_id(request_id, "request_id")?;
            let request = ScriptOnlinePlayersRequest::try_new(
                request_id,
                limit.unwrap_or(MAX_ONLINE_PLAYER_QUERY_LIMIT),
            )
            .map_err(dto_error)?;
            push_command(
                &list_online_players_invocation,
                ScriptCommand::ListOnlinePlayers { request },
            )
        })?,
    )?;
    let transaction_invocation = Arc::clone(&invocation);
    api.set(
        "inventory_storage_transaction",
        lua.create_function(
            move |_,
                  (player_id, transaction_id, inventory, storage): (
                u64,
                LuaString,
                Table,
                Table,
            )| {
                let transaction_id = bounded_script_id(transaction_id, "transaction_id")?;
                let transaction = ScriptInventoryStorageTransaction::try_new(
                    &transaction_id,
                    ScriptPlayerId::new(player_id),
                    parse_inventory_deltas(inventory)?,
                    parse_storage_mutations(storage)?,
                )
                .map_err(dto_error)?;
                push_command(
                    &transaction_invocation,
                    ScriptCommand::InventoryStorageTransaction { transaction },
                )
            },
        )?,
    )?;
    let player_inventory_invocation = Arc::clone(&invocation);
    api.set(
        "inventory_transaction",
        lua.create_function(
            move |_, (player_id, request_id, inventory): (u64, LuaString, Table)| {
                let request_id = bounded_script_id(request_id, "request_id")?;
                let transaction = ScriptPlayerInventoryTransaction::try_new(
                    request_id,
                    ScriptPlayerId::new(player_id),
                    parse_inventory_deltas(inventory)?,
                )
                .map_err(dto_error)?;
                push_command(
                    &player_inventory_invocation,
                    ScriptCommand::PlayerInventoryTransaction { transaction },
                )
            },
        )?,
    )?;
    let upsert_zone_invocation = Arc::clone(&invocation);
    api.set(
        "upsert_zone",
        lua.create_function(
            move |_,
                  (zone_id, dimension, min_x, min_y, min_z, max_x, max_y, max_z): (
                LuaString,
                LuaString,
                f64,
                f64,
                f64,
                f64,
                f64,
                f64,
            )| {
                let zone_id = bounded_script_id(zone_id, "zone_id")?;
                let dimension = bounded_lua_string(
                    dimension,
                    "dimension",
                    crate::MAX_SCRIPT_RESOURCE_ID_BYTES,
                    false,
                )?;
                let minimum = ScriptPosition::try_new(min_x, min_y, min_z)
                    .ok_or_else(|| lua_input_error("zone_minimum", "invalid"))?;
                let maximum = ScriptPosition::try_new(max_x, max_y, max_z)
                    .ok_or_else(|| lua_input_error("zone_maximum", "invalid"))?;
                let zone = ScriptAxisAlignedZone::try_new(&zone_id, &dimension, minimum, maximum)
                    .map_err(dto_error)?;
                push_command(&upsert_zone_invocation, ScriptCommand::UpsertZone { zone })
            },
        )?,
    )?;
    let upsert_protected_zone_invocation = Arc::clone(&invocation);
    api.set(
        "upsert_protected_zone",
        lua.create_function(
            move |_,
                  (
                zone_id,
                dimension,
                allowed_actor_uuid,
                min_x,
                min_y,
                min_z,
                max_x,
                max_y,
                max_z,
            ): (
                LuaString,
                LuaString,
                LuaString,
                f64,
                f64,
                f64,
                f64,
                f64,
                f64,
            )| {
                let zone_id = bounded_script_id(zone_id, "zone_id")?;
                let dimension = bounded_lua_string(
                    dimension,
                    "dimension",
                    crate::MAX_SCRIPT_RESOURCE_ID_BYTES,
                    false,
                )?;
                let allowed_actor_uuid = bounded_lua_string(
                    allowed_actor_uuid,
                    "allowed_actor_uuid",
                    crate::MAX_SCRIPT_PLAYER_UUID_BYTES,
                    false,
                )?;
                let minimum = ScriptPosition::try_new(min_x, min_y, min_z)
                    .ok_or_else(|| lua_input_error("zone_minimum", "invalid"))?;
                let maximum = ScriptPosition::try_new(max_x, max_y, max_z)
                    .ok_or_else(|| lua_input_error("zone_maximum", "invalid"))?;
                let protection = ScriptZoneProtection::try_actor_or_operator(allowed_actor_uuid)
                    .map_err(dto_error)?;
                let zone = ScriptAxisAlignedZone::try_new_with_protection(
                    zone_id,
                    dimension,
                    minimum,
                    maximum,
                    Some(protection),
                )
                .map_err(dto_error)?;
                push_command(
                    &upsert_protected_zone_invocation,
                    ScriptCommand::UpsertZone { zone },
                )
            },
        )?,
    )?;
    let remove_zone_invocation = Arc::clone(&invocation);
    api.set(
        "remove_zone",
        lua.create_function(move |_, zone_id: LuaString| {
            let zone_id = bounded_script_id(zone_id, "zone_id")?;
            push_command(
                &remove_zone_invocation,
                ScriptCommand::RemoveZone { zone_id },
            )
        })?,
    )?;
    let bind_villager_invocation = Arc::clone(&invocation);
    api.set(
        "bind_nearest_villager",
        lua.create_function(
            move |_, (request_id, x, y, z, radius): (LuaString, f64, f64, f64, f64)| {
                let request_id = bounded_script_id(request_id, "request_id")?;
                let center = ScriptPosition::try_new(x, y, z)
                    .ok_or_else(|| lua_input_error("villager_center", "invalid"))?;
                let request = ScriptVillagerBindingRequest::try_new(request_id, center, radius)
                    .map_err(dto_error)?;
                push_command(
                    &bind_villager_invocation,
                    ScriptCommand::RequestVillagerBinding { request },
                )
            },
        )?,
    )?;
    let set_villager_idle_invocation = Arc::clone(&invocation);
    api.set(
        "set_villager_idle",
        lua.create_function(
            move |_, (request_id, binding_token): (LuaString, LuaString)| {
                let request_id = bounded_script_id(request_id, "request_id")?;
                let binding_token = bounded_script_id(binding_token, "binding_token")?;
                let request = ScriptVillagerGoalRequest::try_new(
                    request_id,
                    binding_token,
                    ScriptVillagerGoal::idle(),
                )
                .map_err(dto_error)?;
                push_command(
                    &set_villager_idle_invocation,
                    ScriptCommand::SetVillagerGoal { request },
                )
            },
        )?,
    )?;
    let move_villager_invocation = Arc::clone(&invocation);
    api.set(
        "move_villager_to",
        lua.create_function(
            move |_,
                  (request_id, binding_token, x, y, z, speed): (
                LuaString,
                LuaString,
                f64,
                f64,
                f64,
                f64,
            )| {
                let request_id = bounded_script_id(request_id, "request_id")?;
                let binding_token = bounded_script_id(binding_token, "binding_token")?;
                let target = ScriptPosition::try_new(x, y, z)
                    .ok_or_else(|| lua_input_error("villager_target", "invalid"))?;
                let goal = ScriptVillagerGoal::follow_position(target, speed).map_err(dto_error)?;
                let request = ScriptVillagerGoalRequest::try_new(request_id, binding_token, goal)
                    .map_err(dto_error)?;
                push_command(
                    &move_villager_invocation,
                    ScriptCommand::SetVillagerGoal { request },
                )
            },
        )?,
    )?;
    lua.globals().set("solaris", api)
}

fn config_table_to_lua(lua: &Lua, config: &toml::Table) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    for (key, value) in config {
        table.set(key.as_str(), config_value_to_lua(lua, value)?)?;
    }
    Ok(table)
}

fn config_value_to_lua(lua: &Lua, value: &toml::Value) -> mlua::Result<Value> {
    match value {
        toml::Value::String(value) => Ok(Value::String(lua.create_string(value)?)),
        toml::Value::Integer(value) => Ok(Value::Integer(*value)),
        toml::Value::Float(value) => Ok(Value::Number(*value)),
        toml::Value::Boolean(value) => Ok(Value::Boolean(*value)),
        toml::Value::Array(values) => {
            let table = lua.create_table()?;
            for (index, value) in values.iter().enumerate() {
                table.set(index + 1, config_value_to_lua(lua, value)?)?;
            }
            Ok(Value::Table(table))
        }
        toml::Value::Table(values) => Ok(Value::Table(config_table_to_lua(lua, values)?)),
        toml::Value::Datetime(_) => Err(mlua::Error::runtime(
            "plugin config datetime values are unsupported",
        )),
    }
}

fn lua_input_error(field: &'static str, code: &'static str) -> mlua::Error {
    mlua::Error::runtime(format!("solaris_input:{field}:{code}"))
}

fn dto_error(error: ScriptDtoError) -> mlua::Error {
    let (field, code) = match error {
        ScriptDtoError::EmptyValue { field } => (field, "empty"),
        ScriptDtoError::ValueTooLong { field, .. } => (field, "too_long"),
        ScriptDtoError::InvalidId { .. } => ("id", "invalid"),
        ScriptDtoError::InvalidResourceId { .. } => ("resource_id", "invalid"),
        ScriptDtoError::InvalidAmount => ("amount", "invalid"),
        ScriptDtoError::InvalidBounds => ("bounds", "invalid"),
        ScriptDtoError::TooManyEntries { field, .. } => (field, "too_many"),
        ScriptDtoError::DuplicateId { .. } => ("id", "duplicate"),
        ScriptDtoError::EmptyTransaction => ("transaction", "empty"),
        ScriptDtoError::InconsistentResult { field } => (field, "inconsistent"),
    };
    lua_input_error(field, code)
}

fn bounded_lua_string(
    value: LuaString,
    field: &'static str,
    max: usize,
    allow_empty: bool,
) -> mlua::Result<String> {
    let bytes = value.as_bytes();
    if bytes.len() > max {
        return Err(lua_input_error(field, "too_long"));
    }
    if !allow_empty && bytes.is_empty() {
        return Err(lua_input_error(field, "empty"));
    }
    let value = std::str::from_utf8(&bytes).map_err(|_| lua_input_error(field, "utf8"))?;
    Ok(value.to_owned())
}

fn bounded_script_id(value: LuaString, field: &'static str) -> mlua::Result<String> {
    let value = bounded_lua_string(value, field, crate::MAX_SCRIPT_ID_BYTES, false)?;
    crate::validate_script_id(&value).map_err(dto_error)
}

fn parse_inventory_menu(
    menu_id: LuaString,
    title: LuaString,
    slots: Table,
) -> mlua::Result<ScriptInventoryMenu> {
    let menu_id = bounded_script_id(menu_id, "menu_id")?;
    let title = bounded_lua_string(
        title,
        "menu_title",
        crate::MAX_INVENTORY_MENU_TITLE_BYTES,
        false,
    )?;
    let len = validate_sequence_shape(&slots, crate::MAX_INVENTORY_MENU_SLOTS, "menu_slots")?;
    let mut parsed = Vec::with_capacity(len);
    for index in 1..=len {
        let slot = raw_table_entry(&slots, index, "menu_slot")?;
        validate_record_shape(&slot, &["slot", "resource", "count", "label"], "menu_slot")?;
        let index = raw_u8_field(&slot, "slot", "menu_slot_index")?;
        let resource = raw_bounded_string_field(
            &slot,
            "resource",
            "menu_resource",
            crate::MAX_SCRIPT_RESOURCE_ID_BYTES,
            false,
        )?;
        let count = raw_u8_field(&slot, "count", "menu_count")?;
        let label = raw_optional_bounded_string_field(
            &slot,
            "label",
            "menu_label",
            crate::MAX_INVENTORY_MENU_TITLE_BYTES,
            true,
        )?;
        let item = ScriptInventoryMenuItem::try_new(&resource, count, label).map_err(dto_error)?;
        parsed.push(ScriptInventoryMenuSlot::new(index, item));
    }
    ScriptInventoryMenu::try_new(&menu_id, title, parsed).map_err(dto_error)
}

fn parse_inventory_deltas(table: Table) -> mlua::Result<Vec<ScriptInventoryResourceDelta>> {
    let len = validate_sequence_shape(
        &table,
        crate::MAX_INVENTORY_STORAGE_MUTATIONS,
        "inventory_deltas",
    )?;
    let mut deltas = Vec::with_capacity(len);
    for index in 1..=len {
        let delta = raw_table_entry(&table, index, "inventory_delta")?;
        validate_record_shape(&delta, &["resource", "delta"], "inventory_delta")?;
        let resource = raw_bounded_string_field(
            &delta,
            "resource",
            "inventory_resource",
            crate::MAX_SCRIPT_RESOURCE_ID_BYTES,
            false,
        )?;
        let amount = raw_i16_field(&delta, "delta", "inventory_delta")?;
        deltas.push(ScriptInventoryResourceDelta::try_new(&resource, amount).map_err(dto_error)?);
    }
    Ok(deltas)
}

fn parse_storage_mutations(table: Table) -> mlua::Result<Vec<ScriptStorageMutation>> {
    let len = validate_sequence_shape(
        &table,
        crate::MAX_INVENTORY_STORAGE_MUTATIONS,
        "storage_mutations",
    )?;
    let mut mutations = Vec::with_capacity(len);
    for index in 1..=len {
        let mutation = raw_table_entry(&table, index, "storage_mutation")?;
        validate_record_shape(
            &mutation,
            &["operation", "key", "expected_version", "value"],
            "storage_mutation",
        )?;
        let operation =
            raw_bounded_string_field(&mutation, "operation", "storage_operation", 6, false)?;
        let key = raw_bounded_string_field(
            &mutation,
            "key",
            "storage_key",
            crate::MAX_PLUGIN_STORAGE_KEY_BYTES,
            false,
        )?;
        let expected_version =
            raw_optional_u64_field(&mutation, "expected_version", "storage_expected_version")?;
        let mutation = match operation.as_str() {
            "cas" => ScriptStorageMutation::compare_and_swap(
                &key,
                expected_version,
                raw_bounded_string_field(
                    &mutation,
                    "value",
                    "storage_value",
                    crate::MAX_PLUGIN_STORAGE_VALUE_BYTES,
                    false,
                )?,
            )
            .map_err(dto_error)?,
            "delete" => ScriptStorageMutation::delete(&key, expected_version).map_err(dto_error)?,
            _ => return Err(lua_input_error("storage_operation", "invalid")),
        };
        mutations.push(mutation);
    }
    Ok(mutations)
}

fn validate_sequence_shape(table: &Table, max: usize, field: &'static str) -> mlua::Result<usize> {
    let raw_len = table.raw_len();
    if raw_len > max {
        return Err(lua_input_error(field, "too_many"));
    }
    let mut count = 0_usize;
    for pair in table.clone().pairs::<Value, Value>() {
        let (key, _) = pair?;
        count = count
            .checked_add(1)
            .ok_or_else(|| lua_input_error(field, "too_many"))?;
        if count > max {
            return Err(lua_input_error(field, "too_many"));
        }
        let Value::Integer(key) = key else {
            return Err(lua_input_error(field, "shape"));
        };
        let Ok(key) = usize::try_from(key) else {
            return Err(lua_input_error(field, "shape"));
        };
        if key == 0 || key > raw_len {
            return Err(lua_input_error(field, "shape"));
        }
    }
    if count != raw_len {
        return Err(lua_input_error(field, "shape"));
    }
    Ok(raw_len)
}

fn validate_record_shape(
    table: &Table,
    allowed_fields: &[&'static str],
    field: &'static str,
) -> mlua::Result<()> {
    let mut count = 0_usize;
    for pair in table.clone().pairs::<Value, Value>() {
        let (key, _) = pair?;
        count = count
            .checked_add(1)
            .ok_or_else(|| lua_input_error(field, "too_many_fields"))?;
        if count > allowed_fields.len() {
            return Err(lua_input_error(field, "too_many_fields"));
        }
        let Value::String(key) = key else {
            return Err(lua_input_error(field, "shape"));
        };
        let key = key.as_bytes();
        if !allowed_fields
            .iter()
            .any(|allowed| key.as_ref() == allowed.as_bytes())
        {
            return Err(lua_input_error(field, "unknown_field"));
        }
    }
    Ok(())
}

fn raw_table_entry(table: &Table, index: usize, field: &'static str) -> mlua::Result<Table> {
    match table.raw_get::<Value>(index)? {
        Value::Table(value) => Ok(value),
        _ => Err(lua_input_error(field, "type")),
    }
}

fn raw_bounded_string_field(
    table: &Table,
    key: &'static str,
    field: &'static str,
    max: usize,
    allow_empty: bool,
) -> mlua::Result<String> {
    match table.raw_get::<Value>(key)? {
        Value::String(value) => bounded_lua_string(value, field, max, allow_empty),
        _ => Err(lua_input_error(field, "type")),
    }
}

fn raw_optional_bounded_string_field(
    table: &Table,
    key: &'static str,
    field: &'static str,
    max: usize,
    allow_empty: bool,
) -> mlua::Result<Option<String>> {
    match table.raw_get::<Value>(key)? {
        Value::Nil => Ok(None),
        Value::String(value) => bounded_lua_string(value, field, max, allow_empty).map(Some),
        _ => Err(lua_input_error(field, "type")),
    }
}

fn raw_u8_field(table: &Table, key: &'static str, field: &'static str) -> mlua::Result<u8> {
    match table.raw_get::<Value>(key)? {
        Value::Integer(value) => u8::try_from(value).map_err(|_| lua_input_error(field, "range")),
        _ => Err(lua_input_error(field, "type")),
    }
}

fn raw_i16_field(table: &Table, key: &'static str, field: &'static str) -> mlua::Result<i16> {
    match table.raw_get::<Value>(key)? {
        Value::Integer(value) => i16::try_from(value).map_err(|_| lua_input_error(field, "range")),
        _ => Err(lua_input_error(field, "type")),
    }
}

fn raw_optional_u64_field(
    table: &Table,
    key: &'static str,
    field: &'static str,
) -> mlua::Result<Option<u64>> {
    match table.raw_get::<Value>(key)? {
        Value::Nil => Ok(None),
        Value::Integer(value) => u64::try_from(value)
            .map(Some)
            .map_err(|_| lua_input_error(field, "range")),
        _ => Err(lua_input_error(field, "type")),
    }
}

fn push_command(
    invocation: &Arc<Mutex<Option<InvocationState>>>,
    command: ScriptCommand,
) -> mlua::Result<()> {
    let mut invocation = lock_invocation(invocation)?;
    let invocation = invocation
        .as_mut()
        .ok_or_else(|| mlua::Error::runtime("Solaris API called outside an event handler"))?;
    invocation
        .batch
        .try_push_authorized(command, &invocation.capabilities)
        .map_err(command_error)
}

fn command_error(error: CommandBatchError) -> mlua::Error {
    match error {
        CommandBatchError::Full { limit } => {
            mlua::Error::runtime(format!("command limit {} exceeded", limit.get()))
        }
        CommandBatchError::PermissionDenied { capability } => {
            mlua::Error::runtime(format!("command capability denied: {}", capability.code()))
        }
        CommandBatchError::ProvenanceRejected => {
            mlua::Error::runtime("host-attached command rejected")
        }
        CommandBatchError::InvalidCommand { error } => {
            mlua::Error::runtime(format!("invalid command: {error:?}"))
        }
        CommandBatchError::AdmissionUnavailable => {
            mlua::Error::runtime("host command admission unavailable")
        }
    }
}

fn lock_invocation(
    invocation: &Arc<Mutex<Option<InvocationState>>>,
) -> mlua::Result<std::sync::MutexGuard<'_, Option<InvocationState>>> {
    match invocation.lock() {
        Ok(invocation) => Ok(invocation),
        Err(mut poisoned) => {
            **poisoned.get_mut() = None;
            Err(mlua::Error::runtime("Lua invocation authority poisoned"))
        }
    }
}

fn run_with_instruction_budget<T>(
    lua: &Lua,
    budget: NonZeroU64,
    run: impl FnOnce() -> mlua::Result<T>,
) -> mlua::Result<T> {
    install_instruction_budget_hook(lua, budget)?;
    let result = run();
    lua.remove_interrupt();
    result
}

fn install_instruction_budget_hook(lua: &Lua, budget: NonZeroU64) -> mlua::Result<()> {
    let consumed = Arc::new(AtomicU64::new(0));
    let interrupt_consumed = Arc::clone(&consumed);
    lua.set_interrupt(move |_| {
        let total = interrupt_consumed.fetch_add(LUAU_INTERRUPT_FUEL_COST, Ordering::Relaxed)
            + LUAU_INTERRUPT_FUEL_COST;
        if total >= budget.get() {
            return Err(mlua::Error::runtime("instruction budget exceeded"));
        }
        Ok(VmState::Continue)
    });
    Ok(())
}

fn timer_event_table(
    lua: &Lua,
    timer_id: &str,
    scheduled_tick: u64,
    fired_tick: u64,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("name", "plugin.timer")?;
    table.set("timer_id", timer_id)?;
    table.set("scheduled_tick", scheduled_tick)?;
    table.set("fired_tick", fired_tick)?;
    Ok(table)
}

fn event_table(lua: &Lua, event: &ScriptEvent) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("name", event.event_name())?;
    match event.kind() {
        ScriptEventKind::ServerStarted => {}
        ScriptEventKind::ServerStopping { reason } => table.set("reason", reason.as_str())?,
        ScriptEventKind::PlayerJoined {
            player_id,
            username,
            context,
        } => {
            table.set("player_id", player_id.value())?;
            table.set("username", username.as_str())?;
            set_player_context(&table, context)?;
        }
        ScriptEventKind::PlayerLeft { player_id, reason } => {
            table.set("player_id", player_id.value())?;
            table.set("reason", reason.as_str())?;
        }
        ScriptEventKind::PlayerChat {
            player_id,
            message,
            context,
        } => {
            table.set("player_id", player_id.value())?;
            table.set("message", message.as_str())?;
            set_player_context(&table, context)?;
        }
        ScriptEventKind::PlayerBlockBroken {
            player_id,
            context,
            dimension,
            block_id,
            x,
            y,
            z,
            game_mode,
        }
        | ScriptEventKind::PlayerBlockPlaced {
            player_id,
            context,
            dimension,
            block_id,
            x,
            y,
            z,
            game_mode,
        } => {
            table.set("player_id", player_id.value())?;
            set_player_context(&table, context)?;
            table.set("player_x", context.x())?;
            table.set("player_y", context.y())?;
            table.set("player_z", context.z())?;
            table.set("dimension", dimension.as_str())?;
            table.set("block_id", block_id.as_str())?;
            table.set("x", *x)?;
            table.set("y", *y)?;
            table.set("z", *z)?;
            table.set("game_mode", game_mode.as_str())?;
        }
        ScriptEventKind::PlayerItemCrafted {
            player_id,
            context,
            dimension,
            item_id,
            count,
            craft_count,
            source,
            game_mode,
        } => {
            table.set("player_id", player_id.value())?;
            set_player_context(&table, context)?;
            table.set("dimension", dimension.as_str())?;
            table.set("item_id", item_id.as_str())?;
            table.set("count", *count)?;
            table.set("craft_count", *craft_count)?;
            table.set("source", source.as_str())?;
            table.set("game_mode", game_mode.as_str())?;
        }
        ScriptEventKind::PlayerItemPickedUp {
            player_id,
            context,
            dimension,
            item_id,
            count,
            source,
            game_mode,
        } => {
            table.set("player_id", player_id.value())?;
            set_player_context(&table, context)?;
            table.set("dimension", dimension.as_str())?;
            table.set("item_id", item_id.as_str())?;
            table.set("count", *count)?;
            table.set("source", source.as_str())?;
            table.set("game_mode", game_mode.as_str())?;
        }
        ScriptEventKind::PlayerEntityKilled {
            player_id,
            context,
            dimension,
            entity_id,
            entity_type,
            source,
            game_mode,
        } => {
            table.set("player_id", player_id.value())?;
            set_player_context(&table, context)?;
            table.set("dimension", dimension.as_str())?;
            table.set("entity_id", entity_id.value())?;
            table.set("entity_type", entity_type.as_str())?;
            table.set("source", source.as_str())?;
            table.set("game_mode", game_mode.as_str())?;
        }
        ScriptEventKind::PlayerEntityInteracted {
            player_id,
            context,
            dimension,
            entity_id,
            entity_type,
            hand,
            secondary_action,
            game_mode,
        } => {
            table.set("player_id", player_id.value())?;
            set_player_context(&table, context)?;
            table.set("dimension", dimension.as_str())?;
            table.set("entity_id", entity_id.value())?;
            table.set("entity_type", entity_type.as_str())?;
            table.set("hand", hand.as_str())?;
            table.set("secondary_action", *secondary_action)?;
            table.set("game_mode", game_mode.as_str())?;
        }
        ScriptEventKind::PlayerDied {
            player_id,
            context,
            dimension,
            game_mode,
        } => {
            table.set("player_id", player_id.value())?;
            set_player_context(&table, context)?;
            table.set("dimension", dimension.as_str())?;
            table.set("game_mode", game_mode.as_str())?;
        }
        ScriptEventKind::PlayerCommand {
            player_id,
            username,
            root,
            arguments,
            context,
        } => {
            table.set("player_id", player_id.value())?;
            table.set("username", username.as_str())?;
            table.set("root", root.as_str())?;
            table.set("arguments", arguments.as_str())?;
            set_player_context(&table, context)?;
        }
        ScriptEventKind::ServerTick { tick } => table.set("tick", *tick)?,
        ScriptEventKind::PluginStorageGetResult {
            request_id,
            key,
            value,
            version,
            failure,
        } => {
            table.set("request_id", request_id.as_str())?;
            table.set("key", key.as_str())?;
            table.set("value", value.as_deref())?;
            table.set("version", *version)?;
            table.set("failure", failure.map(|failure| failure.as_str()))?;
        }
        ScriptEventKind::PluginStorageCasResult {
            request_id,
            key,
            applied,
            version,
            failure,
        } => {
            table.set("request_id", request_id.as_str())?;
            table.set("key", key.as_str())?;
            table.set("applied", *applied)?;
            table.set("version", *version)?;
            table.set("failure", failure.map(|failure| failure.as_str()))?;
        }
        ScriptEventKind::PluginStorageDeleteResult {
            request_id,
            key,
            deleted,
            version,
            failure,
        } => {
            table.set("request_id", request_id.as_str())?;
            table.set("key", key.as_str())?;
            table.set("deleted", *deleted)?;
            table.set("version", *version)?;
            table.set("failure", failure.map(|failure| failure.as_str()))?;
        }
        ScriptEventKind::InventoryMenuClicked {
            player_id,
            context,
            menu_id,
            slot,
            click,
        } => {
            table.set("player_id", player_id.value())?;
            table.set("menu_id", menu_id.as_str())?;
            table.set("slot", *slot)?;
            table.set("click", inventory_click_name(*click))?;
            set_player_context(&table, context)?;
        }
        ScriptEventKind::InventoryStorageTransactionResult {
            request_id,
            committed,
        } => {
            table.set("request_id", request_id.as_str())?;
            table.set("committed", *committed)?;
        }
        ScriptEventKind::PlayerInventoryTransactionResult {
            request_id,
            player_id,
            failure,
        } => {
            table.set("request_id", request_id.as_str())?;
            table.set("player_id", player_id.value())?;
            table.set("committed", failure.is_none())?;
            table.set("failure", failure.map(|failure| failure.as_str()))?;
        }
        ScriptEventKind::PlayerZoneEntered {
            player_id,
            context,
            zone_id,
        }
        | ScriptEventKind::PlayerZoneExited {
            player_id,
            context,
            zone_id,
        } => {
            table.set("player_id", player_id.value())?;
            table.set("zone_id", zone_id.as_str())?;
            set_player_context(&table, context)?;
        }
        ScriptEventKind::ZoneCommandResult { zone_id, accepted } => {
            table.set("zone_id", zone_id.as_str())?;
            table.set("accepted", *accepted)?;
        }
        ScriptEventKind::PlayerTeleportResult {
            request_id,
            player_id,
            position,
            failure,
        } => {
            table.set("request_id", request_id.as_str())?;
            table.set("player_id", player_id.value())?;
            table.set("x", position.x())?;
            table.set("y", position.y())?;
            table.set("z", position.z())?;
            table.set("committed", failure.is_none())?;
            table.set("failure", failure.map(|failure| failure.as_str()))?;
        }
        ScriptEventKind::OnlinePlayersResult {
            request_id,
            players,
            truncated,
        } => {
            table.set("request_id", request_id.as_str())?;
            table.set("truncated", *truncated)?;
            let snapshots = lua.create_table_with_capacity(players.len(), 0)?;
            for (index, player) in players.iter().enumerate() {
                let snapshot = lua.create_table()?;
                snapshot.set("player_id", player.player_id().value())?;
                snapshot.set("dimension", player.dimension())?;
                set_player_context(&snapshot, player.context())?;
                snapshots.set(index + 1, snapshot)?;
            }
            table.set("players", snapshots)?;
        }
        ScriptEventKind::VillagerBindingResult {
            request_id,
            binding,
            failure,
        } => {
            table.set("request_id", request_id.as_str())?;
            match binding {
                Some(binding) => {
                    table.set("binding_token", binding.token())?;
                    table.set("binding_expires_at_tick", binding.expires_at_tick())?;
                }
                None => {
                    table.set("binding_token", mlua::Value::Nil)?;
                    table.set("binding_expires_at_tick", mlua::Value::Nil)?;
                }
            }
            table.set("failure", failure.map(|failure| failure.as_str()))?;
        }
        ScriptEventKind::VillagerGoalResult {
            request_id,
            goal,
            failure,
        } => {
            table.set("request_id", request_id.as_str())?;
            table.set("goal", goal.kind())?;
            table.set("accepted", failure.is_none())?;
            table.set("failure", failure.map(|failure| failure.as_str()))?;
            if let Some(target) = goal.target() {
                table.set("x", target.x())?;
                table.set("y", target.y())?;
                table.set("z", target.z())?;
            }
            table.set("speed", goal.speed())?;
        }
        ScriptEventKind::LoaderInteraction {
            player_id,
            interaction_id,
            payload,
        } => {
            table.set("player_id", player_id.value())?;
            table.set("interaction_id", interaction_id.as_str())?;
            table.set("payload", payload.as_str())?;
        }
    }
    Ok(table)
}

fn set_player_context(table: &Table, context: &crate::ScriptPlayerContext) -> mlua::Result<()> {
    table.set("context_verified", true)?;
    table.set("uuid", context.uuid())?;
    table.set("username", context.username())?;
    table.set("operator", context.operator())?;
    table.set("x", context.x())?;
    table.set("y", context.y())?;
    table.set("z", context.z())?;
    Ok(())
}

fn handler_name(event: &ScriptEvent) -> &'static str {
    match event.kind() {
        ScriptEventKind::ServerStarted => "on_server_started",
        ScriptEventKind::ServerStopping { .. } => "on_server_stopping",
        ScriptEventKind::PlayerJoined { .. } => "on_player_joined",
        ScriptEventKind::PlayerLeft { .. } => "on_player_left",
        ScriptEventKind::PlayerChat { .. } => "on_player_chat",
        ScriptEventKind::PlayerBlockBroken { .. } => "on_player_block_broken",
        ScriptEventKind::PlayerBlockPlaced { .. } => "on_player_block_placed",
        ScriptEventKind::PlayerItemCrafted { .. } => "on_player_item_crafted",
        ScriptEventKind::PlayerItemPickedUp { .. } => "on_player_item_picked_up",
        ScriptEventKind::PlayerEntityKilled { .. } => "on_player_entity_killed",
        ScriptEventKind::PlayerEntityInteracted { .. } => "on_player_entity_interacted",
        ScriptEventKind::PlayerDied { .. } => "on_player_died",
        ScriptEventKind::PlayerCommand { .. } => "on_player_command",
        ScriptEventKind::ServerTick { .. } => "on_server_tick",
        ScriptEventKind::PluginStorageGetResult { .. } => "on_plugin_storage_get_result",
        ScriptEventKind::PluginStorageCasResult { .. } => "on_plugin_storage_cas_result",
        ScriptEventKind::PluginStorageDeleteResult { .. } => "on_plugin_storage_delete_result",
        ScriptEventKind::InventoryMenuClicked { .. } => "on_inventory_menu_clicked",
        ScriptEventKind::InventoryStorageTransactionResult { .. } => {
            "on_inventory_storage_transaction_result"
        }
        ScriptEventKind::PlayerInventoryTransactionResult { .. } => {
            "on_player_inventory_transaction_result"
        }
        ScriptEventKind::PlayerZoneEntered { .. } => "on_player_zone_entered",
        ScriptEventKind::PlayerZoneExited { .. } => "on_player_zone_exited",
        ScriptEventKind::ZoneCommandResult { .. } => "on_zone_command_result",
        ScriptEventKind::PlayerTeleportResult { .. } => "on_player_teleport_result",
        ScriptEventKind::OnlinePlayersResult { .. } => "on_player_online_result",
        ScriptEventKind::VillagerBindingResult { .. } => "on_villager_binding_result",
        ScriptEventKind::VillagerGoalResult { .. } => "on_villager_goal_result",
        ScriptEventKind::LoaderInteraction { .. } => "on_loader_interaction",
    }
}

fn inventory_click_name(click: crate::ScriptInventoryClick) -> &'static str {
    match click {
        crate::ScriptInventoryClick::Primary => "primary",
        crate::ScriptInventoryClick::Secondary => "secondary",
        crate::ScriptInventoryClick::ShiftPrimary => "shift_primary",
        crate::ScriptInventoryClick::ShiftSecondary => "shift_secondary",
    }
}

fn lua_error(error: mlua::Error) -> String {
    error.to_string()
}

fn runtime_error(error: mlua::Error) -> RuntimeError {
    RuntimeError::Trap {
        message: error.to_string(),
    }
}

#[cfg(test)]
mod player_inventory_tests;
#[cfg(test)]
mod plugin_config_tests;
#[cfg(test)]
mod timer_tests;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{
        MAX_SCRIPT_RESOURCE_ID_BYTES, PlayerCommandAdmission, RuntimeControls, SCRIPT_API_VERSION,
        ScriptCommand, ScriptCraftingSource, ScriptEvent, ScriptGameMode, ScriptPlayerContext,
        ScriptPlayerId, ScriptPluginManifest,
    };

    static TEST_TEMP_ID: AtomicU64 = AtomicU64::new(0);

    fn manifest(events: &[&str]) -> ValidatedScriptPluginManifest {
        let mut manifest =
            ScriptPluginManifest::new("test-plugin", "Test Plugin", "0.1.0", SCRIPT_API_VERSION);
        for event in events {
            manifest = manifest.subscribe_event(*event);
        }
        manifest.validate().unwrap()
    }

    fn command_manifest(id: &str, root: &str) -> ValidatedScriptPluginManifest {
        ScriptPluginManifest::new(id, id, "0.1.0", SCRIPT_API_VERSION)
            .declare_player_command_root(root)
            .validate()
            .unwrap()
    }

    fn player_context(username: &str) -> ScriptPlayerContext {
        ScriptPlayerContext::new("test-player", username, false, 0.0, 64.0, 0.0)
    }

    #[test]
    fn lua_join_handler_emits_targeted_chat_command() {
        let mut runtime = LuaScriptRuntime::from_source(
            manifest(&["player.joined"]),
            r#"
                function on_player_joined(event)
                    solaris.send_message(event.player_id, "Welcome " .. event.username)
                end
            "#,
            LuaRuntimeLimits::default(),
        )
        .unwrap();
        let controls = RuntimeControls::unrestricted();

        let batch = runtime
            .handle_event(
                &ScriptEvent::player_joined_with_context(
                    ScriptPlayerId::new(7),
                    player_context("Alex"),
                ),
                RuntimeContext::new(&controls, NonZeroUsize::new(8).unwrap()),
            )
            .unwrap();

        assert_eq!(
            batch.commands(),
            &[ScriptCommand::SendChatMessage {
                player_id: ScriptPlayerId::new(7),
                message: "Welcome Alex".to_owned(),
            }]
        );
    }

    #[test]
    fn targeted_loader_interaction_reaches_only_its_owner_handler() {
        let mut runtime = LuaScriptRuntime::from_source(
            manifest(&[]),
            r#"
                function on_loader_interaction(event)
                    solaris.send_message(
                        event.player_id,
                        event.interaction_id .. "=" .. event.payload)
                end
            "#,
            LuaRuntimeLimits::default(),
        )
        .unwrap();
        let controls = RuntimeControls::unrestricted();
        let event = ScriptEvent::loader_interaction(
            "test-plugin",
            ScriptPlayerId::new(7),
            "test-plugin:continue",
            "accepted",
        )
        .unwrap();

        let batch = runtime
            .handle_event(
                &event,
                RuntimeContext::new(&controls, NonZeroUsize::new(8).unwrap()),
            )
            .unwrap();

        assert_eq!(
            batch.commands(),
            &[ScriptCommand::SendChatMessage {
                player_id: ScriptPlayerId::new(7),
                message: "test-plugin:continue=accepted".to_owned(),
            }]
        );
    }

    #[test]
    fn lua_online_player_result_exposes_nested_authoritative_snapshots() {
        let mut runtime = LuaScriptRuntime::from_source(
            manifest(&[]),
            r#"
                function on_player_online_result(event)
                    local player = event.players[1]
                    solaris.broadcast(event.request_id .. ":" .. player.username .. ":" .. player.dimension .. ":" .. tostring(event.truncated))
                end
            "#,
            LuaRuntimeLimits::default(),
        )
        .unwrap();
        let request = ScriptOnlinePlayersRequest::try_new("who", 1).unwrap();
        let player = crate::ScriptOnlinePlayerSnapshot::try_new(
            ScriptPlayerId::new(7),
            player_context("Alex"),
            "minecraft:overworld",
        )
        .unwrap();
        let event = ScriptEvent::online_players_result("test-plugin", &request, vec![player], true)
            .unwrap();
        let controls = RuntimeControls::unrestricted();

        let batch = runtime
            .handle_event(
                &event,
                RuntimeContext::new(&controls, NonZeroUsize::new(1).unwrap()),
            )
            .unwrap();
        assert_eq!(
            batch.commands(),
            &[ScriptCommand::BroadcastChatMessage {
                message: "who:Alex:minecraft:overworld:true".to_owned(),
            }]
        );
    }

    #[test]
    fn lua_player_command_handler_receives_fields_and_uses_bounded_apis() {
        let command_manifest =
            ScriptPluginManifest::new("test-plugin", "Test Plugin", "0.1.0", SCRIPT_API_VERSION)
                .declare_player_command_root("hello")
                .declare_console_command_root("time")
                .validate()
                .unwrap();
        let mut runtime = LuaScriptRuntime::from_source(
            command_manifest,
            r#"
                function on_player_command(event)
                    solaris.send_message(
                        event.player_id,
                        event.username .. ":" .. event.root .. ":" .. event.arguments
                    )
                    solaris.broadcast("command received")
                    solaris.disconnect(event.player_id, "done")
                    solaris.run_console("time set day")
                end
            "#,
            LuaRuntimeLimits::default(),
        )
        .unwrap();
        let controls = RuntimeControls::unrestricted();

        let batch = runtime
            .handle_event(
                &ScriptEvent::try_player_command_with_context(
                    "test-plugin",
                    ScriptPlayerId::new(7),
                    player_context("Alex"),
                    "hello",
                    "one two",
                )
                .unwrap(),
                RuntimeContext::new(&controls, NonZeroUsize::new(4).unwrap()),
            )
            .unwrap();

        assert_eq!(
            batch.commands(),
            &[
                ScriptCommand::SendChatMessage {
                    player_id: ScriptPlayerId::new(7),
                    message: "Alex:hello:one two".to_owned(),
                },
                ScriptCommand::BroadcastChatMessage {
                    message: "command received".to_owned(),
                },
                ScriptCommand::DisconnectPlayer {
                    player_id: ScriptPlayerId::new(7),
                    reason: "done".to_owned(),
                },
                ScriptCommand::RunConsoleCommand {
                    command: "time set day".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn lua_spawn_entity_emits_authorized_bounded_dto() {
        let spawn_manifest =
            ScriptPluginManifest::new("spawn-test", "Spawn Test", "0.1.0", SCRIPT_API_VERSION)
                .declare_player_command_root("pet")
                .declare_spawn_entity_type("minecraft:pig")
                .validate()
                .unwrap();
        let controls = RuntimeControls::unrestricted();
        let event = ScriptEvent::try_player_command_with_context(
            "spawn-test",
            ScriptPlayerId::new(7),
            player_context("Alex"),
            "pet",
            "",
        )
        .unwrap();
        let mut runtime = LuaScriptRuntime::from_source(
            spawn_manifest,
            r#"
                function on_player_command(event)
                    solaris.spawn_entity(event.player_id, "minecraft:pig", 1.25, 64.0, -2.5)
                end
            "#,
            LuaRuntimeLimits::default(),
        )
        .unwrap();

        let batch = runtime
            .handle_event(
                &event,
                RuntimeContext::new(&controls, NonZeroUsize::new(8).unwrap()),
            )
            .unwrap();
        assert_eq!(
            batch.commands(),
            &[ScriptCommand::SpawnEntity {
                actor: ScriptPlayerId::new(7),
                entity_type: "minecraft:pig".to_owned(),
                position: crate::ScriptPosition::try_new(1.25, 64.0, -2.5).unwrap(),
            }]
        );

        for source in [
            r#"solaris.spawn_entity(event.player_id, "minecraft:cow", 1, 64, 1)"#,
            r#"solaris.spawn_entity(event.player_id, "minecraft:Pig", 1, 64, 1)"#,
            r#"solaris.spawn_entity(event.player_id, "minecraft:pig", 0 / 0, 64, 1)"#,
            r#"solaris.spawn_entity(event.player_id, "minecraft:pig", math.huge, 64, 1)"#,
            r#"solaris.spawn_entity(event.player_id, "minecraft:pig", 30000000.1, 64, 1)"#,
            r#"solaris.spawn_entity(event.player_id, "minecraft:pig", 1, 20000000.1, 1)"#,
            r#"solaris.spawn_entity(event.player_id, "minecraft:pig", 1, 64, -30000000.1)"#,
        ] {
            let mut runtime = LuaScriptRuntime::from_source(
                ScriptPluginManifest::new("spawn-test", "Spawn Test", "0.1.0", SCRIPT_API_VERSION)
                    .declare_player_command_root("pet")
                    .declare_spawn_entity_type("minecraft:pig")
                    .validate()
                    .unwrap(),
                &format!("function on_player_command(event) {source} end"),
                LuaRuntimeLimits::default(),
            )
            .unwrap();
            assert!(
                runtime
                    .handle_event(
                        &event,
                        RuntimeContext::new(&controls, NonZeroUsize::new(8).unwrap()),
                    )
                    .is_err()
            );
        }

        let oversized_type = format!("minecraft:{}", "a".repeat(MAX_SCRIPT_RESOURCE_ID_BYTES));
        let source = format!(
            "function on_player_command(event) solaris.spawn_entity(event.player_id, '{oversized_type}', 1, 64, 1) end"
        );
        let mut runtime = LuaScriptRuntime::from_source(
            ScriptPluginManifest::new("spawn-test", "Spawn Test", "0.1.0", SCRIPT_API_VERSION)
                .declare_player_command_root("pet")
                .declare_spawn_entity_type("minecraft:pig")
                .validate()
                .unwrap(),
            &source,
            LuaRuntimeLimits::default(),
        )
        .unwrap();
        assert!(
            runtime
                .handle_event(
                    &event,
                    RuntimeContext::new(&controls, NonZeroUsize::new(8).unwrap()),
                )
                .is_err()
        );
    }

    #[test]
    fn lua_place_loader_block_emits_bounded_integer_command() {
        let manifest =
            ScriptPluginManifest::new("loader-test", "Loader Test", "0.1.0", SCRIPT_API_VERSION)
                .declare_player_command_root("place")
                .validate()
                .unwrap();
        let controls = RuntimeControls::unrestricted();
        let event = ScriptEvent::try_player_command_with_context(
            "loader-test",
            ScriptPlayerId::new(7),
            player_context("Alex"),
            "place",
            "",
        )
        .unwrap();
        let mut runtime = LuaScriptRuntime::from_source(
            manifest,
            r#"
                function on_player_command(event)
                    solaris.place_loader_block("loader-test:ruby_block", 3, 64, -5)
                end
            "#,
            LuaRuntimeLimits::default(),
        )
        .unwrap();

        let batch = runtime
            .handle_event(
                &event,
                RuntimeContext::new(&controls, NonZeroUsize::new(8).unwrap()),
            )
            .unwrap();
        assert_eq!(
            batch.commands(),
            &[ScriptCommand::PlaceLoaderBlock {
                block_id: "loader-test:ruby_block".to_owned(),
                x: 3,
                y: 64,
                z: -5,
            }]
        );
    }

    #[test]
    fn lua_grant_loader_block_item_emits_bounded_command() {
        let manifest =
            ScriptPluginManifest::new("loader-test", "Loader Test", "0.1.0", SCRIPT_API_VERSION)
                .declare_player_command_root("grant")
                .validate()
                .unwrap();
        let controls = RuntimeControls::unrestricted();
        let event = ScriptEvent::try_player_command_with_context(
            "loader-test",
            ScriptPlayerId::new(7),
            player_context("Alex"),
            "grant",
            "",
        )
        .unwrap();
        let mut runtime = LuaScriptRuntime::from_source(
            manifest,
            r#"
                function on_player_command(event)
                    solaris.grant_loader_block_item(event.player_id, "loader-test:ruby_block", 3)
                end
            "#,
            LuaRuntimeLimits::default(),
        )
        .unwrap();

        let batch = runtime
            .handle_event(
                &event,
                RuntimeContext::new(&controls, NonZeroUsize::new(8).unwrap()),
            )
            .unwrap();
        assert_eq!(
            batch.commands(),
            &[ScriptCommand::GrantLoaderBlockItem {
                player_id: ScriptPlayerId::new(7),
                block_id: "loader-test:ruby_block".to_owned(),
                count: 3,
            }]
        );
    }

    #[test]
    fn lua_player_events_expose_the_exact_context_snapshot_fields() {
        let context_manifest =
            ScriptPluginManifest::new("test-plugin", "Test Plugin", "0.1.0", SCRIPT_API_VERSION)
                .subscribe_event("player.joined")
                .subscribe_event("player.chat")
                .declare_player_command_root("hello")
                .validate()
                .unwrap();
        let mut runtime = LuaScriptRuntime::from_source(
            context_manifest,
            r#"
                local function context(event)
                    return tostring(event.context_verified) .. ":" ..
                        event.uuid .. ":" .. event.username .. ":" ..
                        tostring(event.operator) .. ":" .. event.x .. ":" ..
                        event.y .. ":" .. event.z
                end

                function on_player_joined(event)
                    solaris.send_message(event.player_id, "joined:" .. context(event))
                end

                function on_player_chat(event)
                    solaris.send_message(event.player_id, "chat:" .. context(event))
                end

                function on_player_command(event)
                    solaris.send_message(event.player_id, "command:" .. context(event))
                end
            "#,
            LuaRuntimeLimits::default(),
        )
        .unwrap();
        let controls = RuntimeControls::unrestricted();
        let context = crate::ScriptPlayerContext::new(
            "123e4567-e89b-12d3-a456-426614174000",
            "Alex",
            true,
            1.5,
            64.0,
            -2.25,
        );
        let expected_context = "true:123e4567-e89b-12d3-a456-426614174000:Alex:true:1.5:64:-2.25";

        for (event, expected) in [
            (
                ScriptEvent::player_joined_with_context(ScriptPlayerId::new(7), context.clone()),
                format!("joined:{expected_context}"),
            ),
            (
                ScriptEvent::player_chat_with_context(
                    ScriptPlayerId::new(7),
                    "hello",
                    context.clone(),
                ),
                format!("chat:{expected_context}"),
            ),
            (
                ScriptEvent::try_player_command_with_context(
                    "test-plugin",
                    ScriptPlayerId::new(7),
                    context,
                    "hello",
                    "one two",
                )
                .unwrap(),
                format!("command:{expected_context}"),
            ),
        ] {
            let batch = runtime
                .handle_event(
                    &event,
                    RuntimeContext::new(&controls, NonZeroUsize::new(1).unwrap()),
                )
                .unwrap();
            assert_eq!(
                batch.commands(),
                &[ScriptCommand::SendChatMessage {
                    player_id: ScriptPlayerId::new(7),
                    message: expected,
                }]
            );
        }
    }

    #[test]
    fn lua_block_broken_subscription_dispatches_exact_post_commit_fields() {
        let mut runtime = LuaScriptRuntime::from_source(
            manifest(&["player.block_broken"]),
            r#"
                function on_player_block_broken(event)
                    local expected = {
                        name = true, player_id = true, context_verified = true,
                        uuid = true, username = true, operator = true,
                        player_x = true, player_y = true, player_z = true,
                        dimension = true, block_id = true, x = true, y = true,
                        z = true, game_mode = true,
                    }
                    local field_count = 0
                    for field in pairs(event) do
                        assert(expected[field] == true, "unexpected field: " .. field)
                        field_count = field_count + 1
                    end
                    assert(field_count == 15)
                    assert(event.name == "player.block_broken")
                    assert(event.context_verified == true)
                    assert(event.uuid == "123e4567-e89b-12d3-a456-426614174000")
                    assert(event.username == "Alex")
                    assert(event.operator == true)
                    assert(event.player_x == 1.5)
                    assert(event.player_y == 64.0)
                    assert(event.player_z == -2.25)
                    assert(event.dimension == "minecraft:the_nether")
                    assert(event.block_id == "minecraft:ancient_debris")
                    assert(type(event.x) == "number" and event.x % 1 == 0 and event.x == -3)
                    assert(type(event.y) == "number" and event.y % 1 == 0 and event.y == 15)
                    assert(type(event.z) == "number" and event.z % 1 == 0 and event.z == 27)
                    assert(event.game_mode == "creative")
                    solaris.send_message(event.player_id, "block-broken")
                end
            "#,
            LuaRuntimeLimits::default(),
        )
        .unwrap();
        let controls = RuntimeControls::unrestricted();
        let event = ScriptEvent::try_player_block_broken_with_context(
            ScriptPlayerId::new(7),
            ScriptPlayerContext::new(
                "123e4567-e89b-12d3-a456-426614174000",
                "Alex",
                true,
                1.5,
                64.0,
                -2.25,
            ),
            "minecraft:the_nether",
            "minecraft:ancient_debris",
            -3,
            15,
            27,
            ScriptGameMode::Creative,
        )
        .unwrap();

        let batch = runtime
            .handle_event(
                &event,
                RuntimeContext::new(&controls, NonZeroUsize::new(1).unwrap()),
            )
            .unwrap();

        assert_eq!(
            batch.commands(),
            &[ScriptCommand::SendChatMessage {
                player_id: ScriptPlayerId::new(7),
                message: "block-broken".to_owned(),
            }]
        );
    }

    #[test]
    fn lua_block_placed_subscription_dispatches_exact_fields() {
        let mut runtime = LuaScriptRuntime::from_source(
            manifest(&["player.block_placed"]),
            r#"
                function on_player_block_placed(event)
                    local expected = {
                        name = true, player_id = true, context_verified = true,
                        uuid = true, username = true, operator = true,
                        player_x = true, player_y = true, player_z = true,
                        dimension = true, block_id = true, x = true, y = true,
                        z = true, game_mode = true,
                    }
                    local field_count = 0
                    for field in pairs(event) do
                        assert(expected[field] == true, "unexpected field: " .. field)
                        field_count = field_count + 1
                    end
                    assert(field_count == 15)
                    assert(event.name == "player.block_placed")
                    assert(event.context_verified == true)
                    assert(event.uuid == "123e4567-e89b-12d3-a456-426614174000")
                    assert(event.username == "Alex")
                    assert(event.operator == true)
                    assert(event.player_x == 1.5)
                    assert(event.player_y == 64.0)
                    assert(event.player_z == -2.25)
                    assert(event.dimension == "minecraft:the_end")
                    assert(event.block_id == "minecraft:obsidian")
                    assert(type(event.x) == "number" and event.x % 1 == 0 and event.x == -3)
                    assert(type(event.y) == "number" and event.y % 1 == 0 and event.y == 15)
                    assert(type(event.z) == "number" and event.z % 1 == 0 and event.z == 27)
                    assert(event.game_mode == "creative")
                    solaris.send_message(event.player_id, "block-placed")
                end
            "#,
            LuaRuntimeLimits::default(),
        )
        .unwrap();
        let controls = RuntimeControls::unrestricted();
        let event = ScriptEvent::try_player_block_placed_with_context(
            ScriptPlayerId::new(7),
            ScriptPlayerContext::new(
                "123e4567-e89b-12d3-a456-426614174000",
                "Alex",
                true,
                1.5,
                64.0,
                -2.25,
            ),
            "minecraft:the_end",
            "minecraft:obsidian",
            -3,
            15,
            27,
            ScriptGameMode::Creative,
        )
        .unwrap();

        let batch = runtime
            .handle_event(
                &event,
                RuntimeContext::new(&controls, NonZeroUsize::new(1).unwrap()),
            )
            .unwrap();

        assert_eq!(
            batch.commands(),
            &[ScriptCommand::SendChatMessage {
                player_id: ScriptPlayerId::new(7),
                message: "block-placed".to_owned(),
            }]
        );
    }

    #[test]
    fn lua_item_crafted_subscription_dispatches_exact_fields() {
        let mut runtime = LuaScriptRuntime::from_source(
            manifest(&["player.item_crafted"]),
            r#"
                function on_player_item_crafted(event)
                    local expected = {
                        name = true, player_id = true, context_verified = true,
                        uuid = true, username = true, operator = true,
                        x = true, y = true, z = true, dimension = true,
                        item_id = true, count = true, craft_count = true,
                        source = true, game_mode = true,
                    }
                    local field_count = 0
                    for field in pairs(event) do
                        assert(expected[field] == true, "unexpected field: " .. field)
                        field_count = field_count + 1
                    end
                    assert(field_count == 15)
                    assert(event.name == "player.item_crafted")
                    assert(event.player_id == 7)
                    assert(event.context_verified == true)
                    assert(event.uuid == "123e4567-e89b-12d3-a456-426614174000")
                    assert(event.username == "Alex")
                    assert(event.operator == true)
                    assert(event.x == 1.5)
                    assert(event.y == 64.0)
                    assert(event.z == -2.25)
                    assert(event.dimension == "minecraft:overworld")
                    assert(event.item_id == "minecraft:oak_planks")
                    assert(type(event.count) == "number" and event.count % 1 == 0 and event.count == 12)
                    assert(type(event.craft_count) == "number" and event.craft_count % 1 == 0 and event.craft_count == 3)
                    assert(event.source == "crafting_table")
                    assert(event.game_mode == "adventure")
                    solaris.send_message(event.player_id, "item-crafted")
                end
            "#,
            LuaRuntimeLimits::default(),
        )
        .unwrap();
        let controls = RuntimeControls::unrestricted();
        let event = ScriptEvent::try_player_item_crafted_with_context(
            ScriptPlayerId::new(7),
            ScriptPlayerContext::new(
                "123e4567-e89b-12d3-a456-426614174000",
                "Alex",
                true,
                1.5,
                64.0,
                -2.25,
            ),
            "minecraft:overworld",
            "minecraft:oak_planks",
            12,
            3,
            ScriptCraftingSource::CraftingTable,
            ScriptGameMode::Adventure,
        )
        .unwrap();

        let batch = runtime
            .handle_event(
                &event,
                RuntimeContext::new(&controls, NonZeroUsize::new(1).unwrap()),
            )
            .unwrap();

        assert_eq!(
            batch.commands(),
            &[ScriptCommand::SendChatMessage {
                player_id: ScriptPlayerId::new(7),
                message: "item-crafted".to_owned(),
            }]
        );
    }

    #[test]
    fn lua_infinite_handler_is_stopped_by_instruction_budget() {
        let mut runtime = LuaScriptRuntime::from_source(
            manifest(&["server.tick"]),
            r#"
                function on_server_tick(_event)
                    while true do end
                end
            "#,
            LuaRuntimeLimits {
                instructions_per_event: NonZeroU64::new(10_000).unwrap(),
                ..LuaRuntimeLimits::default()
            },
        )
        .unwrap();
        let controls = RuntimeControls::unrestricted();

        let error = runtime
            .handle_event(
                &ScriptEvent::server_tick(1),
                RuntimeContext::new(&controls, NonZeroUsize::new(8).unwrap()),
            )
            .unwrap_err();

        assert!(
            matches!(error, crate::RuntimeError::Trap { message } if message.contains("instruction budget"))
        );
    }

    #[test]
    fn lua_memory_exhaustion_fails_the_invocation_without_returning_its_partial_batch() {
        let mut runtime = LuaScriptRuntime::from_source(
            manifest(&["server.tick"]),
            r#"
                function on_server_tick(_event)
                    solaris.broadcast("must-not-publish")
                    local allocation = string.rep("x", 32 * 1024 * 1024)
                    solaris.broadcast(allocation)
                end
            "#,
            LuaRuntimeLimits::default(),
        )
        .unwrap();
        let controls = RuntimeControls::unrestricted();

        let error = runtime
            .handle_event(
                &ScriptEvent::server_tick(1),
                RuntimeContext::new(&controls, NonZeroUsize::new(2).unwrap()),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            RuntimeError::Trap { message } if message.to_ascii_lowercase().contains("memory")
        ));
        assert!(lock_invocation(&runtime.invocation).unwrap().is_none());
    }

    #[test]
    fn lua_batch_rejection_callback_reports_closed_queue_exactly() {
        let runtime = LuaScriptRuntime::from_source(
            manifest(&[]),
            r#"
                rejection = "missing"
                function on_command_batch_rejected(result)
                    rejection = result.reason .. ":" .. result.command_count
                end
            "#,
            LuaRuntimeLimits::default(),
        )
        .unwrap();

        runtime.notify_batch_rejected("queue_closed", 2).unwrap();

        assert_eq!(
            runtime.lua.globals().get::<String>("rejection").unwrap(),
            "queue_closed:2"
        );
    }

    #[tokio::test]
    async fn lua_host_rejects_a_saturated_invocation_atomically_and_reports_the_batch() {
        let source = PluginSource {
            manifest: ScriptPluginManifest::new("atomic", "Atomic", "0.1.0", SCRIPT_API_VERSION)
                .subscribe_event("server.tick")
                .validate()
                .unwrap(),
            config: toml::Table::new(),
            source: r#"
                rejection = "missing"
                function on_command_batch_rejected(result)
                    rejection = result.reason .. ":" .. result.command_count
                end
                function on_server_tick(event)
                    if event.tick == 1 then
                        solaris.broadcast("batch-first")
                        solaris.broadcast("batch-second")
                    else
                        solaris.broadcast(rejection)
                    end
                end
            "#
            .to_owned(),
            source_path: PathBuf::from("atomic/main.lua"),
            worldgen_ore_profile: None,
            worldgen_settlement_plan: None,
            client_bundles: Vec::new(),
        };
        let (boundary, endpoint) =
            script_boundary_pair(NonZeroUsize::new(2).unwrap(), NonZeroUsize::new(1).unwrap());
        endpoint
            .try_submit_command(ScriptCommand::BroadcastChatMessage {
                message: "existing".to_owned(),
            })
            .unwrap();
        let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);
        let (progress_tx, progress_rx) = std::sync::mpsc::sync_channel(2);
        let host = thread::spawn(move || {
            let mut endpoint = endpoint;
            run_lua_host_inner(&mut endpoint, vec![source], startup_tx, Some(progress_tx));
        });
        assert_eq!(startup_rx.recv().unwrap(), 1);

        boundary
            .try_enqueue_event(ScriptEvent::server_tick(1))
            .unwrap();
        assert_eq!(progress_rx.recv().unwrap(), "server.tick");

        let existing = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .expect("preexisting command was not available")
            .expect("script command queue closed");
        assert_eq!(
            existing,
            ScriptCommand::BroadcastChatMessage {
                message: "existing".to_owned(),
            }
        );
        boundary
            .try_enqueue_event(ScriptEvent::server_tick(2))
            .unwrap();
        assert_eq!(progress_rx.recv().unwrap(), "server.tick");
        let report = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .expect("batch rejection callback did not publish its later report")
            .expect("script command queue closed");
        assert!(matches!(
            report,
            ScriptCommand::HostAttached { provenance, request }
                if provenance.plugin_id() == "atomic"
                    && matches!(request.as_ref(), ScriptCommand::BroadcastChatMessage { message } if message == "queue_full:2")
        ));
        assert!(matches!(
            boundary.command_rx.try_lock().unwrap().try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));

        drop(boundary);
        tokio::task::spawn_blocking(move || host.join())
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn shipped_basic_economy_releases_pending_read_after_batch_rejection() {
        let examples =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/basic-economy");
        let source = read_plugin_source(&examples).unwrap();
        let (boundary, endpoint) =
            script_boundary_pair(NonZeroUsize::new(4).unwrap(), NonZeroUsize::new(2).unwrap());
        for message in ["existing-first", "existing-second"] {
            endpoint
                .try_submit_command(ScriptCommand::BroadcastChatMessage {
                    message: message.to_owned(),
                })
                .unwrap();
        }
        let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);
        let (progress_tx, progress_rx) = std::sync::mpsc::sync_channel(2);
        let host = thread::spawn(move || {
            let mut endpoint = endpoint;
            run_lua_host_inner(&mut endpoint, vec![source], startup_tx, Some(progress_tx));
        });
        assert_eq!(startup_rx.recv().unwrap(), 1);

        let player_id = ScriptPlayerId::new(7);
        let context = player_context("Alex");
        assert_eq!(
            boundary
                .try_enqueue_player_command_with_context(player_id, context.clone(), "economy",),
            Ok(PlayerCommandAdmission::Enqueued)
        );
        assert_eq!(progress_rx.recv().unwrap(), "player.command");

        for expected in ["existing-first", "existing-second"] {
            let command = boundary.recv_command().await.unwrap();
            assert!(matches!(
                command,
                ScriptCommand::BroadcastChatMessage { message } if message == expected
            ));
        }

        assert_eq!(
            boundary.try_enqueue_player_command_with_context(player_id, context, "economy"),
            Ok(PlayerCommandAdmission::Enqueued)
        );
        assert_eq!(progress_rx.recv().unwrap(), "player.command");

        let mut saw_rejection_notice = false;
        let mut saw_retry_read = false;
        for _ in 0..2 {
            let command = boundary.recv_command().await.unwrap();
            let ScriptCommand::HostAttached {
                provenance,
                request,
            } = command
            else {
                panic!("economy retry command was not host attached");
            };
            assert_eq!(provenance.plugin_id(), "basic-economy");
            match request.as_ref() {
                ScriptCommand::SendChatMessage {
                    player_id: target,
                    message,
                } => {
                    assert_eq!(*target, player_id);
                    assert_eq!(message, "Economy request rejected: queue_full.");
                    saw_rejection_notice = true;
                }
                ScriptCommand::PluginStorageGet { request } => {
                    assert_eq!(request.request_id(), "command-7");
                    saw_retry_read = true;
                }
                other => panic!("unexpected economy retry command: {other:?}"),
            }
        }
        assert!(saw_rejection_notice);
        assert!(saw_retry_read);

        drop(boundary);
        tokio::task::spawn_blocking(move || host.join())
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn lua_api_rejects_oversized_chat_before_it_reaches_the_command_queue() {
        let mut runtime = LuaScriptRuntime::from_source(
            manifest(&["server.tick"]),
            r#"
                function on_server_tick(_event)
                    solaris.broadcast(string.rep("x", 4097))
                end
            "#,
            LuaRuntimeLimits::default(),
        )
        .unwrap();
        let controls = RuntimeControls::unrestricted();

        let error = runtime
            .handle_event(
                &ScriptEvent::server_tick(1),
                RuntimeContext::new(&controls, NonZeroUsize::new(8).unwrap()),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            crate::RuntimeError::Trap { message }
                if message.contains("solaris_input:chat_message:too_long")
        ));
    }

    #[test]
    fn oversized_nested_lua_string_is_rejected_before_host_owned_dto_allocation() {
        let manifest = ScriptPluginManifest::new(
            "heap-boundary",
            "Heap Boundary",
            "0.1.0",
            SCRIPT_API_VERSION,
        )
        .subscribe_event("server.tick")
        .declare_inventory_menus()
        .validate()
        .unwrap();
        let mut runtime = LuaScriptRuntime::from_source(
            manifest,
            r#"
                function on_server_tick(_event)
                    solaris.open_inventory_menu(7, "catalog", "Catalog", {
                        { slot = 0, resource = string.rep("x", 4097), count = 1 },
                    })
                end
            "#,
            LuaRuntimeLimits {
                memory_bytes: NonZeroUsize::new(512 * 1024).unwrap(),
                ..LuaRuntimeLimits::default()
            },
        )
        .unwrap();

        let error = runtime
            .handle_event(
                &ScriptEvent::server_tick(1),
                RuntimeContext::new(
                    &RuntimeControls::unrestricted(),
                    NonZeroUsize::new(1).unwrap(),
                ),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            RuntimeError::Trap { message }
                if message.contains("solaris_input:menu_resource:too_long")
        ));
    }

    #[test]
    fn every_lua_api_bounds_strings_before_command_construction() {
        let manifest =
            ScriptPluginManifest::new("raw-boundary", "Raw Boundary", "0.1.0", SCRIPT_API_VERSION)
                .subscribe_event("server.tick")
                .declare_console_command_root("say")
                .declare_spawn_entity_type("minecraft:pig")
                .declare_plugin_storage()
                .declare_inventory_menus()
                .declare_inventory_storage_transactions()
                .declare_zones()
                .declare_villagers()
                .validate()
                .unwrap();
        let cases = [
            (
                "send_message",
                "solaris.send_message(7, string.rep('x', 4097))",
                "chat_message:too_long",
            ),
            (
                "broadcast",
                "solaris.broadcast(string.rep('x', 4097))",
                "chat_message:too_long",
            ),
            (
                "broadcast.invalid_utf8",
                "solaris.broadcast(string.char(255))",
                "chat_message:utf8",
            ),
            (
                "disconnect",
                "solaris.disconnect(7, string.rep('x', 1025))",
                "disconnect_reason:too_long",
            ),
            (
                "run_console",
                "solaris.run_console(string.rep('x', 257))",
                "console_command:too_long",
            ),
            (
                "spawn_entity",
                "solaris.spawn_entity(7, string.rep('x', 129), 0, 64, 0)",
                "entity_type:too_long",
            ),
            (
                "storage_get.request_id",
                "solaris.storage_get(string.rep('x', 65), 'coins')",
                "request_id:too_long",
            ),
            (
                "storage_get.key",
                "solaris.storage_get('read', string.rep('x', 129))",
                "storage_key:too_long",
            ),
            (
                "storage_cas.value",
                "solaris.storage_cas('write', 'coins', nil, string.rep('x', 4097))",
                "storage_value:too_long",
            ),
            (
                "storage_delete.key",
                "solaris.storage_delete('delete', string.rep('x', 129), nil)",
                "storage_key:too_long",
            ),
            (
                "open_inventory_menu.id",
                "solaris.open_inventory_menu(7, string.rep('x', 65), 'Catalog', {})",
                "menu_id:too_long",
            ),
            (
                "open_inventory_menu.title",
                "solaris.open_inventory_menu(7, 'catalog', string.rep('x', 129), {})",
                "menu_title:too_long",
            ),
            (
                "open_inventory_menu.label",
                "solaris.open_inventory_menu(7, 'catalog', 'Catalog', {{slot=0, resource='minecraft:apple', count=1, label=string.rep('x', 129)}})",
                "menu_label:too_long",
            ),
            (
                "close_inventory_menu",
                "solaris.close_inventory_menu(7, string.rep('x', 65))",
                "menu_id:too_long",
            ),
            (
                "inventory_storage_transaction.id",
                "solaris.inventory_storage_transaction(7, string.rep('x', 65), {{resource='minecraft:apple', delta=1}}, {{operation='cas', key='coins', value='1'}})",
                "transaction_id:too_long",
            ),
            (
                "inventory_storage_transaction.resource",
                "solaris.inventory_storage_transaction(7, 'tx', {{resource=string.rep('x', 129), delta=1}}, {{operation='cas', key='coins', value='1'}})",
                "inventory_resource:too_long",
            ),
            (
                "inventory_storage_transaction.operation",
                "solaris.inventory_storage_transaction(7, 'tx', {{resource='minecraft:apple', delta=1}}, {{operation='compare', key='coins', value='1'}})",
                "storage_operation:too_long",
            ),
            (
                "inventory_storage_transaction.value",
                "solaris.inventory_storage_transaction(7, 'tx', {{resource='minecraft:apple', delta=1}}, {{operation='cas', key='coins', value=string.rep('x', 4097)}})",
                "storage_value:too_long",
            ),
            (
                "upsert_zone.id",
                "solaris.upsert_zone(string.rep('x', 65), 'minecraft:overworld', 0, 0, 0, 1, 1, 1)",
                "zone_id:too_long",
            ),
            (
                "upsert_zone.dimension",
                "solaris.upsert_zone('shop', string.rep('x', 129), 0, 0, 0, 1, 1, 1)",
                "dimension:too_long",
            ),
            (
                "upsert_protected_zone.actor",
                "solaris.upsert_protected_zone('claim', 'minecraft:overworld', 'not-a-uuid', 0, 0, 0, 1, 1, 1)",
                "id:invalid",
            ),
            (
                "remove_zone",
                "solaris.remove_zone(string.rep('x', 65))",
                "zone_id:too_long",
            ),
            (
                "bind_nearest_villager.request_id",
                "solaris.bind_nearest_villager(string.rep('x', 65), 0, 64, 0, 16)",
                "request_id:too_long",
            ),
            (
                "set_villager_idle.request_id",
                "solaris.set_villager_idle(string.rep('x', 65), 'binding-1')",
                "request_id:too_long",
            ),
            (
                "set_villager_idle.binding",
                "solaris.set_villager_idle('idle', string.rep('x', 65))",
                "binding_token:too_long",
            ),
            (
                "move_villager_to.binding",
                "solaris.move_villager_to('move', string.rep('x', 65), 0, 64, 0, 0.3)",
                "binding_token:too_long",
            ),
        ];
        let controls = RuntimeControls::unrestricted();

        for (case, call, expected) in cases {
            let source = format!("function on_server_tick(_event) {call} end");
            let mut runtime = LuaScriptRuntime::from_source(
                manifest.clone(),
                &source,
                LuaRuntimeLimits {
                    memory_bytes: NonZeroUsize::new(512 * 1024).unwrap(),
                    ..LuaRuntimeLimits::default()
                },
            )
            .unwrap();
            let error = runtime
                .handle_event(
                    &ScriptEvent::server_tick(1),
                    RuntimeContext::new(&controls, NonZeroUsize::new(1).unwrap()),
                )
                .unwrap_err();
            assert!(
                matches!(error, RuntimeError::Trap { message } if message.contains(expected)),
                "{case} did not reject at its raw boundary"
            );
        }
    }

    #[test]
    fn lua_table_count_and_shape_are_rejected_before_nested_dto_parsing() {
        let manifest = ScriptPluginManifest::new(
            "table-boundary",
            "Table Boundary",
            "0.1.0",
            SCRIPT_API_VERSION,
        )
        .subscribe_event("server.tick")
        .declare_inventory_menus()
        .validate()
        .unwrap();
        for (source, expected) in [
            (
                r#"
                    function on_server_tick(_event)
                        local slots = {}
                        for index = 1, 55 do
                            slots[index] = {slot=0, resource="minecraft:apple", count=1}
                        end
                        solaris.open_inventory_menu(7, "catalog", "Catalog", slots)
                    end
                "#,
                "menu_slots:too_many",
            ),
            (
                r#"
                    function on_server_tick(_event)
                        solaris.open_inventory_menu(7, "catalog", "Catalog", {
                            [1] = {slot=0, resource="minecraft:apple", count=1},
                            [3] = {slot=1, resource="minecraft:bread", count=1},
                        })
                    end
                "#,
                "menu_slots:shape",
            ),
            (
                r#"
                    function on_server_tick(_event)
                        solaris.open_inventory_menu(7, "catalog", "Catalog", {
                            {slot=0, resource="minecraft:apple", count=1, unknown="x"},
                        })
                    end
                "#,
                "menu_slot:unknown_field",
            ),
        ] {
            let mut runtime = LuaScriptRuntime::from_source(
                manifest.clone(),
                source,
                LuaRuntimeLimits::default(),
            )
            .unwrap();
            let error = runtime
                .handle_event(
                    &ScriptEvent::server_tick(1),
                    RuntimeContext::new(
                        &RuntimeControls::unrestricted(),
                        NonZeroUsize::new(1).unwrap(),
                    ),
                )
                .unwrap_err();
            assert!(matches!(
                error,
                RuntimeError::Trap { message } if message.contains(expected)
            ));
        }
    }

    #[test]
    fn lua_runtime_does_not_expose_filesystem_process_or_debug_libraries() {
        let mut runtime = LuaScriptRuntime::from_source(
            manifest(&["server.tick"]),
            r#"
                assert(os == nil)
                assert(io == nil)
                assert(package == nil)
                assert(debug == nil)

                function on_server_tick(_event)
                    solaris.broadcast("sandboxed")
                end
            "#,
            LuaRuntimeLimits::default(),
        )
        .unwrap();
        let controls = RuntimeControls::unrestricted();

        let batch = runtime
            .handle_event(
                &ScriptEvent::server_tick(1),
                RuntimeContext::new(&controls, NonZeroUsize::new(8).unwrap()),
            )
            .unwrap();

        assert_eq!(
            batch.commands(),
            &[ScriptCommand::BroadcastChatMessage {
                message: "sandboxed".to_owned(),
            }]
        );
    }

    #[tokio::test]
    async fn failed_handler_is_disabled_without_stopping_other_plugins() {
        let bad = PluginSource {
            manifest: manifest(&["server.tick"]),
            config: toml::Table::new(),
            source: r#"
                function on_server_tick(_event)
                    error("broken plugin")
                end
            "#
            .to_owned(),
            source_path: PathBuf::from("bad/main.lua"),
            worldgen_ore_profile: None,
            worldgen_settlement_plan: None,
            client_bundles: Vec::new(),
        };
        let good_manifest =
            ScriptPluginManifest::new("good-plugin", "Good Plugin", "0.1.0", SCRIPT_API_VERSION)
                .subscribe_event("server.tick")
                .validate()
                .unwrap();
        let good = PluginSource {
            manifest: good_manifest,
            config: toml::Table::new(),
            source: r#"
                function on_server_tick(event)
                    solaris.broadcast("tick " .. event.tick)
                end
            "#
            .to_owned(),
            source_path: PathBuf::from("good/main.lua"),
            worldgen_ore_profile: None,
            worldgen_settlement_plan: None,
            client_bundles: Vec::new(),
        };
        let (boundary, endpoint) =
            script_boundary_pair(NonZeroUsize::new(4).unwrap(), NonZeroUsize::new(4).unwrap());
        let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);
        let host = thread::spawn(move || run_lua_host(endpoint, vec![bad, good], startup_tx));
        assert_eq!(startup_rx.recv().unwrap(), 2);

        boundary
            .try_enqueue_event(ScriptEvent::server_tick(1))
            .unwrap();
        let first = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .expect("good plugin did not answer first tick")
            .expect("script command queue closed");
        assert!(matches!(
            first,
            ScriptCommand::HostAttached { provenance, request }
                if provenance.plugin_id() == "good-plugin"
                    && matches!(request.as_ref(), ScriptCommand::BroadcastChatMessage { message } if message == "tick 1")
        ));

        boundary
            .try_enqueue_event(ScriptEvent::server_tick(2))
            .unwrap();
        let second = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .expect("good plugin did not answer second tick")
            .expect("script command queue closed");
        assert!(matches!(
            second,
            ScriptCommand::HostAttached { provenance, request }
                if provenance.plugin_id() == "good-plugin"
                    && matches!(request.as_ref(), ScriptCommand::BroadcastChatMessage { message } if message == "tick 2")
        ));

        drop(boundary);
        tokio::task::spawn_blocking(move || host.join())
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn lua_host_skips_duplicate_plugin_ids() {
        let source = |path: &str| PluginSource {
            manifest: manifest(&["server.tick"]),
            config: toml::Table::new(),
            source: "function on_server_tick(_event) end".to_owned(),
            source_path: PathBuf::from(path),
            worldgen_ore_profile: None,
            worldgen_settlement_plan: None,
            client_bundles: Vec::new(),
        };
        let (boundary, endpoint) =
            script_boundary_pair(NonZeroUsize::new(4).unwrap(), NonZeroUsize::new(4).unwrap());
        let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);
        let host = thread::spawn(move || {
            run_lua_host(
                endpoint,
                vec![source("first/main.lua"), source("second/main.lua")],
                startup_tx,
            )
        });

        assert_eq!(startup_rx.recv().unwrap(), 1);

        drop(boundary);
        host.join().unwrap();
    }

    #[test]
    fn disk_manifest_parses_player_command_roots() {
        let disk: DiskManifest = toml::from_str(
            r#"
                id = "greetings"
                name = "Greetings"
                version = "0.1.0"
                api = "0.6.0"
                player_commands = ["hello", "hello", "warp_home"]
            "#,
        )
        .unwrap();

        assert_eq!(
            disk.player_commands,
            vec![
                "hello".to_owned(),
                "hello".to_owned(),
                "warp_home".to_owned()
            ]
        );
    }

    #[test]
    fn lua_host_rejects_the_later_plugin_when_player_command_roots_conflict() {
        let source = |id: &str, path: &str| PluginSource {
            manifest: command_manifest(id, "hello"),
            config: toml::Table::new(),
            source: "function on_player_command(_event) end".to_owned(),
            source_path: PathBuf::from(path),
            worldgen_ore_profile: None,
            worldgen_settlement_plan: None,
            client_bundles: Vec::new(),
        };
        let (boundary, endpoint) =
            script_boundary_pair(NonZeroUsize::new(4).unwrap(), NonZeroUsize::new(4).unwrap());
        let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);
        let host = thread::spawn(move || {
            run_lua_host(
                endpoint,
                vec![
                    source("first", "first/main.lua"),
                    source("second", "second/main.lua"),
                ],
                startup_tx,
            )
        });

        assert_eq!(startup_rx.recv().unwrap(), 1);
        assert_eq!(boundary.player_command_roots(), vec!["hello".to_owned()]);

        drop(boundary);
        host.join().unwrap();
    }

    #[tokio::test]
    async fn player_command_event_runs_only_the_owning_plugin() {
        let source = |id: &str, root: &str| PluginSource {
            manifest: command_manifest(id, root),
            config: toml::Table::new(),
            source: format!(
                r#"
                    function on_player_command(_event)
                        solaris.broadcast("{id}")
                    end
                "#
            ),
            source_path: PathBuf::from(format!("{id}/main.lua")),
            worldgen_ore_profile: None,
            worldgen_settlement_plan: None,
            client_bundles: Vec::new(),
        };
        let (boundary, endpoint) =
            script_boundary_pair(NonZeroUsize::new(4).unwrap(), NonZeroUsize::new(4).unwrap());
        let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);
        let host = thread::spawn(move || {
            run_lua_host(
                endpoint,
                vec![source("greetings", "hello"), source("farewells", "bye")],
                startup_tx,
            )
        });
        assert_eq!(startup_rx.recv().unwrap(), 2);

        assert_eq!(
            boundary.try_enqueue_player_command_with_context(
                ScriptPlayerId::new(7),
                player_context("Alex"),
                "hello one two",
            ),
            Ok(PlayerCommandAdmission::Enqueued)
        );
        let command = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .expect("owning plugin did not handle player command")
            .expect("script command queue closed");
        assert!(matches!(
            command,
            ScriptCommand::HostAttached { provenance, request }
                if provenance.plugin_id() == "greetings"
                    && matches!(request.as_ref(), ScriptCommand::BroadcastChatMessage { message } if message == "greetings")
        ));

        drop(boundary);
        tokio::task::spawn_blocking(move || host.join())
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn disabled_plugin_loses_player_command_ownership_before_host_progresses() {
        let bad = PluginSource {
            manifest: command_manifest("bad", "hello"),
            config: toml::Table::new(),
            source: r#"
                function on_player_command(_event)
                    error("broken plugin")
                end
            "#
            .to_owned(),
            source_path: PathBuf::from("bad/main.lua"),
            worldgen_ore_profile: None,
            worldgen_settlement_plan: None,
            client_bundles: Vec::new(),
        };
        let good = PluginSource {
            manifest: ScriptPluginManifest::new("good", "good", "0.1.0", SCRIPT_API_VERSION)
                .subscribe_event("server.tick")
                .validate()
                .unwrap(),
            config: toml::Table::new(),
            source: r#"
                function on_server_tick(_event)
                    solaris.broadcast("progressed")
                end
            "#
            .to_owned(),
            source_path: PathBuf::from("good/main.lua"),
            worldgen_ore_profile: None,
            worldgen_settlement_plan: None,
            client_bundles: Vec::new(),
        };
        let (boundary, endpoint) =
            script_boundary_pair(NonZeroUsize::new(4).unwrap(), NonZeroUsize::new(4).unwrap());
        let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);
        let host = thread::spawn(move || run_lua_host(endpoint, vec![bad, good], startup_tx));
        assert_eq!(startup_rx.recv().unwrap(), 2);

        assert_eq!(
            boundary.try_enqueue_player_command_with_context(
                ScriptPlayerId::new(7),
                player_context("Alex"),
                "hello",
            ),
            Ok(PlayerCommandAdmission::Enqueued)
        );
        boundary
            .try_enqueue_event(ScriptEvent::server_tick(1))
            .unwrap();
        let progress = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .expect("host did not process the event after the failed command")
            .expect("script command queue closed");
        assert!(matches!(
            progress,
            ScriptCommand::HostAttached { provenance, request }
                if provenance.plugin_id() == "good"
                    && matches!(request.as_ref(), ScriptCommand::BroadcastChatMessage { message } if message == "progressed")
        ));
        assert!(boundary.player_command_roots().is_empty());
        assert_eq!(
            boundary.try_enqueue_player_command_with_context(
                ScriptPlayerId::new(7),
                player_context("Alex"),
                "hello",
            ),
            Ok(PlayerCommandAdmission::NotOwned)
        );

        drop(boundary);
        tokio::task::spawn_blocking(move || host.join())
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn lua_rejects_an_undeclared_storage_capability_before_queuing_a_command() {
        let mut runtime = LuaScriptRuntime::from_source(
            manifest(&["server.tick"]),
            r#"
                function on_server_tick(_event)
                    solaris.storage_get("read", "balance:player-7")
                end
            "#,
            LuaRuntimeLimits::default(),
        )
        .unwrap();

        let error = runtime
            .handle_event(
                &ScriptEvent::server_tick(1),
                RuntimeContext::new(
                    &RuntimeControls::unrestricted(),
                    NonZeroUsize::new(1).unwrap(),
                ),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            RuntimeError::Trap { message }
                if message.contains("command capability denied: plugin_storage")
        ));
    }

    #[test]
    fn lua_permission_errors_render_only_the_bounded_capability_code() {
        let error = command_error(CommandBatchError::PermissionDenied {
            capability: crate::ScriptCommandCapabilityKind::RunConsoleCommand,
        });

        assert_eq!(
            error.to_string(),
            "runtime error: command capability denied: run_console_command"
        );
    }

    #[test]
    fn lua_extended_contract_emits_validated_dto_requests_and_respects_batch_capacity() {
        let manifest = ScriptPluginManifest::new(
            "contract-test",
            "Contract Test",
            "0.1.0",
            SCRIPT_API_VERSION,
        )
        .subscribe_event("server.tick")
        .declare_plugin_storage()
        .declare_inventory_menus()
        .declare_inventory_storage_transactions()
        .declare_zones()
        .declare_villagers()
        .validate()
        .unwrap();
        let mut runtime = LuaScriptRuntime::from_source(
            manifest,
            r#"
                function on_server_tick(_event)
                    solaris.storage_get("read", "coins:player-7")
                    solaris.storage_cas("write", "coins:player-7", 2, "9")
                    solaris.storage_delete("delete", "coins:obsolete", 3)
                    solaris.open_inventory_menu(7, "catalog", "Catalog", {
                        { slot = 0, resource = "minecraft:apple", count = 1, label = "Apple" },
                    })
                    solaris.inventory_storage_transaction(7, "purchase", {
                        { resource = "minecraft:apple", delta = 1 },
                    }, {
                        { operation = "cas", key = "coins:player-7", expected_version = 2, value = "6" },
                    })
                    solaris.upsert_zone("shop", "minecraft:overworld", 0, 60, 0, 8, 80, 8)
                    solaris.remove_zone("shop")
                    solaris.bind_nearest_villager("bind", 0, 64, 0, 16)
                    solaris.set_villager_idle("idle", "binding-1")
                end
            "#,
            LuaRuntimeLimits::default(),
        )
        .unwrap();
        let controls = RuntimeControls::unrestricted();
        let batch = runtime
            .handle_event(
                &ScriptEvent::server_tick(1),
                RuntimeContext::new(&controls, NonZeroUsize::new(9).unwrap()),
            )
            .unwrap();
        assert!(matches!(
            batch.commands(),
            [
                ScriptCommand::PluginStorageGet { .. },
                ScriptCommand::PluginStorageCompareAndSwap { .. },
                ScriptCommand::PluginStorageDelete { .. },
                ScriptCommand::OpenInventoryMenu { .. },
                ScriptCommand::InventoryStorageTransaction { .. },
                ScriptCommand::UpsertZone { .. },
                ScriptCommand::RemoveZone { .. },
                ScriptCommand::RequestVillagerBinding { .. },
                ScriptCommand::SetVillagerGoal { .. },
            ]
        ));

        let mut saturated = LuaScriptRuntime::from_source(
            ScriptPluginManifest::new("storage", "Storage", "0.1.0", SCRIPT_API_VERSION)
                .subscribe_event("server.tick")
                .declare_plugin_storage()
                .validate()
                .unwrap(),
            r#"
                function on_server_tick(_event)
                    solaris.storage_get("first", "coins:player-7")
                    solaris.storage_get("second", "coins:player-8")
                end
            "#,
            LuaRuntimeLimits::default(),
        )
        .unwrap();
        let error = saturated
            .handle_event(
                &ScriptEvent::server_tick(1),
                RuntimeContext::new(&controls, NonZeroUsize::new(1).unwrap()),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            RuntimeError::Trap { message } if message.contains("command limit 1 exceeded")
        ));
    }

    #[test]
    fn lua_villager_goal_api_emits_only_engine_goal_commands() {
        let manifest =
            ScriptPluginManifest::new("settlement", "Settlement", "0.1.0", SCRIPT_API_VERSION)
                .subscribe_event("server.tick")
                .declare_villagers()
                .validate()
                .unwrap();
        let controls = RuntimeControls::unrestricted();
        let mut runtime = LuaScriptRuntime::from_source(
            manifest,
            r#"
                function on_server_tick(_event)
                    solaris.move_villager_to("move-1", "binding-1", 8.5, 64, -3.5, 0.3)
                    solaris.set_villager_idle("idle-1", "binding-2")
                end
            "#,
            LuaRuntimeLimits::default(),
        )
        .unwrap();

        let batch = runtime
            .handle_event(
                &ScriptEvent::server_tick(1),
                RuntimeContext::new(&controls, NonZeroUsize::new(2).unwrap()),
            )
            .unwrap();
        assert!(matches!(
            batch.commands(),
            [
                ScriptCommand::SetVillagerGoal { request: moving },
                ScriptCommand::SetVillagerGoal { request: idle },
            ] if moving.goal().kind() == "follow_position"
                && moving.goal().target() == ScriptPosition::try_new(8.5, 64.0, -3.5)
                && moving.goal().speed() == Some(0.3)
                && idle.goal().kind() == "idle"
                && moving.binding_token() == "binding-1"
                && idle.binding_token() == "binding-2"
        ));
    }

    #[test]
    fn lua_villager_goal_rejections_are_synchronous_and_emit_no_command() {
        let controls = RuntimeControls::unrestricted();
        let cases = [
            (
                true,
                "solaris.move_villager_to(string.rep('x', 65), 'binding-1', 0, 64, 0, 0.3)",
            ),
            (
                true,
                "solaris.move_villager_to('move', string.rep('x', 65), 0, 64, 0, 0.3)",
            ),
            (
                true,
                "solaris.move_villager_to('move', 'binding-1', 0, 64, 0, 0)",
            ),
            (
                true,
                "solaris.move_villager_to('move', 'binding-1', 0, 64, 0, 4.1)",
            ),
            (false, "solaris.set_villager_idle('idle', 'binding-1')"),
        ];

        for (declare_villagers, call) in cases {
            let mut manifest =
                ScriptPluginManifest::new("settlement", "Settlement", "0.1.0", SCRIPT_API_VERSION)
                    .subscribe_event("server.tick");
            if declare_villagers {
                manifest = manifest.declare_villagers();
            }
            let source = format!(
                "function on_server_tick(_event) local accepted = pcall(function() {call} end); assert(not accepted) end"
            );
            let mut runtime = LuaScriptRuntime::from_source(
                manifest.validate().unwrap(),
                &source,
                LuaRuntimeLimits::default(),
            )
            .unwrap();
            let batch = runtime
                .handle_event(
                    &ScriptEvent::server_tick(1),
                    RuntimeContext::new(&controls, NonZeroUsize::new(1).unwrap()),
                )
                .unwrap();
            assert!(
                batch.commands().is_empty(),
                "rejected call emitted {batch:?}"
            );
        }
    }

    #[test]
    fn lua_villager_goal_result_uses_targeted_callback_and_exact_fields() {
        let manifest =
            ScriptPluginManifest::new("settlement", "Settlement", "0.1.0", SCRIPT_API_VERSION)
                .subscribe_event("villager.goal_result")
                .validate()
                .unwrap();
        let mut runtime = LuaScriptRuntime::from_source(
            manifest,
            r#"
                function on_villager_goal_result(event)
                    solaris.broadcast(event.request_id .. ":" .. event.goal .. ":" .. tostring(event.accepted) .. ":" .. tostring(event.failure))
                end
            "#,
            LuaRuntimeLimits::default(),
        )
        .unwrap();
        let request = crate::ScriptVillagerGoalRequest::try_new(
            "goal-1",
            "binding-1",
            crate::ScriptVillagerGoal::idle(),
        )
        .unwrap();
        let event = ScriptEvent::villager_goal_result(
            "settlement",
            &request,
            Some(crate::ScriptVillagerGoalFailure::BindingUnavailable),
        )
        .unwrap();
        let controls = RuntimeControls::unrestricted();
        let batch = runtime
            .handle_event(
                &event,
                RuntimeContext::new(&controls, NonZeroUsize::new(1).unwrap()),
            )
            .unwrap();
        assert_eq!(
            batch.commands(),
            &[ScriptCommand::BroadcastChatMessage {
                message: "goal-1:idle:false:binding_unavailable".to_owned(),
            }]
        );
    }

    #[tokio::test]
    async fn lua_host_attaches_loaded_plugin_identity_and_isolates_targeted_result_events() {
        let source = |id: &str| PluginSource {
            manifest: ScriptPluginManifest::new(id, id, "0.1.0", SCRIPT_API_VERSION)
                .declare_plugin_storage()
                .validate()
                .unwrap(),
            config: toml::Table::new(),
            source: format!(
                r#"
                    local claimed_plugin_id = "forged-plugin"
                    function on_plugin_storage_get_result(_event)
                        solaris.broadcast("{id}:" .. claimed_plugin_id)
                    end
                "#
            ),
            source_path: PathBuf::from(format!("{id}/main.lua")),
            worldgen_ore_profile: None,
            worldgen_settlement_plan: None,
            client_bundles: Vec::new(),
        };
        let (boundary, endpoint) =
            script_boundary_pair(NonZeroUsize::new(2).unwrap(), NonZeroUsize::new(2).unwrap());
        let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);
        let host = thread::spawn(move || {
            run_lua_host(endpoint, vec![source("owner"), source("other")], startup_tx)
        });
        assert_eq!(startup_rx.recv().unwrap(), 2);
        let request = ScriptPluginStorageGetRequest::try_new("read", "balance:player-7").unwrap();

        boundary
            .try_enqueue_event(
                ScriptEvent::plugin_storage_get_result(
                    "owner",
                    &request,
                    Some("9".to_owned()),
                    Some(1),
                )
                .unwrap(),
            )
            .unwrap();
        let command = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .expect("target plugin did not receive its result")
            .expect("script command queue closed");
        assert!(matches!(
            command,
            ScriptCommand::HostAttached {
                provenance,
                request,
            } if provenance.plugin_id() == "owner"
                && matches!(request.as_ref(), ScriptCommand::BroadcastChatMessage { message } if message == "owner:forged-plugin")
        ));

        drop(boundary);
        tokio::task::spawn_blocking(move || host.join())
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn example_plugins_load_against_the_contract_without_live_server_adapters() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins");
        let controls = RuntimeControls::unrestricted();
        for (name, expected_commands) in [
            ("basic-economy", 1_usize),
            ("colony-villager-scaffold", 2_usize),
            ("geological-mines", 0_usize),
            ("land-claims", 1_usize),
            ("online-roster", 0_usize),
            ("settlement-prototype", 0_usize),
        ] {
            let source = read_plugin_source(&examples.join(name)).unwrap();
            assert_eq!(source.manifest.requested_api_version(), SCRIPT_API_VERSION);
            let mut runtime = LuaScriptRuntime::from_source_with_config(
                source.manifest,
                &source.source,
                source.config,
                LuaRuntimeLimits::default(),
            )
            .unwrap();
            let batch = runtime
                .handle_event(
                    &ScriptEvent::server_started(),
                    RuntimeContext::new(&controls, NonZeroUsize::new(8).unwrap()),
                )
                .unwrap();
            assert_eq!(batch.commands().len(), expected_commands, "{name}");
        }
    }

    #[test]
    fn online_roster_recovers_rejected_queries_and_bounds_menu_labels() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins");
        let source = read_plugin_source(&examples.join("online-roster")).unwrap();
        let mut runtime = LuaScriptRuntime::from_source_with_config(
            source.manifest,
            &source.source,
            source.config,
            LuaRuntimeLimits::default(),
        )
        .unwrap();
        let controls = RuntimeControls::unrestricted();
        let command = ScriptEvent::try_player_command_with_context(
            "online-roster",
            ScriptPlayerId::new(7),
            player_context("SixteenCharName1"),
            "who",
            "",
        )
        .unwrap();

        let first = runtime
            .handle_event(
                &command,
                RuntimeContext::new(&controls, NonZeroUsize::new(1).unwrap()),
            )
            .unwrap();
        assert!(matches!(
            first.commands(),
            [ScriptCommand::ListOnlinePlayers { .. }]
        ));
        runtime.notify_batch_rejected("queue_full", 1).unwrap();

        let second = runtime
            .handle_event(
                &command,
                RuntimeContext::new(&controls, NonZeroUsize::new(1).unwrap()),
            )
            .unwrap();
        let [ScriptCommand::ListOnlinePlayers { request }] = second.commands() else {
            panic!("rejected query must be retryable");
        };
        let request = request.clone();
        let dimension = format!("minecraft:{}", "a".repeat(118));
        let player = crate::ScriptOnlinePlayerSnapshot::try_new(
            ScriptPlayerId::new(7),
            player_context("SixteenCharName1"),
            &dimension,
        )
        .unwrap();
        let result =
            ScriptEvent::online_players_result("online-roster", &request, vec![player], false)
                .unwrap();
        let menu = runtime
            .handle_event(
                &result,
                RuntimeContext::new(&controls, NonZeroUsize::new(1).unwrap()),
            )
            .unwrap();
        let [ScriptCommand::OpenInventoryMenu { menu, .. }] = menu.commands() else {
            panic!("online result must open one roster menu");
        };
        assert_eq!(menu.slots()[0].item().label().unwrap().len(), 128);
    }

    #[test]
    fn plugin_files_are_rejected_at_the_streaming_size_boundary() {
        let root = std::env::temp_dir().join(format!(
            "solaris-mc-script-bounds-{}-{}",
            std::process::id(),
            TEST_TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let plugin = root.join("oversized");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        fs::create_dir(&plugin).unwrap();
        fs::write(
            plugin.join("plugin.toml"),
            "x".repeat(MAX_PLUGIN_MANIFEST_BYTES + 1),
        )
        .unwrap();
        fs::write(plugin.join("main.lua"), "return nil").unwrap();
        assert!(
            read_plugin_source(&plugin)
                .unwrap_err()
                .contains("plugin.toml exceeds")
        );

        fs::write(
            plugin.join("plugin.toml"),
            "id='bounded'\nname='Bounded'\nversion='0.1.0'\napi='0.6.0'\n",
        )
        .unwrap();
        fs::write(
            plugin.join("main.lua"),
            "x".repeat(MAX_PLUGIN_SOURCE_BYTES + 1),
        )
        .unwrap();
        assert!(
            read_plugin_source(&plugin)
                .unwrap_err()
                .contains("main.lua exceeds")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn poisoned_invocation_authority_disables_all_future_handlers() {
        let mut runtime = LuaScriptRuntime::from_source(
            manifest(&["server.tick"]),
            "function on_server_tick(_) solaris.broadcast('must-not-run') end",
            LuaRuntimeLimits::default(),
        )
        .unwrap();
        let invocation = Arc::clone(&runtime.invocation);
        thread::spawn(move || {
            let _guard = invocation.lock().unwrap();
            panic!("poison invocation authority");
        })
        .join()
        .unwrap_err();
        let controls = RuntimeControls::unrestricted();

        for tick in [1, 2] {
            let error = runtime
                .handle_event(
                    &ScriptEvent::server_tick(tick),
                    RuntimeContext::new(&controls, NonZeroUsize::new(1).unwrap()),
                )
                .unwrap_err();
            assert!(matches!(
                error,
                RuntimeError::Trap { message } if message.contains("authority poisoned")
            ));
        }
    }
}
