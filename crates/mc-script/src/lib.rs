//! # mc-script
//!
//! Safe script runtime contracts and the built-in Luau plugin host.
//!
//! Immutable event snapshots enter runtimes and bounded command batches leave
//! them. The optional `lua-runtime` feature adds an isolated Luau VM per plugin on
//! one dedicated host thread, with fixed memory and execution-fuel limits.

use std::collections::BTreeMap;
use std::fmt;
use std::num::{NonZeroU64, NonZeroUsize};
#[cfg(any(test, feature = "lua-runtime"))]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock, Weak};
use std::time::Duration;

use tokio::sync::{Mutex, mpsc};

#[cfg(feature = "lua-runtime")]
mod lua;

#[cfg(test)]
mod entity_interaction_tests;
#[cfg(test)]
mod entity_kill_tests;
#[cfg(test)]
mod item_pickup_tests;
#[cfg(test)]
mod player_death_tests;
#[cfg(test)]
mod player_inventory_tests;
#[cfg(test)]
mod player_query_tests;
#[cfg(test)]
mod player_teleport_tests;
#[cfg(test)]
mod tick_delivery_tests;

#[cfg(feature = "lua-runtime")]
pub use lua::{
    BundledLuauPlugin, LuaClientBundle, LuaClientContentKind, LuaClientLoader, LuaClientPermission,
    LuaHost, LuaHostConfig, LuaHostError, LuaSettlementBuilding, LuaSettlementBuildingRole,
    LuaSettlementBuildingTemplate, LuaSettlementExtension, LuaSettlementInhabitant,
    LuaSettlementInhabitantKind, LuaSettlementJob, LuaSettlementPlan, LuaWorldgenOreProfile,
    LuaWorldgenSettlementProfile, PreparedLuaPlugins, prepare_bundled_luau_plugins,
    prepare_lua_plugins, start_lua_host, start_prepared_lua_host,
};

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Semantic version of the stable script API contract.
pub const SCRIPT_API_VERSION: ScriptApiVersion = ScriptApiVersion::new(0, 6, 0);

/// Maximum entity types one plugin may allow-list for spawning.
pub const MAX_SPAWN_ENTITY_TYPES: usize = 32;

/// Maximum byte length of a script-visible namespaced resource identifier.
pub const MAX_SCRIPT_RESOURCE_ID_BYTES: usize = 128;
pub const MAX_SCRIPT_LOADER_INTERACTION_PAYLOAD_BYTES: usize = 4_096;

/// Maximum byte length of a plugin-scoped identifier or request correlation id.
pub const MAX_SCRIPT_ID_BYTES: usize = 64;

pub const MAX_SCRIPT_PLAYER_UUID_BYTES: usize = 64;
pub const MAX_SCRIPT_PLAYER_NAME_BYTES: usize = 16;
pub const MAX_SCRIPT_CHAT_MESSAGE_BYTES: usize = 4_096;
pub const MAX_SCRIPT_DISCONNECT_REASON_BYTES: usize = 1_024;
pub const MAX_SCRIPT_CONSOLE_COMMAND_BYTES: usize = 256;
pub const MAX_SCRIPT_COMMAND_BATCH: usize = 32;
pub const MAX_SCRIPT_EVENT_QUEUE_CAPACITY: usize = 1_024;
pub const MAX_SCRIPT_COMMAND_QUEUE_CAPACITY: usize = 256;
/// Maximum connected-player snapshots returned by one plugin query.
pub const MAX_ONLINE_PLAYER_QUERY_LIMIT: usize = 256;
pub const MAX_PLUGIN_ID_BYTES: usize = 64;
pub const MAX_PLUGIN_DISPLAY_NAME_BYTES: usize = 128;
pub const MAX_PLUGIN_VERSION_BYTES: usize = 64;
pub const MAX_MANIFEST_EVENT_SUBSCRIPTIONS: usize = 64;
pub const MAX_MANIFEST_DEPENDENCIES: usize = 64;
pub const MAX_MANIFEST_CAPABILITIES: usize = 128;
pub const MAX_MANIFEST_PERMISSIONS: usize = 64;
pub const MAX_MANIFEST_FIELD_BYTES: usize = 128;

/// Maximum byte length of a plugin storage key.
pub const MAX_PLUGIN_STORAGE_KEY_BYTES: usize = 128;

/// Maximum byte length of a plugin storage value.
pub const MAX_PLUGIN_STORAGE_VALUE_BYTES: usize = 4_096;

/// Maximum number of server-owned menu slots a plugin may describe.
pub const MAX_INVENTORY_MENU_SLOTS: usize = 54;

/// Maximum atomic inventory and storage mutations in one plugin request.
pub const MAX_INVENTORY_STORAGE_MUTATIONS: usize = 16;

/// Maximum absolute resource count changed by one inventory delta.
pub const MAX_INVENTORY_RESOURCE_DELTA: i16 = 64;

/// Maximum byte length of a server-rendered inventory menu title.
pub const MAX_INVENTORY_MENU_TITLE_BYTES: usize = 128;

/// Maximum search radius for an ephemeral villager binding request.
pub const MAX_VILLAGER_BINDING_RADIUS: f64 = 64.0;

/// Maximum movement speed exposed through a bound-villager goal request.
pub const MAX_VILLAGER_GOAL_SPEED: f64 = 4.0;

/// Maximum absolute horizontal coordinate accepted from Lua.
pub const SCRIPT_HORIZONTAL_COORDINATE_LIMIT: f64 = 30_000_000.0;

/// Maximum absolute vertical coordinate accepted from Lua.
pub const SCRIPT_VERTICAL_COORDINATE_LIMIT: f64 = 20_000_000.0;

/// Maximum byte length of one ASCII plugin player-command root.
pub const MAX_PLAYER_COMMAND_ROOT_BYTES: usize = 64;

/// Maximum number of active plugin player-command roots across the server.
pub const MAX_PLAYER_COMMAND_ROOTS: usize = 128;

/// Maximum command-tree nodes added by active plugin roots.
pub const MAX_PLAYER_COMMAND_TREE_NODES: usize = MAX_PLAYER_COMMAND_ROOTS * 2;

/// Player command roots reserved by Solaris' built-in command parser.
pub const BUILT_IN_PLAYER_COMMAND_ROOTS: &[&str] = &[
    "debug",
    "defaultgamemode",
    "gamemode",
    "gamerule",
    "give",
    "kill",
    "save-all",
    "status",
    "stop",
    "summon",
    "teleport",
    "time",
    "tp",
];

const fn decimal_u8_len(value: u8) -> usize {
    if value >= 100 {
        3
    } else if value >= 10 {
        2
    } else {
        1
    }
}

/// Result of admitting a player command to a plugin-owned root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlayerCommandAdmission {
    NotOwned,
    Enqueued,
    Dropped,
    PermissionDenied,
    OwnedRejected { error: ScriptDtoError },
}

/// Version requested by a script runtime or supported by the server host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScriptApiVersion {
    major: u16,
    minor: u16,
    patch: u16,
}

impl ScriptApiVersion {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    pub const fn major(&self) -> u16 {
        self.major
    }

    pub const fn minor(&self) -> u16 {
        self.minor
    }

    pub const fn patch(&self) -> u16 {
        self.patch
    }
}

pub const fn supports_script_api_version(requested: ScriptApiVersion) -> bool {
    requested.major == SCRIPT_API_VERSION.major
        && requested.minor == SCRIPT_API_VERSION.minor
        && requested.patch == SCRIPT_API_VERSION.patch
}

/// Stable player identifier snapshot for script-visible DTOs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ScriptPlayerId(u64);

impl ScriptPlayerId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

/// Validated immutable position carried by script commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ScriptPosition {
    x_bits: u64,
    y_bits: u64,
    z_bits: u64,
}

impl ScriptPosition {
    #[must_use]
    pub fn try_new(x: f64, y: f64, z: f64) -> Option<Self> {
        (x.is_finite()
            && y.is_finite()
            && z.is_finite()
            && x.abs() <= SCRIPT_HORIZONTAL_COORDINATE_LIMIT
            && y.abs() <= SCRIPT_VERTICAL_COORDINATE_LIMIT
            && z.abs() <= SCRIPT_HORIZONTAL_COORDINATE_LIMIT)
            .then_some(Self {
                x_bits: x.to_bits(),
                y_bits: y.to_bits(),
                z_bits: z.to_bits(),
            })
    }

    #[must_use]
    pub fn x(self) -> f64 {
        f64::from_bits(self.x_bits)
    }

    #[must_use]
    pub fn y(self) -> f64 {
        f64::from_bits(self.y_bits)
    }

    #[must_use]
    pub fn z(self) -> f64 {
        f64::from_bits(self.z_bits)
    }
}

/// Bounded same-dimension player teleport requested by one plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPlayerTeleportRequest {
    request_id: String,
    player_id: ScriptPlayerId,
    position: ScriptPosition,
}

impl ScriptPlayerTeleportRequest {
    pub fn try_new(
        request_id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        position: ScriptPosition,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            request_id: validate_script_id(request_id.as_ref())?,
            player_id,
            position,
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub const fn player_id(&self) -> ScriptPlayerId {
        self.player_id
    }

    pub const fn position(&self) -> ScriptPosition {
        self.position
    }
}

/// Exact reason why an admitted player teleport did not commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptPlayerTeleportFailure {
    PlayerUnavailable,
    TeleportPending,
    RuntimeUnavailable,
}

impl ScriptPlayerTeleportFailure {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PlayerUnavailable => "player_unavailable",
            Self::TeleportPending => "teleport_pending",
            Self::RuntimeUnavailable => "runtime_unavailable",
        }
    }
}

/// Bounded request for a point-in-time connected-player snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptOnlinePlayersRequest {
    request_id: String,
    limit: usize,
}

impl ScriptOnlinePlayersRequest {
    pub fn try_new(request_id: impl AsRef<str>, limit: usize) -> Result<Self, ScriptDtoError> {
        if limit == 0 || limit > MAX_ONLINE_PLAYER_QUERY_LIMIT {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(Self {
            request_id: validate_script_id(request_id.as_ref())?,
            limit,
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub const fn limit(&self) -> usize {
        self.limit
    }
}

/// Immutable server-authoritative player context attached to gameplay events.
///
/// This is a point-in-time value. It deliberately contains no connection or
/// network-address data and cannot be used to query live server state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPlayerContext {
    snapshot: Box<ScriptPlayerContextSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScriptPlayerContextSnapshot {
    uuid: String,
    username: String,
    operator: bool,
    x_bits: u64,
    y_bits: u64,
    z_bits: u64,
}

impl ScriptPlayerContext {
    pub fn try_new(
        uuid: impl AsRef<str>,
        username: impl AsRef<str>,
        operator: bool,
        x: f64,
        y: f64,
        z: f64,
    ) -> Result<Self, ScriptDtoError> {
        let uuid = uuid.as_ref();
        let username = username.as_ref();
        validate_bounded_nonempty("player uuid", uuid, MAX_SCRIPT_PLAYER_UUID_BYTES)?;
        validate_bounded_nonempty("player username", username, MAX_SCRIPT_PLAYER_NAME_BYTES)?;
        validate_player_identity(uuid, username)?;
        let position = ScriptPosition::try_new(x, y, z).ok_or(ScriptDtoError::InvalidBounds)?;
        Ok(Self {
            snapshot: Box::new(ScriptPlayerContextSnapshot {
                uuid: uuid.to_owned(),
                username: username.to_owned(),
                operator,
                x_bits: position.x_bits,
                y_bits: position.y_bits,
                z_bits: position.z_bits,
            }),
        })
    }

    #[must_use]
    pub fn new(
        uuid: impl AsRef<str>,
        username: impl AsRef<str>,
        operator: bool,
        x: f64,
        y: f64,
        z: f64,
    ) -> Self {
        Self::try_new(uuid, username, operator, x, y, z)
            .expect("server-authored script player context must be bounded")
    }

    #[must_use]
    pub fn uuid(&self) -> &str {
        &self.snapshot.uuid
    }

    #[must_use]
    pub fn username(&self) -> &str {
        &self.snapshot.username
    }

    #[must_use]
    pub fn operator(&self) -> bool {
        self.snapshot.operator
    }

    #[must_use]
    pub fn x(&self) -> f64 {
        f64::from_bits(self.snapshot.x_bits)
    }

    #[must_use]
    pub fn y(&self) -> f64 {
        f64::from_bits(self.snapshot.y_bits)
    }

    #[must_use]
    pub fn z(&self) -> f64 {
        f64::from_bits(self.snapshot.z_bits)
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_bounded_nonempty("player uuid", self.uuid(), MAX_SCRIPT_PLAYER_UUID_BYTES)?;
        validate_bounded_nonempty(
            "player username",
            self.username(),
            MAX_SCRIPT_PLAYER_NAME_BYTES,
        )?;
        validate_player_identity(self.uuid(), self.username())?;
        ScriptPosition::try_new(self.x(), self.y(), self.z())
            .ok_or(ScriptDtoError::InvalidBounds)
            .map(drop)
    }
}

/// Immutable identity, pose, and dimension for one connected player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptOnlinePlayerSnapshot {
    player_id: ScriptPlayerId,
    context: ScriptPlayerContext,
    dimension: String,
}

impl ScriptOnlinePlayerSnapshot {
    pub fn try_new(
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: impl AsRef<str>,
    ) -> Result<Self, ScriptDtoError> {
        context.validate()?;
        let dimension = dimension.as_ref();
        validate_contract_resource_id(dimension)?;
        Ok(Self {
            player_id,
            context,
            dimension: dimension.to_owned(),
        })
    }

    pub const fn player_id(&self) -> ScriptPlayerId {
        self.player_id
    }

    pub fn context(&self) -> &ScriptPlayerContext {
        &self.context
    }

    pub fn dimension(&self) -> &str {
        &self.dimension
    }

    fn validate(&self) -> Result<(), ScriptDtoError> {
        self.context.validate()?;
        validate_contract_resource_id(&self.dimension).map(drop)
    }
}

/// Stable entity identifier snapshot for script-visible DTOs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ScriptEntityId(u64);

impl ScriptEntityId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

/// Validation failure for a bounded script DTO.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptDtoError {
    InvalidId {
        field: &'static str,
        actual_bytes: usize,
    },
    InvalidResourceId {
        field: &'static str,
        actual_bytes: usize,
    },
    ValueTooLong {
        field: &'static str,
        max_bytes: usize,
        actual_bytes: usize,
    },
    EmptyValue {
        field: &'static str,
    },
    InconsistentResult {
        field: &'static str,
    },
    InvalidBounds,
    InvalidAmount,
    EmptyTransaction,
    TooManyEntries {
        field: &'static str,
        max: usize,
    },
    DuplicateId {
        field: &'static str,
        actual_bytes: usize,
    },
}

impl fmt::Display for ScriptDtoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId {
                field,
                actual_bytes,
            } => write!(formatter, "{field}:invalid_id:{actual_bytes}"),
            Self::InvalidResourceId {
                field,
                actual_bytes,
            } => write!(formatter, "{field}:invalid_resource_id:{actual_bytes}"),
            Self::ValueTooLong {
                field,
                max_bytes,
                actual_bytes,
            } => write!(formatter, "{field}:too_long:{actual_bytes}:{max_bytes}"),
            Self::EmptyValue { field } => write!(formatter, "{field}:empty"),
            Self::InconsistentResult { field } => write!(formatter, "{field}:inconsistent"),
            Self::InvalidBounds => formatter.write_str("bounds:invalid"),
            Self::InvalidAmount => formatter.write_str("amount:invalid"),
            Self::EmptyTransaction => formatter.write_str("transaction:empty"),
            Self::TooManyEntries { field, max } => {
                write!(formatter, "{field}:too_many:{max}")
            }
            Self::DuplicateId {
                field,
                actual_bytes,
            } => write!(formatter, "{field}:duplicate:{actual_bytes}"),
        }
    }
}

impl std::error::Error for ScriptDtoError {}

/// Generic actor policy attached to a protected zone by its owning plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptZoneProtection {
    allowed_actor_uuid: String,
}

impl ScriptZoneProtection {
    pub fn try_actor_or_operator(
        allowed_actor_uuid: impl AsRef<str>,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            allowed_actor_uuid: normalize_player_uuid(allowed_actor_uuid.as_ref())?,
        })
    }

    pub fn allowed_actor_uuid(&self) -> &str {
        &self.allowed_actor_uuid
    }

    pub fn allows_actor(&self, actor_uuid: &str, operator: bool) -> bool {
        operator
            || normalize_player_uuid(actor_uuid)
                .is_ok_and(|actor_uuid| actor_uuid == self.allowed_actor_uuid)
    }
}

/// A bounded axis-aligned zone definition. The server owns its lifecycle and membership checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptAxisAlignedZone {
    id: String,
    dimension: String,
    minimum: ScriptPosition,
    maximum: ScriptPosition,
    protection: Option<ScriptZoneProtection>,
}

impl ScriptAxisAlignedZone {
    pub fn try_new(
        id: impl AsRef<str>,
        dimension: impl AsRef<str>,
        minimum: ScriptPosition,
        maximum: ScriptPosition,
    ) -> Result<Self, ScriptDtoError> {
        Self::try_new_with_protection(id, dimension, minimum, maximum, None)
    }

    pub fn try_new_with_protection(
        id: impl AsRef<str>,
        dimension: impl AsRef<str>,
        minimum: ScriptPosition,
        maximum: ScriptPosition,
        protection: Option<ScriptZoneProtection>,
    ) -> Result<Self, ScriptDtoError> {
        if minimum.x() > maximum.x() || minimum.y() > maximum.y() || minimum.z() > maximum.z() {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(Self {
            id: validate_script_id(id.as_ref())?,
            dimension: validate_contract_resource_id(dimension.as_ref())?,
            minimum,
            maximum,
            protection,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn dimension(&self) -> &str {
        &self.dimension
    }

    pub fn minimum(&self) -> ScriptPosition {
        self.minimum
    }

    pub fn maximum(&self) -> ScriptPosition {
        self.maximum
    }

    pub fn protection(&self) -> Option<&ScriptZoneProtection> {
        self.protection.as_ref()
    }
}

/// One immutable item descriptor used in a server-owned inventory menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptInventoryMenuItem {
    resource_id: String,
    count: u8,
    label: Option<String>,
}

impl ScriptInventoryMenuItem {
    pub fn try_new(
        resource_id: impl AsRef<str>,
        count: u8,
        label: Option<String>,
    ) -> Result<Self, ScriptDtoError> {
        if count == 0 {
            return Err(ScriptDtoError::InvalidAmount);
        }
        if let Some(label) = &label
            && label.len() > MAX_INVENTORY_MENU_TITLE_BYTES
        {
            return Err(ScriptDtoError::ValueTooLong {
                field: "inventory menu item label",
                max_bytes: MAX_INVENTORY_MENU_TITLE_BYTES,
                actual_bytes: label.len(),
            });
        }
        Ok(Self {
            resource_id: validate_contract_resource_id(resource_id.as_ref())?,
            count,
            label,
        })
    }

    pub fn resource_id(&self) -> &str {
        &self.resource_id
    }

    pub const fn count(&self) -> u8 {
        self.count
    }

    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }
}

/// One fixed slot in a server-owned inventory menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptInventoryMenuSlot {
    index: u8,
    item: ScriptInventoryMenuItem,
}

impl ScriptInventoryMenuSlot {
    pub const fn new(index: u8, item: ScriptInventoryMenuItem) -> Self {
        Self { index, item }
    }

    pub const fn index(&self) -> u8 {
        self.index
    }

    pub fn item(&self) -> &ScriptInventoryMenuItem {
        &self.item
    }
}

/// Immutable menu description requested by a plugin and owned by the server after admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptInventoryMenu {
    id: String,
    title: String,
    slots: Vec<ScriptInventoryMenuSlot>,
}

impl ScriptInventoryMenu {
    pub fn try_new(
        id: impl AsRef<str>,
        title: impl AsRef<str>,
        slots: Vec<ScriptInventoryMenuSlot>,
    ) -> Result<Self, ScriptDtoError> {
        let title = title.as_ref();
        validate_bounded_nonempty(
            "inventory menu title",
            title,
            MAX_INVENTORY_MENU_TITLE_BYTES,
        )?;
        if slots.len() > MAX_INVENTORY_MENU_SLOTS {
            return Err(ScriptDtoError::TooManyEntries {
                field: "inventory menu slots",
                max: MAX_INVENTORY_MENU_SLOTS,
            });
        }
        let mut indexes = BTreeMap::new();
        for slot in &slots {
            if usize::from(slot.index) >= MAX_INVENTORY_MENU_SLOTS {
                return Err(ScriptDtoError::InvalidBounds);
            }
            if indexes.insert(slot.index, ()).is_some() {
                return Err(ScriptDtoError::DuplicateId {
                    field: "inventory menu slot index",
                    actual_bytes: decimal_u8_len(slot.index),
                });
            }
        }
        Ok(Self {
            id: validate_script_id(id.as_ref())?,
            title: title.to_owned(),
            slots,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn slots(&self) -> &[ScriptInventoryMenuSlot] {
        &self.slots
    }
}

/// One logical player-inventory resource delta in an atomic transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptInventoryResourceDelta {
    resource_id: String,
    delta: i16,
}

impl ScriptInventoryResourceDelta {
    pub fn try_new(resource_id: impl AsRef<str>, delta: i16) -> Result<Self, ScriptDtoError> {
        if delta == 0 || delta.unsigned_abs() > MAX_INVENTORY_RESOURCE_DELTA as u16 {
            return Err(ScriptDtoError::InvalidAmount);
        }
        Ok(Self {
            resource_id: validate_contract_resource_id(resource_id.as_ref())?,
            delta,
        })
    }

    pub fn resource_id(&self) -> &str {
        &self.resource_id
    }

    pub const fn delta(&self) -> i16 {
        self.delta
    }
}

/// One plugin-storage mutation in an atomic inventory and storage transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptStorageMutation {
    CompareAndSwap {
        key: String,
        expected_version: Option<u64>,
        value: String,
    },
    Delete {
        key: String,
        expected_version: Option<u64>,
    },
}

impl ScriptStorageMutation {
    pub fn compare_and_swap(
        key: impl AsRef<str>,
        expected_version: Option<u64>,
        value: impl AsRef<str>,
    ) -> Result<Self, ScriptDtoError> {
        let value = value.as_ref();
        validate_plugin_storage_value(value)?;
        Ok(Self::CompareAndSwap {
            key: validate_plugin_storage_key(key.as_ref())?,
            expected_version,
            value: value.to_owned(),
        })
    }

    pub fn delete(
        key: impl AsRef<str>,
        expected_version: Option<u64>,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self::Delete {
            key: validate_plugin_storage_key(key.as_ref())?,
            expected_version,
        })
    }

    pub fn key(&self) -> &str {
        match self {
            Self::CompareAndSwap { key, .. } | Self::Delete { key, .. } => key,
        }
    }

    fn validate(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::CompareAndSwap { key, value, .. } => {
                validate_plugin_storage_key(key)?;
                validate_plugin_storage_value(value)
            }
            Self::Delete { key, .. } => validate_plugin_storage_key(key).map(drop),
        }
    }
}

/// Validated plugin-storage read request used to correlate its targeted result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPluginStorageGetRequest {
    request_id: String,
    key: String,
}

impl ScriptPluginStorageGetRequest {
    pub fn try_new(
        request_id: impl AsRef<str>,
        key: impl AsRef<str>,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            request_id: validate_script_id(request_id.as_ref())?,
            key: validate_plugin_storage_key(key.as_ref())?,
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn key(&self) -> &str {
        &self.key
    }
}

/// Validated plugin-storage compare-and-swap request and result correlation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPluginStorageCompareAndSwapRequest {
    request_id: String,
    key: String,
    expected_version: Option<u64>,
    value: String,
}

impl ScriptPluginStorageCompareAndSwapRequest {
    pub fn try_new(
        request_id: impl AsRef<str>,
        key: impl AsRef<str>,
        expected_version: Option<u64>,
        value: impl AsRef<str>,
    ) -> Result<Self, ScriptDtoError> {
        let value = value.as_ref();
        validate_plugin_storage_value(value)?;
        Ok(Self {
            request_id: validate_script_id(request_id.as_ref())?,
            key: validate_plugin_storage_key(key.as_ref())?,
            expected_version,
            value: value.to_owned(),
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub const fn expected_version(&self) -> Option<u64> {
        self.expected_version
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

/// Validated plugin-storage delete request used to correlate its targeted result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPluginStorageDeleteRequest {
    request_id: String,
    key: String,
    expected_version: Option<u64>,
}

/// Explicit terminal reason for a plugin-storage request that could not run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptPluginStorageFailure {
    /// This server has no persistent world-backed plugin storage.
    Unavailable,
    /// The storage actor stopped after a durable write or synchronization failure.
    DurabilityFailed,
}

impl ScriptPluginStorageFailure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::DurabilityFailed => "durability_failed",
        }
    }
}

impl ScriptPluginStorageDeleteRequest {
    pub fn try_new(
        request_id: impl AsRef<str>,
        key: impl AsRef<str>,
        expected_version: Option<u64>,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            request_id: validate_script_id(request_id.as_ref())?,
            key: validate_plugin_storage_key(key.as_ref())?,
            expected_version,
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub const fn expected_version(&self) -> Option<u64> {
        self.expected_version
    }
}

/// A bounded request the server must commit or reject as one inventory and storage mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptInventoryStorageTransaction {
    id: String,
    player_id: ScriptPlayerId,
    inventory: Vec<ScriptInventoryResourceDelta>,
    storage: Vec<ScriptStorageMutation>,
}

impl ScriptInventoryStorageTransaction {
    pub fn try_new(
        id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        inventory: Vec<ScriptInventoryResourceDelta>,
        storage: Vec<ScriptStorageMutation>,
    ) -> Result<Self, ScriptDtoError> {
        if inventory.is_empty() || storage.is_empty() {
            return Err(ScriptDtoError::EmptyTransaction);
        }
        if inventory.len() > MAX_INVENTORY_STORAGE_MUTATIONS
            || storage.len() > MAX_INVENTORY_STORAGE_MUTATIONS
        {
            return Err(ScriptDtoError::TooManyEntries {
                field: "inventory storage transaction",
                max: MAX_INVENTORY_STORAGE_MUTATIONS,
            });
        }
        let mut inventory_ids = BTreeMap::new();
        for delta in &inventory {
            if inventory_ids.insert(delta.resource_id(), ()).is_some() {
                return Err(ScriptDtoError::DuplicateId {
                    field: "inventory resource id",
                    actual_bytes: delta.resource_id().len(),
                });
            }
        }
        let mut storage_keys = BTreeMap::new();
        for mutation in &storage {
            mutation.validate()?;
            if storage_keys.insert(mutation.key(), ()).is_some() {
                return Err(ScriptDtoError::DuplicateId {
                    field: "plugin storage key",
                    actual_bytes: mutation.key().len(),
                });
            }
        }
        Ok(Self {
            id: validate_script_id(id.as_ref())?,
            player_id,
            inventory,
            storage,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub const fn player_id(&self) -> ScriptPlayerId {
        self.player_id
    }

    pub fn inventory(&self) -> &[ScriptInventoryResourceDelta] {
        &self.inventory
    }

    pub fn storage(&self) -> &[ScriptStorageMutation] {
        &self.storage
    }
}

/// Bounded atomic mutation of one connected player's main inventory and hotbar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPlayerInventoryTransaction {
    request_id: String,
    player_id: ScriptPlayerId,
    deltas: Vec<ScriptInventoryResourceDelta>,
}

impl ScriptPlayerInventoryTransaction {
    pub fn try_new(
        request_id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        deltas: Vec<ScriptInventoryResourceDelta>,
    ) -> Result<Self, ScriptDtoError> {
        if deltas.is_empty() {
            return Err(ScriptDtoError::EmptyTransaction);
        }
        if deltas.len() > MAX_INVENTORY_STORAGE_MUTATIONS {
            return Err(ScriptDtoError::TooManyEntries {
                field: "player inventory transaction",
                max: MAX_INVENTORY_STORAGE_MUTATIONS,
            });
        }
        let mut resource_ids = BTreeMap::new();
        for delta in &deltas {
            if resource_ids.insert(delta.resource_id(), ()).is_some() {
                return Err(ScriptDtoError::DuplicateId {
                    field: "inventory resource id",
                    actual_bytes: delta.resource_id().len(),
                });
            }
        }
        Ok(Self {
            request_id: validate_script_id(request_id.as_ref())?,
            player_id,
            deltas,
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub const fn player_id(&self) -> ScriptPlayerId {
        self.player_id
    }

    pub fn deltas(&self) -> &[ScriptInventoryResourceDelta] {
        &self.deltas
    }
}

/// Exact reason why an admitted player inventory transaction did not commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptPlayerInventoryFailure {
    PlayerUnavailable,
    RuntimeUnavailable,
    UnknownResource,
    InsufficientResource,
    InventoryFull,
}

impl ScriptPlayerInventoryFailure {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PlayerUnavailable => "player_unavailable",
            Self::RuntimeUnavailable => "runtime_unavailable",
            Self::UnknownResource => "unknown_resource",
            Self::InsufficientResource => "insufficient_resource",
            Self::InventoryFull => "inventory_full",
        }
    }
}

/// Bounded request for the server to bind one nearby villager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptVillagerBindingRequest {
    request_id: String,
    center: ScriptPosition,
    radius_bits: u64,
}

impl ScriptVillagerBindingRequest {
    pub fn try_new(
        request_id: impl AsRef<str>,
        center: ScriptPosition,
        radius: f64,
    ) -> Result<Self, ScriptDtoError> {
        if !radius.is_finite() || radius <= 0.0 || radius > MAX_VILLAGER_BINDING_RADIUS {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(Self {
            request_id: validate_script_id(request_id.as_ref())?,
            center,
            radius_bits: radius.to_bits(),
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub const fn center(&self) -> ScriptPosition {
        self.center
    }

    pub fn radius(&self) -> f64 {
        f64::from_bits(self.radius_bits)
    }
}

/// Ephemeral server-issued villager binding token. It is not an entity handle or pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptVillagerBinding {
    token: String,
    expires_at_tick: u64,
}

impl ScriptVillagerBinding {
    pub fn try_new(token: impl AsRef<str>, expires_at_tick: u64) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            token: validate_script_id(token.as_ref())?,
            expires_at_tick,
        })
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub const fn expires_at_tick(&self) -> u64 {
        self.expires_at_tick
    }
}

/// Exact engine goal requested for an opaque bound villager.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptVillagerGoal {
    Idle,
    FollowPosition {
        target: ScriptPosition,
        speed_bits: u64,
    },
}

impl ScriptVillagerGoal {
    pub const fn idle() -> Self {
        Self::Idle
    }

    pub fn follow_position(target: ScriptPosition, speed: f64) -> Result<Self, ScriptDtoError> {
        if !speed.is_finite() || speed <= 0.0 || speed > MAX_VILLAGER_GOAL_SPEED {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(Self::FollowPosition {
            target,
            speed_bits: speed.to_bits(),
        })
    }

    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::FollowPosition { .. } => "follow_position",
        }
    }

    pub const fn target(&self) -> Option<ScriptPosition> {
        match self {
            Self::Idle => None,
            Self::FollowPosition { target, .. } => Some(*target),
        }
    }

    pub fn speed(&self) -> Option<f64> {
        match self {
            Self::Idle => None,
            Self::FollowPosition { speed_bits, .. } => Some(f64::from_bits(*speed_bits)),
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::Idle => Ok(()),
            Self::FollowPosition { target, speed_bits } => {
                let speed = f64::from_bits(*speed_bits);
                ScriptPosition::try_new(target.x(), target.y(), target.z())
                    .ok_or(ScriptDtoError::InvalidBounds)?;
                if !speed.is_finite() || speed <= 0.0 || speed > MAX_VILLAGER_GOAL_SPEED {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
        }
    }
}

/// Bounded request to apply one engine goal through a server-issued villager binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptVillagerGoalRequest {
    request_id: String,
    binding_token: String,
    goal: ScriptVillagerGoal,
}

impl ScriptVillagerGoalRequest {
    pub fn try_new(
        request_id: impl AsRef<str>,
        binding_token: impl AsRef<str>,
        goal: ScriptVillagerGoal,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            request_id: validate_script_id(request_id.as_ref())?,
            binding_token: validate_script_id(binding_token.as_ref())?,
            goal,
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn binding_token(&self) -> &str {
        &self.binding_token
    }

    pub const fn goal(&self) -> &ScriptVillagerGoal {
        &self.goal
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptVillagerBindingFailure {
    NotFound,
    Busy,
}

impl ScriptVillagerBindingFailure {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Busy => "busy",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptVillagerGoalFailure {
    BindingUnavailable,
    Busy,
}

impl ScriptVillagerGoalFailure {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BindingUnavailable => "binding_unavailable",
            Self::Busy => "busy",
        }
    }
}

/// Server-normalized inventory click kind. Plugins never receive slot stacks or packet state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptInventoryClick {
    Primary,
    Secondary,
    ShiftPrimary,
    ShiftSecondary,
}

/// Closed crafting source snapshot exposed by item-crafted events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptCraftingSource {
    Inventory,
    CraftingTable,
}

impl ScriptCraftingSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inventory => "inventory",
            Self::CraftingTable => "crafting_table",
        }
    }
}

/// Closed pickup source snapshot exposed by item-picked-up events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptItemPickupSource {
    ItemEntity,
    Arrow,
}

impl ScriptItemPickupSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ItemEntity => "item_entity",
            Self::Arrow => "arrow",
        }
    }
}

/// Closed source snapshot exposed by player entity-kill events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptEntityKillSource {
    Melee,
}

impl ScriptEntityKillSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Melee => "melee",
        }
    }
}

/// Closed hand snapshot exposed by player entity-interaction events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptInteractionHand {
    MainHand,
    OffHand,
}

impl ScriptInteractionHand {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MainHand => "main_hand",
            Self::OffHand => "off_hand",
        }
    }
}

/// Closed game-mode snapshot exposed by gameplay events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptGameMode {
    Survival,
    Creative,
    Adventure,
}

impl ScriptGameMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Survival => "survival",
            Self::Creative => "creative",
            Self::Adventure => "adventure",
        }
    }
}

/// Immutable inbound event snapshots visible to script runtimes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptEvent {
    target_plugin_id: Option<String>,
    kind: ScriptEventKind,
}

impl ScriptEvent {
    /// Build a server-started event snapshot.
    pub fn server_started() -> Self {
        Self {
            target_plugin_id: None,
            kind: ScriptEventKind::ServerStarted,
        }
    }

    /// Build a server-stopping event snapshot.
    pub fn server_stopping(reason: impl AsRef<str>) -> Self {
        let reason = reason.as_ref();
        validate_bounded_nonempty(
            "server stopping reason",
            reason,
            MAX_SCRIPT_DISCONNECT_REASON_BYTES,
        )
        .expect("server-authored stopping reason must be bounded");
        Self {
            target_plugin_id: None,
            kind: ScriptEventKind::ServerStopping {
                reason: reason.to_owned(),
            },
        }
    }

    /// Build a player-joined event snapshot with server-authoritative context.
    pub fn player_joined_with_context(
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
    ) -> Self {
        context
            .validate()
            .expect("server-authored player context must be bounded");
        Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerJoined {
                player_id,
                username: context.username().to_owned(),
                context,
            },
        }
    }

    /// Build a player-left event snapshot.
    pub fn player_left(player_id: ScriptPlayerId, reason: impl AsRef<str>) -> Self {
        let reason = reason.as_ref();
        validate_bounded_nonempty(
            "player-left reason",
            reason,
            MAX_SCRIPT_DISCONNECT_REASON_BYTES,
        )
        .expect("server-authored player-left reason must be bounded");
        Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerLeft {
                player_id,
                reason: reason.to_owned(),
            },
        }
    }

    /// Build a player-chat event snapshot with server-authoritative context.
    pub fn player_chat_with_context(
        player_id: ScriptPlayerId,
        message: impl AsRef<str>,
        context: ScriptPlayerContext,
    ) -> Self {
        let message = message.as_ref();
        validate_bounded_nonempty(
            "player chat message",
            message,
            MAX_SCRIPT_CHAT_MESSAGE_BYTES,
        )
        .expect("server-authored player chat must be bounded");
        context
            .validate()
            .expect("server-authored player context must be bounded");
        Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerChat {
                player_id,
                message: message.to_owned(),
                context,
            },
        }
    }

    /// Build a reliable block-break event after the authoritative world commit.
    #[allow(clippy::too_many_arguments)]
    pub fn try_player_block_broken_with_context(
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: impl AsRef<str>,
        block_id: impl AsRef<str>,
        x: i32,
        y: i32,
        z: i32,
        game_mode: ScriptGameMode,
    ) -> Result<Self, ScriptDtoError> {
        context.validate()?;
        Ok(Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerBlockBroken {
                player_id,
                context,
                dimension: validate_contract_resource_id(dimension.as_ref())?,
                block_id: validate_contract_resource_id(block_id.as_ref())?,
                x,
                y,
                z,
                game_mode,
            },
        })
    }

    /// Build a reliable block-place event after the authoritative world commit.
    #[allow(clippy::too_many_arguments)]
    pub fn try_player_block_placed_with_context(
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: impl AsRef<str>,
        block_id: impl AsRef<str>,
        x: i32,
        y: i32,
        z: i32,
        game_mode: ScriptGameMode,
    ) -> Result<Self, ScriptDtoError> {
        context.validate()?;
        Ok(Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerBlockPlaced {
                player_id,
                context,
                dimension: validate_contract_resource_id(dimension.as_ref())?,
                block_id: validate_contract_resource_id(block_id.as_ref())?,
                x,
                y,
                z,
                game_mode,
            },
        })
    }

    /// Build a reliable item-crafted event after the authoritative inventory commit.
    #[allow(clippy::too_many_arguments)]
    pub fn try_player_item_crafted_with_context(
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: impl AsRef<str>,
        item_id: impl AsRef<str>,
        count: u64,
        craft_count: u32,
        source: ScriptCraftingSource,
        game_mode: ScriptGameMode,
    ) -> Result<Self, ScriptDtoError> {
        context.validate()?;
        if count == 0 || craft_count == 0 {
            return Err(ScriptDtoError::InvalidAmount);
        }
        Ok(Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerItemCrafted {
                player_id,
                context,
                dimension: validate_contract_resource_id(dimension.as_ref())?,
                item_id: validate_contract_resource_id(item_id.as_ref())?,
                count,
                craft_count,
                source,
                game_mode,
            },
        })
    }

    /// Build a reliable item-pickup event after the authoritative inventory commit.
    #[allow(clippy::too_many_arguments)]
    pub fn try_player_item_picked_up_with_context(
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: impl AsRef<str>,
        item_id: impl AsRef<str>,
        count: u64,
        source: ScriptItemPickupSource,
        game_mode: ScriptGameMode,
    ) -> Result<Self, ScriptDtoError> {
        context.validate()?;
        if count == 0 {
            return Err(ScriptDtoError::InvalidAmount);
        }
        Ok(Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerItemPickedUp {
                player_id,
                context,
                dimension: validate_contract_resource_id(dimension.as_ref())?,
                item_id: validate_contract_resource_id(item_id.as_ref())?,
                count,
                source,
                game_mode,
            },
        })
    }

    /// Build a reliable direct player-melee kill event after the entity commit.
    pub fn try_player_entity_killed_with_context(
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: impl AsRef<str>,
        entity_id: ScriptEntityId,
        entity_type: impl AsRef<str>,
        source: ScriptEntityKillSource,
        game_mode: ScriptGameMode,
    ) -> Result<Self, ScriptDtoError> {
        context.validate()?;
        Ok(Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerEntityKilled {
                player_id,
                context,
                dimension: validate_contract_resource_id(dimension.as_ref())?,
                entity_id,
                entity_type: validate_contract_resource_id(entity_type.as_ref())?,
                source,
                game_mode,
            },
        })
    }

    /// Build a broadcast event after an authoritative entity interaction is accepted.
    #[allow(clippy::too_many_arguments)]
    pub fn try_player_entity_interacted_with_context(
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: impl AsRef<str>,
        entity_id: ScriptEntityId,
        entity_type: impl AsRef<str>,
        hand: ScriptInteractionHand,
        secondary_action: bool,
        game_mode: ScriptGameMode,
    ) -> Result<Self, ScriptDtoError> {
        context.validate()?;
        Ok(Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerEntityInteracted {
                player_id,
                context,
                dimension: validate_contract_resource_id(dimension.as_ref())?,
                entity_id,
                entity_type: validate_contract_resource_id(entity_type.as_ref())?,
                hand,
                secondary_action,
                game_mode,
            },
        })
    }

    /// Build a reliable player-death event after the authoritative survival commit.
    pub fn try_player_died_with_context(
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: impl AsRef<str>,
        game_mode: ScriptGameMode,
    ) -> Result<Self, ScriptDtoError> {
        context.validate()?;
        Ok(Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerDied {
                player_id,
                context,
                dimension: validate_contract_resource_id(dimension.as_ref())?,
                game_mode,
            },
        })
    }

    /// Build a bounded player command event with server-authoritative context.
    pub fn try_player_command_with_context(
        target_plugin_id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        root: impl AsRef<str>,
        arguments: impl AsRef<str>,
    ) -> Result<Self, ScriptDtoError> {
        let target_plugin_id = validate_target_plugin_id(target_plugin_id.as_ref())?;
        let root = root.as_ref();
        let arguments = arguments.as_ref();
        validate_bounded_nonempty("player command root", root, MAX_PLAYER_COMMAND_ROOT_BYTES)?;
        validate_bounded_value(
            "player command arguments",
            arguments,
            MAX_SCRIPT_CHAT_MESSAGE_BYTES,
        )?;
        context.validate()?;
        Ok(Self {
            target_plugin_id: Some(target_plugin_id),
            kind: ScriptEventKind::PlayerCommand {
                player_id,
                username: context.username().to_owned(),
                root: root.to_owned(),
                arguments: arguments.to_owned(),
                context,
            },
        })
    }

    /// Build a server-tick event snapshot.
    pub fn server_tick(tick: u64) -> Self {
        Self {
            target_plugin_id: None,
            kind: ScriptEventKind::ServerTick { tick },
        }
    }

    /// Build a targeted plugin-storage read result.
    pub fn plugin_storage_get_result(
        target_plugin_id: impl AsRef<str>,
        request: &ScriptPluginStorageGetRequest,
        value: Option<String>,
        version: Option<u64>,
    ) -> Result<Self, ScriptDtoError> {
        if value.is_some() != version.is_some() {
            return Err(ScriptDtoError::InconsistentResult {
                field: "plugin storage get value/version",
            });
        }
        if let Some(value) = &value {
            validate_plugin_storage_value(value)?;
        }
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::PluginStorageGetResult {
                request_id: request.request_id().to_owned(),
                key: request.key().to_owned(),
                value,
                version,
                failure: None,
            },
        })
    }

    /// Build a targeted plugin-storage compare-and-swap result.
    pub fn plugin_storage_cas_result(
        target_plugin_id: impl AsRef<str>,
        request: &ScriptPluginStorageCompareAndSwapRequest,
        applied: bool,
        version: Option<u64>,
    ) -> Result<Self, ScriptDtoError> {
        if applied && version.is_none() {
            return Err(ScriptDtoError::InconsistentResult {
                field: "plugin storage compare-and-swap success/version",
            });
        }
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::PluginStorageCasResult {
                request_id: request.request_id().to_owned(),
                key: request.key().to_owned(),
                applied,
                version,
                failure: None,
            },
        })
    }

    /// Build a targeted plugin-storage delete result.
    pub fn plugin_storage_delete_result(
        target_plugin_id: impl AsRef<str>,
        request: &ScriptPluginStorageDeleteRequest,
        deleted: bool,
        version: Option<u64>,
    ) -> Result<Self, ScriptDtoError> {
        if deleted && version.is_none() {
            return Err(ScriptDtoError::InconsistentResult {
                field: "plugin storage delete success/version",
            });
        }
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::PluginStorageDeleteResult {
                request_id: request.request_id().to_owned(),
                key: request.key().to_owned(),
                deleted,
                version,
                failure: None,
            },
        })
    }

    /// Build a targeted click event for one server-owned inventory menu.
    pub(crate) fn inventory_menu_clicked(
        target_plugin_id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        menu: &ScriptInventoryMenu,
        slot: u8,
        click: ScriptInventoryClick,
    ) -> Result<Self, ScriptDtoError> {
        if usize::from(slot) >= MAX_INVENTORY_MENU_SLOTS {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::InventoryMenuClicked {
                player_id,
                context,
                menu_id: menu.id().to_owned(),
                slot,
                click,
            },
        })
    }

    /// Build a targeted completion event for an atomic inventory and storage request.
    pub(crate) fn inventory_storage_transaction_result(
        target_plugin_id: impl AsRef<str>,
        transaction: &ScriptInventoryStorageTransaction,
        committed: bool,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::InventoryStorageTransactionResult {
                request_id: transaction.id().to_owned(),
                committed,
            },
        })
    }

    /// Build the targeted result of one admitted player inventory transaction.
    pub(crate) fn player_inventory_transaction_result(
        target_plugin_id: impl AsRef<str>,
        transaction: &ScriptPlayerInventoryTransaction,
        failure: Option<ScriptPlayerInventoryFailure>,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::PlayerInventoryTransactionResult {
                request_id: transaction.request_id().to_owned(),
                player_id: transaction.player_id(),
                failure,
            },
        })
    }

    /// Build a targeted zone-entry snapshot owned by the plugin that registered the zone.
    pub(crate) fn player_zone_entered(
        target_plugin_id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        zone: &ScriptAxisAlignedZone,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::PlayerZoneEntered {
                player_id,
                context,
                zone_id: zone.id().to_owned(),
            },
        })
    }

    /// Build a targeted zone-exit snapshot owned by the plugin that registered the zone.
    pub(crate) fn player_zone_exited(
        target_plugin_id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        zone: &ScriptAxisAlignedZone,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::PlayerZoneExited {
                player_id,
                context,
                zone_id: zone.id().to_owned(),
            },
        })
    }

    fn zone_command_result(
        target_plugin_id: impl AsRef<str>,
        zone_id: impl AsRef<str>,
        accepted: bool,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::ZoneCommandResult {
                zone_id: validate_script_id(zone_id.as_ref())?.to_owned(),
                accepted,
            },
        })
    }

    /// Build the targeted result of one admitted same-dimension player teleport.
    pub(crate) fn player_teleport_result(
        target_plugin_id: impl AsRef<str>,
        request: &ScriptPlayerTeleportRequest,
        failure: Option<ScriptPlayerTeleportFailure>,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::PlayerTeleportResult {
                request_id: request.request_id().to_owned(),
                player_id: request.player_id(),
                position: request.position(),
                failure,
            },
        })
    }

    /// Build the targeted result of one admitted connected-player query.
    pub(crate) fn online_players_result(
        target_plugin_id: impl AsRef<str>,
        request: &ScriptOnlinePlayersRequest,
        players: Vec<ScriptOnlinePlayerSnapshot>,
        truncated: bool,
    ) -> Result<Self, ScriptDtoError> {
        if players.len() > request.limit() {
            return Err(ScriptDtoError::TooManyEntries {
                field: "online player snapshots",
                max: request.limit(),
            });
        }
        for player in &players {
            player.validate()?;
        }
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::OnlinePlayersResult {
                request_id: request.request_id().to_owned(),
                players,
                truncated,
            },
        })
    }

    /// Build a targeted ephemeral villager-binding result without exposing an entity reference.
    pub(crate) fn villager_binding_result(
        target_plugin_id: impl AsRef<str>,
        request: &ScriptVillagerBindingRequest,
        binding: Option<ScriptVillagerBinding>,
        failure: Option<ScriptVillagerBindingFailure>,
    ) -> Result<Self, ScriptDtoError> {
        if binding.is_some() == failure.is_some() {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::VillagerBindingResult {
                request_id: request.request_id().to_owned(),
                binding,
                failure,
            },
        })
    }

    /// Build a targeted result for one admitted bound-villager goal.
    pub(crate) fn villager_goal_result(
        target_plugin_id: impl AsRef<str>,
        request: &ScriptVillagerGoalRequest,
        failure: Option<ScriptVillagerGoalFailure>,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            target_plugin_id: Some(validate_target_plugin_id(target_plugin_id.as_ref())?),
            kind: ScriptEventKind::VillagerGoalResult {
                request_id: request.request_id().to_owned(),
                goal: request.goal().clone(),
                failure,
            },
        })
    }

    /// Build one client-originated Loader interaction targeted to its bundle owner.
    pub fn loader_interaction(
        target_plugin_id: impl AsRef<str>,
        player_id: ScriptPlayerId,
        interaction_id: impl AsRef<str>,
        payload: impl AsRef<str>,
    ) -> Result<Self, ScriptDtoError> {
        let target_plugin_id = validate_target_plugin_id(target_plugin_id.as_ref())?;
        let interaction_id = validate_contract_resource_id(interaction_id.as_ref())?;
        if !interaction_id
            .strip_prefix(&target_plugin_id)
            .is_some_and(|suffix| suffix.starts_with(':') && suffix.len() > 1)
        {
            return Err(ScriptDtoError::InvalidResourceId {
                field: "Loader interaction id",
                actual_bytes: interaction_id.len(),
            });
        }
        let payload = payload.as_ref();
        validate_bounded_value(
            "Loader interaction payload",
            payload,
            MAX_SCRIPT_LOADER_INTERACTION_PAYLOAD_BYTES,
        )?;
        Ok(Self {
            target_plugin_id: Some(target_plugin_id),
            kind: ScriptEventKind::LoaderInteraction {
                player_id,
                interaction_id: interaction_id.to_owned(),
                payload: payload.to_owned(),
            },
        })
    }

    /// Return the plugin id for an event that must not be broadcast to other runtimes.
    pub fn target_plugin_id(&self) -> Option<&str> {
        self.target_plugin_id.as_deref()
    }

    /// Return the immutable event kind.
    pub fn kind(&self) -> &ScriptEventKind {
        &self.kind
    }

    /// Return the stable manifest subscription name for this event.
    pub fn event_name(&self) -> &'static str {
        match self.kind {
            ScriptEventKind::ServerStarted => "server.started",
            ScriptEventKind::ServerStopping { .. } => "server.stopping",
            ScriptEventKind::PlayerJoined { .. } => "player.joined",
            ScriptEventKind::PlayerLeft { .. } => "player.left",
            ScriptEventKind::PlayerChat { .. } => "player.chat",
            ScriptEventKind::PlayerBlockBroken { .. } => "player.block_broken",
            ScriptEventKind::PlayerBlockPlaced { .. } => "player.block_placed",
            ScriptEventKind::PlayerItemCrafted { .. } => "player.item_crafted",
            ScriptEventKind::PlayerItemPickedUp { .. } => "player.item_picked_up",
            ScriptEventKind::PlayerEntityKilled { .. } => "player.entity_killed",
            ScriptEventKind::PlayerEntityInteracted { .. } => "player.entity_interacted",
            ScriptEventKind::PlayerDied { .. } => "player.died",
            ScriptEventKind::PlayerCommand { .. } => "player.command",
            ScriptEventKind::ServerTick { .. } => "server.tick",
            ScriptEventKind::PluginStorageGetResult { .. } => "plugin.storage.get_result",
            ScriptEventKind::PluginStorageCasResult { .. } => "plugin.storage.cas_result",
            ScriptEventKind::PluginStorageDeleteResult { .. } => "plugin.storage.delete_result",
            ScriptEventKind::InventoryMenuClicked { .. } => "inventory.menu.clicked",
            ScriptEventKind::InventoryStorageTransactionResult { .. } => {
                "inventory.storage_transaction.result"
            }
            ScriptEventKind::PlayerInventoryTransactionResult { .. } => {
                "player.inventory_transaction_result"
            }
            ScriptEventKind::PlayerZoneEntered { .. } => "player.zone_entered",
            ScriptEventKind::PlayerZoneExited { .. } => "player.zone_exited",
            ScriptEventKind::ZoneCommandResult { .. } => "zone.command_result",
            ScriptEventKind::PlayerTeleportResult { .. } => "player.teleport_result",
            ScriptEventKind::OnlinePlayersResult { .. } => "player.online_result",
            ScriptEventKind::VillagerBindingResult { .. } => "villager.binding_result",
            ScriptEventKind::VillagerGoalResult { .. } => "villager.goal_result",
            ScriptEventKind::LoaderInteraction { .. } => "loader.interaction",
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if let Some(plugin_id) = &self.target_plugin_id {
            validate_target_plugin_id(plugin_id)?;
        }
        match &self.kind {
            ScriptEventKind::ServerStarted | ScriptEventKind::ServerTick { .. } => Ok(()),
            ScriptEventKind::ServerStopping { reason }
            | ScriptEventKind::PlayerLeft { reason, .. } => validate_bounded_nonempty(
                "event reason",
                reason,
                MAX_SCRIPT_DISCONNECT_REASON_BYTES,
            ),
            ScriptEventKind::PlayerJoined {
                username, context, ..
            } => {
                validate_bounded_nonempty(
                    "event username",
                    username,
                    MAX_SCRIPT_PLAYER_NAME_BYTES,
                )?;
                context.validate()
            }
            ScriptEventKind::PlayerChat {
                message, context, ..
            } => {
                validate_bounded_nonempty(
                    "event chat message",
                    message,
                    MAX_SCRIPT_CHAT_MESSAGE_BYTES,
                )?;
                context.validate()
            }
            ScriptEventKind::PlayerBlockBroken {
                context,
                dimension,
                block_id,
                ..
            }
            | ScriptEventKind::PlayerBlockPlaced {
                context,
                dimension,
                block_id,
                ..
            } => {
                context.validate()?;
                validate_contract_resource_id(dimension)?;
                validate_contract_resource_id(block_id).map(drop)
            }
            ScriptEventKind::PlayerItemCrafted {
                context,
                dimension,
                item_id,
                count,
                craft_count,
                ..
            } => {
                context.validate()?;
                validate_contract_resource_id(dimension)?;
                validate_contract_resource_id(item_id)?;
                if *count == 0 || *craft_count == 0 {
                    return Err(ScriptDtoError::InvalidAmount);
                }
                Ok(())
            }
            ScriptEventKind::PlayerItemPickedUp {
                context,
                dimension,
                item_id,
                count,
                ..
            } => {
                context.validate()?;
                validate_contract_resource_id(dimension)?;
                validate_contract_resource_id(item_id)?;
                if *count == 0 {
                    return Err(ScriptDtoError::InvalidAmount);
                }
                Ok(())
            }
            ScriptEventKind::PlayerEntityKilled {
                context,
                dimension,
                entity_type,
                ..
            }
            | ScriptEventKind::PlayerEntityInteracted {
                context,
                dimension,
                entity_type,
                ..
            } => {
                context.validate()?;
                validate_contract_resource_id(dimension)?;
                validate_contract_resource_id(entity_type).map(drop)
            }
            ScriptEventKind::PlayerDied {
                context, dimension, ..
            } => {
                context.validate()?;
                validate_contract_resource_id(dimension)?;
                Ok(())
            }
            ScriptEventKind::PlayerCommand {
                username,
                root,
                arguments,
                context,
                ..
            } => {
                validate_bounded_nonempty(
                    "event username",
                    username,
                    MAX_SCRIPT_PLAYER_NAME_BYTES,
                )?;
                validate_bounded_nonempty(
                    "event command root",
                    root,
                    MAX_PLAYER_COMMAND_ROOT_BYTES,
                )?;
                validate_bounded_value(
                    "event command arguments",
                    arguments,
                    MAX_SCRIPT_CHAT_MESSAGE_BYTES,
                )?;
                context.validate()
            }
            ScriptEventKind::PluginStorageGetResult {
                request_id,
                key,
                value,
                version,
                failure,
            } => {
                validate_script_id(request_id)?;
                validate_plugin_storage_key(key)?;
                if value.is_some() != version.is_some() {
                    return Err(ScriptDtoError::InconsistentResult {
                        field: "plugin storage get value/version",
                    });
                }
                if let Some(value) = value {
                    validate_plugin_storage_value(value)?;
                }
                if failure.is_some() && (value.is_some() || version.is_some()) {
                    return Err(ScriptDtoError::InconsistentResult {
                        field: "plugin storage get failure/result",
                    });
                }
                Ok(())
            }
            ScriptEventKind::PluginStorageCasResult {
                request_id,
                key,
                applied,
                version,
                failure,
            }
            | ScriptEventKind::PluginStorageDeleteResult {
                request_id,
                key,
                deleted: applied,
                version,
                failure,
            } => {
                validate_script_id(request_id)?;
                validate_plugin_storage_key(key)?;
                if *applied && version.is_none() {
                    return Err(ScriptDtoError::InconsistentResult {
                        field: "plugin storage mutation success/version",
                    });
                }
                if failure.is_some() && (*applied || version.is_some()) {
                    return Err(ScriptDtoError::InconsistentResult {
                        field: "plugin storage mutation failure/result",
                    });
                }
                Ok(())
            }
            ScriptEventKind::InventoryMenuClicked {
                context,
                menu_id,
                slot,
                ..
            } => {
                context.validate()?;
                validate_script_id(menu_id)?;
                if usize::from(*slot) >= MAX_INVENTORY_MENU_SLOTS {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            ScriptEventKind::InventoryStorageTransactionResult { request_id, .. } => {
                validate_script_id(request_id).map(drop)
            }
            ScriptEventKind::PlayerZoneEntered {
                context, zone_id, ..
            }
            | ScriptEventKind::PlayerZoneExited {
                context, zone_id, ..
            } => {
                context.validate()?;
                validate_script_id(zone_id).map(drop)
            }
            ScriptEventKind::ZoneCommandResult { zone_id, .. } => {
                validate_script_id(zone_id).map(drop)
            }
            ScriptEventKind::PlayerInventoryTransactionResult { request_id, .. }
            | ScriptEventKind::PlayerTeleportResult { request_id, .. } => {
                validate_script_id(request_id).map(drop)
            }
            ScriptEventKind::OnlinePlayersResult {
                request_id,
                players,
                ..
            } => {
                validate_script_id(request_id)?;
                if players.len() > MAX_ONLINE_PLAYER_QUERY_LIMIT {
                    return Err(ScriptDtoError::TooManyEntries {
                        field: "online player snapshots",
                        max: MAX_ONLINE_PLAYER_QUERY_LIMIT,
                    });
                }
                for player in players {
                    player.validate()?;
                }
                Ok(())
            }
            ScriptEventKind::VillagerBindingResult {
                request_id,
                binding,
                failure,
            } => {
                validate_script_id(request_id)?;
                if binding.is_some() == failure.is_some() {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                if let Some(binding) = binding {
                    validate_script_id(binding.token())?;
                }
                Ok(())
            }
            ScriptEventKind::VillagerGoalResult {
                request_id, goal, ..
            } => {
                validate_script_id(request_id)?;
                goal.validate()
            }
            ScriptEventKind::LoaderInteraction {
                interaction_id,
                payload,
                ..
            } => {
                validate_contract_resource_id(interaction_id)?;
                validate_bounded_value(
                    "Loader interaction payload",
                    payload,
                    MAX_SCRIPT_LOADER_INTERACTION_PAYLOAD_BYTES,
                )
            }
        }
    }
}

/// Script-visible event variants.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptEventKind {
    ServerStarted,
    ServerStopping {
        reason: String,
    },
    PlayerJoined {
        player_id: ScriptPlayerId,
        username: String,
        context: ScriptPlayerContext,
    },
    PlayerLeft {
        player_id: ScriptPlayerId,
        reason: String,
    },
    PlayerChat {
        player_id: ScriptPlayerId,
        message: String,
        context: ScriptPlayerContext,
    },
    PlayerBlockBroken {
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: String,
        block_id: String,
        x: i32,
        y: i32,
        z: i32,
        game_mode: ScriptGameMode,
    },
    PlayerBlockPlaced {
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: String,
        block_id: String,
        x: i32,
        y: i32,
        z: i32,
        game_mode: ScriptGameMode,
    },
    PlayerItemCrafted {
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: String,
        item_id: String,
        count: u64,
        craft_count: u32,
        source: ScriptCraftingSource,
        game_mode: ScriptGameMode,
    },
    PlayerItemPickedUp {
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: String,
        item_id: String,
        count: u64,
        source: ScriptItemPickupSource,
        game_mode: ScriptGameMode,
    },
    PlayerEntityKilled {
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: String,
        entity_id: ScriptEntityId,
        entity_type: String,
        source: ScriptEntityKillSource,
        game_mode: ScriptGameMode,
    },
    PlayerEntityInteracted {
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: String,
        entity_id: ScriptEntityId,
        entity_type: String,
        hand: ScriptInteractionHand,
        secondary_action: bool,
        game_mode: ScriptGameMode,
    },
    PlayerDied {
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        dimension: String,
        game_mode: ScriptGameMode,
    },
    PlayerCommand {
        player_id: ScriptPlayerId,
        username: String,
        root: String,
        arguments: String,
        context: ScriptPlayerContext,
    },
    ServerTick {
        tick: u64,
    },
    PluginStorageGetResult {
        request_id: String,
        key: String,
        value: Option<String>,
        version: Option<u64>,
        failure: Option<ScriptPluginStorageFailure>,
    },
    PluginStorageCasResult {
        request_id: String,
        key: String,
        applied: bool,
        version: Option<u64>,
        failure: Option<ScriptPluginStorageFailure>,
    },
    PluginStorageDeleteResult {
        request_id: String,
        key: String,
        deleted: bool,
        version: Option<u64>,
        failure: Option<ScriptPluginStorageFailure>,
    },
    InventoryMenuClicked {
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        menu_id: String,
        slot: u8,
        click: ScriptInventoryClick,
    },
    InventoryStorageTransactionResult {
        request_id: String,
        committed: bool,
    },
    PlayerInventoryTransactionResult {
        request_id: String,
        player_id: ScriptPlayerId,
        failure: Option<ScriptPlayerInventoryFailure>,
    },
    PlayerZoneEntered {
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        zone_id: String,
    },
    PlayerZoneExited {
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        zone_id: String,
    },
    ZoneCommandResult {
        zone_id: String,
        accepted: bool,
    },
    PlayerTeleportResult {
        request_id: String,
        player_id: ScriptPlayerId,
        position: ScriptPosition,
        failure: Option<ScriptPlayerTeleportFailure>,
    },
    OnlinePlayersResult {
        request_id: String,
        players: Vec<ScriptOnlinePlayerSnapshot>,
        truncated: bool,
    },
    VillagerBindingResult {
        request_id: String,
        binding: Option<ScriptVillagerBinding>,
        failure: Option<ScriptVillagerBindingFailure>,
    },
    VillagerGoalResult {
        request_id: String,
        goal: ScriptVillagerGoal,
        failure: Option<ScriptVillagerGoalFailure>,
    },
    LoaderInteraction {
        player_id: ScriptPlayerId,
        interaction_id: String,
        payload: String,
    },
}

/// Outbound command requests emitted by script code.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptCommand {
    /// A request emitted by a Lua VM and attested by its host. Lua has no API for
    /// constructing `ScriptCommandProvenance` or selecting another plugin id.
    HostAttached {
        provenance: ScriptCommandProvenance,
        request: Arc<ScriptCommand>,
    },
    SendChatMessage {
        player_id: ScriptPlayerId,
        message: String,
    },
    BroadcastChatMessage {
        message: String,
    },
    DisconnectPlayer {
        player_id: ScriptPlayerId,
        reason: String,
    },
    RunConsoleCommand {
        command: String,
    },
    SpawnEntity {
        actor: ScriptPlayerId,
        entity_type: String,
        position: ScriptPosition,
    },
    PluginStorageGet {
        request: ScriptPluginStorageGetRequest,
    },
    PluginStorageCompareAndSwap {
        request: ScriptPluginStorageCompareAndSwapRequest,
    },
    PluginStorageDelete {
        request: ScriptPluginStorageDeleteRequest,
    },
    OpenInventoryMenu {
        player_id: ScriptPlayerId,
        menu: ScriptInventoryMenu,
    },
    OpenClientScreen {
        player_id: ScriptPlayerId,
        screen_id: String,
    },
    PlaceLoaderBlock {
        block_id: String,
        x: i32,
        y: i32,
        z: i32,
    },
    GrantLoaderBlockItem {
        player_id: ScriptPlayerId,
        block_id: String,
        count: u8,
    },
    CloseInventoryMenu {
        player_id: ScriptPlayerId,
        menu_id: String,
    },
    InventoryStorageTransaction {
        transaction: ScriptInventoryStorageTransaction,
    },
    PlayerInventoryTransaction {
        transaction: ScriptPlayerInventoryTransaction,
    },
    UpsertZone {
        zone: ScriptAxisAlignedZone,
    },
    RemoveZone {
        zone_id: String,
    },
    RequestVillagerBinding {
        request: ScriptVillagerBindingRequest,
    },
    SetVillagerGoal {
        request: ScriptVillagerGoalRequest,
    },
    TeleportPlayer {
        request: ScriptPlayerTeleportRequest,
    },
    ListOnlinePlayers {
        request: ScriptOnlinePlayersRequest,
    },
}

/// Origin attached by the Lua host after a handler returns its bounded commands.
///
/// The constructor is crate-private so scripts and external adapters cannot
/// fabricate an origin. Adapters may inspect the id to route a completion event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptCommandProvenance {
    plugin_id: Arc<str>,
    nonce: u64,
}

impl ScriptCommandProvenance {
    #[cfg(any(test, feature = "lua-runtime"))]
    fn for_host_plugin(plugin_id: Arc<str>, nonce: u64) -> Self {
        Self { plugin_id, nonce }
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }
}

#[derive(Debug)]
struct HostAdmissionRecord {
    plugin_id: Arc<str>,
    request: Weak<ScriptCommand>,
}

#[derive(Debug, Default)]
struct HostAdmissionLedger {
    #[cfg(any(test, feature = "lua-runtime"))]
    next_nonce: AtomicU64,
    pending: StdMutex<BTreeMap<u64, HostAdmissionRecord>>,
}

impl HostAdmissionLedger {
    #[cfg(any(test, feature = "lua-runtime"))]
    fn issue(
        &self,
        plugin_id: Arc<str>,
        batch: CommandBatch,
    ) -> Result<Vec<ScriptCommand>, CommandBatch> {
        let mut pending = match self.pending.lock() {
            Ok(pending) => pending,
            Err(mut poisoned) => {
                poisoned.get_mut().clear();
                return Err(batch);
            }
        };
        if pending.len().saturating_add(batch.commands.len()) > MAX_SCRIPT_COMMAND_QUEUE_CAPACITY {
            return Err(batch);
        }
        let count = match u64::try_from(batch.commands.len()) {
            Ok(count) => count,
            Err(_) => return Err(batch),
        };
        let first =
            match self
                .next_nonce
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    value.checked_add(count)
                }) {
                Ok(first) => first,
                Err(_) => return Err(batch),
            };
        let mut attached = Vec::with_capacity(batch.commands.len());
        for (offset, request) in (1_u64..=count).zip(batch.commands) {
            let nonce = first + offset;
            let request = Arc::new(request);
            pending.insert(
                nonce,
                HostAdmissionRecord {
                    plugin_id: Arc::clone(&plugin_id),
                    request: Arc::downgrade(&request),
                },
            );
            attached.push(ScriptCommand::HostAttached {
                provenance: ScriptCommandProvenance::for_host_plugin(Arc::clone(&plugin_id), nonce),
                request,
            });
        }
        Ok(attached)
    }

    fn accept(
        &self,
        provenance: ScriptCommandProvenance,
        request: Arc<ScriptCommand>,
    ) -> Result<AdmittedScriptCommand, ScriptCommandAcceptanceError> {
        let mut pending = match self.pending.lock() {
            Ok(pending) => pending,
            Err(mut poisoned) => {
                poisoned.get_mut().clear();
                return Err(ScriptCommandAcceptanceError::AuthorityPoisoned);
            }
        };
        let Some(record) = pending.remove(&provenance.nonce) else {
            return Err(ScriptCommandAcceptanceError::UnknownOrConsumed);
        };
        if record.plugin_id != provenance.plugin_id
            || !record.request.ptr_eq(&Arc::downgrade(&request))
        {
            return Err(ScriptCommandAcceptanceError::RequestMismatch);
        }
        Ok(AdmittedScriptCommand {
            plugin_id: record.plugin_id,
            request,
        })
    }
}

/// One exact host-attested plugin request accepted by the server boundary.
///
/// The value is intentionally not cloneable. A router must accept the raw
/// `HostAttached` command through `ScriptBoundary::accept_host_command` before
/// dispatching any plugin-owned side effect.
#[derive(Debug, PartialEq, Eq)]
pub struct AdmittedScriptCommand {
    plugin_id: Arc<str>,
    request: Arc<ScriptCommand>,
}

impl AdmittedScriptCommand {
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub fn request(&self) -> &ScriptCommand {
        &self.request
    }

    pub fn into_request(self) -> ScriptCommand {
        Arc::try_unwrap(self.request).unwrap_or_else(|request| request.as_ref().clone())
    }

    pub fn into_open_inventory_menu(
        self,
    ) -> Result<(ScriptPluginTarget, ScriptPlayerId, ScriptInventoryMenu), ScriptDtoError> {
        let ScriptCommand::OpenInventoryMenu { player_id, menu } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "inventory menu admission",
            });
        };
        Ok((
            ScriptPluginTarget {
                plugin_id: self.plugin_id,
            },
            *player_id,
            menu.clone(),
        ))
    }

    pub fn into_open_client_screen(
        self,
    ) -> Result<(ScriptPluginTarget, ScriptPlayerId, String), ScriptDtoError> {
        let ScriptCommand::OpenClientScreen {
            player_id,
            screen_id,
        } = self.request.as_ref()
        else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "client screen admission",
            });
        };
        Ok((
            ScriptPluginTarget {
                plugin_id: self.plugin_id,
            },
            *player_id,
            screen_id.clone(),
        ))
    }

    pub fn into_upsert_zone(
        self,
    ) -> Result<(ScriptPluginTarget, ScriptAxisAlignedZone), ScriptDtoError> {
        let ScriptCommand::UpsertZone { zone } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "zone admission",
            });
        };
        Ok((
            ScriptPluginTarget {
                plugin_id: self.plugin_id,
            },
            zone.clone(),
        ))
    }

    pub fn into_remove_zone(self) -> Result<(ScriptPluginTarget, String), ScriptDtoError> {
        let ScriptCommand::RemoveZone { zone_id } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "zone removal admission",
            });
        };
        Ok((
            ScriptPluginTarget {
                plugin_id: self.plugin_id,
            },
            zone_id.clone(),
        ))
    }

    pub fn plugin_storage_get_result(
        self,
        value: Option<&str>,
        version: Option<u64>,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::PluginStorageGet { request } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "plugin storage get admission",
            });
        };
        if let Some(value) = value {
            validate_plugin_storage_value(value)?;
        }
        ScriptEvent::plugin_storage_get_result(
            &self.plugin_id,
            request,
            value.map(str::to_owned),
            version,
        )
    }

    pub fn plugin_storage_cas_result(
        self,
        applied: bool,
        version: Option<u64>,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::PluginStorageCompareAndSwap { request } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "plugin storage compare-and-swap admission",
            });
        };
        ScriptEvent::plugin_storage_cas_result(&self.plugin_id, request, applied, version)
    }

    pub fn plugin_storage_delete_result(
        self,
        deleted: bool,
        version: Option<u64>,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::PluginStorageDelete { request } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "plugin storage delete admission",
            });
        };
        ScriptEvent::plugin_storage_delete_result(&self.plugin_id, request, deleted, version)
    }

    pub fn plugin_storage_failure_result(
        self,
        failure: ScriptPluginStorageFailure,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let target_plugin_id = validate_target_plugin_id(&self.plugin_id)?;
        let kind = match self.request.as_ref() {
            ScriptCommand::PluginStorageGet { request } => {
                ScriptEventKind::PluginStorageGetResult {
                    request_id: request.request_id().to_owned(),
                    key: request.key().to_owned(),
                    value: None,
                    version: None,
                    failure: Some(failure),
                }
            }
            ScriptCommand::PluginStorageCompareAndSwap { request } => {
                ScriptEventKind::PluginStorageCasResult {
                    request_id: request.request_id().to_owned(),
                    key: request.key().to_owned(),
                    applied: false,
                    version: None,
                    failure: Some(failure),
                }
            }
            ScriptCommand::PluginStorageDelete { request } => {
                ScriptEventKind::PluginStorageDeleteResult {
                    request_id: request.request_id().to_owned(),
                    key: request.key().to_owned(),
                    deleted: false,
                    version: None,
                    failure: Some(failure),
                }
            }
            _ => {
                return Err(ScriptDtoError::InconsistentResult {
                    field: "plugin storage failure admission",
                });
            }
        };
        Ok(ScriptEvent {
            target_plugin_id: Some(target_plugin_id),
            kind,
        })
    }

    pub fn inventory_storage_transaction_result(
        self,
        committed: bool,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::InventoryStorageTransaction { transaction } = self.request.as_ref()
        else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "inventory-storage transaction admission",
            });
        };
        ScriptEvent::inventory_storage_transaction_result(&self.plugin_id, transaction, committed)
    }

    pub fn player_inventory_transaction_result(
        self,
        failure: Option<ScriptPlayerInventoryFailure>,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::PlayerInventoryTransaction { transaction } = self.request.as_ref()
        else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "player inventory transaction admission",
            });
        };
        ScriptEvent::player_inventory_transaction_result(&self.plugin_id, transaction, failure)
    }

    pub fn villager_binding_result(
        self,
        binding: Option<ScriptVillagerBinding>,
        failure: Option<ScriptVillagerBindingFailure>,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::RequestVillagerBinding { request } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "villager binding admission",
            });
        };
        ScriptEvent::villager_binding_result(&self.plugin_id, request, binding, failure)
    }

    pub fn villager_goal_result(
        self,
        failure: Option<ScriptVillagerGoalFailure>,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::SetVillagerGoal { request } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "villager goal admission",
            });
        };
        ScriptEvent::villager_goal_result(&self.plugin_id, request, failure)
    }

    pub fn player_teleport_result(
        self,
        failure: Option<ScriptPlayerTeleportFailure>,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::TeleportPlayer { request } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "player teleport admission",
            });
        };
        ScriptEvent::player_teleport_result(&self.plugin_id, request, failure)
    }

    pub fn online_players_result(
        self,
        players: Vec<ScriptOnlinePlayerSnapshot>,
        truncated: bool,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::ListOnlinePlayers { request } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "online players admission",
            });
        };
        ScriptEvent::online_players_result(&self.plugin_id, request, players, truncated)
    }
}

/// Opaque plugin target retained by a production adapter after accepting an
/// owning menu or zone command. External code cannot fabricate a plugin id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPluginTarget {
    plugin_id: Arc<str>,
}

impl ScriptPluginTarget {
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub fn inventory_menu_clicked(
        &self,
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        menu: &ScriptInventoryMenu,
        slot: u8,
        click: ScriptInventoryClick,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        ScriptEvent::inventory_menu_clicked(&self.plugin_id, player_id, context, menu, slot, click)
    }

    pub fn player_zone_entered(
        &self,
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        zone: &ScriptAxisAlignedZone,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        ScriptEvent::player_zone_entered(&self.plugin_id, player_id, context, zone)
    }

    pub fn player_zone_exited(
        &self,
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        zone: &ScriptAxisAlignedZone,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        ScriptEvent::player_zone_exited(&self.plugin_id, player_id, context, zone)
    }

    pub fn zone_command_result(
        &self,
        zone_id: impl AsRef<str>,
        accepted: bool,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        ScriptEvent::zone_command_result(&self.plugin_id, zone_id, accepted)
    }
}

#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptCommandAcceptanceError {
    NotHostAttached,
    UnknownOrConsumed,
    RequestMismatch,
    AuthorityPoisoned,
}

impl ScriptCommand {
    /// Return the host capability required before admitting this command.
    pub fn required_capability_kind(&self) -> Option<ScriptCommandCapabilityKind> {
        self.required_capability()
            .map(RequiredCommandCapability::kind)
    }

    fn required_capability(&self) -> Option<RequiredCommandCapability<'_>> {
        match self {
            Self::HostAttached { request, .. } => request.required_capability(),
            Self::SendChatMessage { .. }
            | Self::BroadcastChatMessage { .. }
            | Self::DisconnectPlayer { .. }
            | Self::OpenClientScreen { .. }
            | Self::PlaceLoaderBlock { .. }
            | Self::GrantLoaderBlockItem { .. } => None,
            Self::RunConsoleCommand { command } => {
                Some(RequiredCommandCapability::RunConsoleCommandRoot {
                    root: console_command_root(command),
                })
            }
            Self::SpawnEntity { entity_type, .. } => {
                Some(RequiredCommandCapability::SpawnEntityType { entity_type })
            }
            Self::PluginStorageGet { .. }
            | Self::PluginStorageCompareAndSwap { .. }
            | Self::PluginStorageDelete { .. } => Some(RequiredCommandCapability::PluginStorage),
            Self::OpenInventoryMenu { .. } | Self::CloseInventoryMenu { .. } => {
                Some(RequiredCommandCapability::InventoryMenus)
            }
            Self::InventoryStorageTransaction { .. } => {
                Some(RequiredCommandCapability::InventoryStorageTransactions)
            }
            Self::PlayerInventoryTransaction { .. } => {
                Some(RequiredCommandCapability::PlayerInventory)
            }
            Self::UpsertZone { .. } | Self::RemoveZone { .. } => {
                Some(RequiredCommandCapability::Zones)
            }
            Self::RequestVillagerBinding { .. } | Self::SetVillagerGoal { .. } => {
                Some(RequiredCommandCapability::Villagers)
            }
            Self::TeleportPlayer { .. } => Some(RequiredCommandCapability::PlayerTeleport),
            Self::ListOnlinePlayers { .. } => Some(RequiredCommandCapability::PlayerQueries),
        }
    }

    fn validate_contract(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::HostAttached { .. } => Err(ScriptDtoError::InconsistentResult {
                field: "nested host-attached command",
            }),
            Self::SendChatMessage { message, .. } | Self::BroadcastChatMessage { message } => {
                validate_bounded_nonempty("chat message", message, MAX_SCRIPT_CHAT_MESSAGE_BYTES)
            }
            Self::DisconnectPlayer { reason, .. } => validate_bounded_nonempty(
                "disconnect reason",
                reason,
                MAX_SCRIPT_DISCONNECT_REASON_BYTES,
            ),
            Self::RunConsoleCommand { command } => {
                validate_bounded_nonempty(
                    "console command",
                    command,
                    MAX_SCRIPT_CONSOLE_COMMAND_BYTES,
                )?;
                let root = console_command_root(command);
                if root.is_empty() || root.len() > MAX_PLAYER_COMMAND_ROOT_BYTES {
                    return Err(ScriptDtoError::InvalidId {
                        field: "console command root",
                        actual_bytes: root.len(),
                    });
                }
                Ok(())
            }
            Self::SpawnEntity {
                entity_type,
                position,
                ..
            } => {
                validate_contract_resource_id(entity_type)?;
                ScriptPosition::try_new(position.x(), position.y(), position.z())
                    .ok_or(ScriptDtoError::InvalidBounds)
                    .map(drop)
            }
            Self::OpenInventoryMenu { menu, .. } => {
                ScriptInventoryMenu::try_new(menu.id(), menu.title(), menu.slots().to_vec())
                    .map(drop)
            }
            Self::OpenClientScreen { screen_id, .. } => {
                validate_contract_resource_id(screen_id).map(drop)
            }
            Self::PlaceLoaderBlock { block_id, .. } => {
                validate_contract_resource_id(block_id).map(drop)
            }
            Self::GrantLoaderBlockItem {
                block_id, count, ..
            } => {
                validate_contract_resource_id(block_id)?;
                if !(1..=64).contains(count) {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::UpsertZone { zone } => ScriptAxisAlignedZone::try_new_with_protection(
                zone.id(),
                zone.dimension(),
                zone.minimum(),
                zone.maximum(),
                zone.protection().cloned(),
            )
            .map(drop),
            Self::PluginStorageGet { request } => {
                ScriptPluginStorageGetRequest::try_new(request.request_id(), request.key())
                    .map(drop)
            }
            Self::PluginStorageCompareAndSwap { request } => {
                ScriptPluginStorageCompareAndSwapRequest::try_new(
                    request.request_id(),
                    request.key(),
                    request.expected_version(),
                    request.value(),
                )
                .map(drop)
            }
            Self::PluginStorageDelete { request } => ScriptPluginStorageDeleteRequest::try_new(
                request.request_id(),
                request.key(),
                request.expected_version(),
            )
            .map(drop),
            Self::CloseInventoryMenu { menu_id, .. } => validate_script_id(menu_id).map(drop),
            Self::InventoryStorageTransaction { transaction } => {
                ScriptInventoryStorageTransaction::try_new(
                    transaction.id(),
                    transaction.player_id(),
                    transaction.inventory().to_vec(),
                    transaction.storage().to_vec(),
                )
                .map(drop)
            }
            Self::PlayerInventoryTransaction { transaction } => {
                ScriptPlayerInventoryTransaction::try_new(
                    transaction.request_id(),
                    transaction.player_id(),
                    transaction.deltas().to_vec(),
                )
                .map(drop)
            }
            Self::RemoveZone { zone_id } => validate_script_id(zone_id).map(drop),
            Self::RequestVillagerBinding { request } => ScriptVillagerBindingRequest::try_new(
                request.request_id(),
                request.center(),
                request.radius(),
            )
            .map(drop),
            Self::SetVillagerGoal { request } => ScriptVillagerGoalRequest::try_new(
                request.request_id(),
                request.binding_token(),
                request.goal().clone(),
            )
            .and_then(|request| request.goal().validate())
            .map(drop),
            Self::TeleportPlayer { request } => ScriptPlayerTeleportRequest::try_new(
                request.request_id(),
                request.player_id(),
                request.position(),
            )
            .map(drop),
            Self::ListOnlinePlayers { request } => {
                ScriptOnlinePlayersRequest::try_new(request.request_id(), request.limit()).map(drop)
            }
        }
    }
}

#[derive(Debug, Clone)]
#[cfg(any(test, feature = "lua-runtime"))]
pub(crate) struct HostCommandAdmission {
    plugin_id: Arc<str>,
    capabilities: Arc<CommandCapabilities>,
}

#[cfg(any(test, feature = "lua-runtime"))]
impl HostCommandAdmission {
    pub(crate) fn from_manifest(manifest: &ValidatedScriptPluginManifest) -> Self {
        Self {
            plugin_id: Arc::from(manifest.plugin_id()),
            capabilities: Arc::new(manifest.to_command_capabilities()),
        }
    }
}

/// Bounded state returned when the script event queue cannot accept an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptQueueError {
    Full,
    Closed,
}

/// Rejection returned by the public raw-command submission path.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptCommandSubmissionError {
    ProvenanceRejected,
    PermissionDenied {
        capability: ScriptCommandCapabilityKind,
    },
    InvalidCommand {
        error: ScriptDtoError,
    },
    QueueFull,
    QueueClosed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(any(test, feature = "lua-runtime"))]
pub(crate) enum ScriptBatchSubmissionError {
    Full(CommandBatch),
    Closed(CommandBatch),
    Rejected {
        batch: CommandBatch,
        error: CommandBatchError,
    },
}

/// Server-owned side of the script boundary.
#[derive(Debug, Clone)]
pub struct ScriptBoundary {
    event_admission: Arc<ScriptEventAdmission>,
    command_rx: Arc<Mutex<mpsc::Receiver<ScriptCommand>>>,
    player_command_owners: PlayerCommandOwners,
    host_admissions: Arc<HostAdmissionLedger>,
}

#[derive(Debug)]
struct ScriptEventAdmission {
    closed: AtomicBool,
    sender: StdMutex<Option<mpsc::Sender<ScriptEvent>>>,
    weak_sender: mpsc::WeakSender<ScriptEvent>,
    coalesced_server_tick: Arc<StdMutex<CoalescedServerTick>>,
}

#[derive(Debug, Default)]
struct CoalescedServerTick {
    pending: Option<u64>,
    highest_seen: Option<u64>,
}

impl ScriptEventAdmission {
    fn sender(&self) -> Option<mpsc::Sender<ScriptEvent>> {
        if self.closed.load(Ordering::Acquire) {
            return None;
        }
        let sender = self.weak_sender.upgrade()?;
        (!self.closed.load(Ordering::Acquire)).then_some(sender)
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        match self.sender.lock() {
            Ok(mut sender) => {
                sender.take();
            }
            Err(poisoned) => {
                poisoned.into_inner().take();
            }
        }
    }
}

impl ScriptBoundary {
    /// Enqueue an immutable event without blocking a server task.
    pub fn try_enqueue_event(&self, event: ScriptEvent) -> Result<(), ScriptQueueError> {
        let Some(event_tx) = self.event_admission.sender() else {
            return Err(ScriptQueueError::Closed);
        };
        event_tx.try_send(event).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => ScriptQueueError::Full,
            mpsc::error::TrySendError::Closed(_) => ScriptQueueError::Closed,
        })
    }

    /// Push the latest simulation tick without blocking or losing timer progress.
    ///
    /// When the normal event queue is full, newer ticks replace the one pending
    /// coalesced tick. The host drains ordinary queued events first, then observes
    /// the latest tick before blocking again.
    pub fn try_enqueue_latest_server_tick(&self, tick: u64) -> Result<(), ScriptQueueError> {
        let Some(event_tx) = self.event_admission.sender() else {
            return Err(ScriptQueueError::Closed);
        };
        let mut coalesced = match self.event_admission.coalesced_server_tick.lock() {
            Ok(coalesced) => coalesced,
            Err(poisoned) => poisoned.into_inner(),
        };
        if coalesced
            .highest_seen
            .is_some_and(|highest| tick <= highest)
        {
            return Ok(());
        }
        coalesced.highest_seen = Some(tick);
        let latest_tick = coalesced
            .pending
            .take()
            .map_or(tick, |pending| pending.max(tick));
        match event_tx.try_send(ScriptEvent::server_tick(latest_tick)) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => {
                coalesced.pending = Some(latest_tick);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Err(ScriptQueueError::Closed),
        }
    }

    /// Deliver a required event, waiting for bounded host-queue capacity.
    ///
    /// Capacity and closure wake this future through the channel. This deliberately
    /// differs from lossy telemetry submitted through [`Self::try_enqueue_event`].
    pub async fn enqueue_required_event(&self, event: ScriptEvent) -> Result<(), ScriptQueueError> {
        let Some(event_tx) = self.event_admission.sender() else {
            return Err(ScriptQueueError::Closed);
        };
        event_tx
            .send(event)
            .await
            .map_err(|_| ScriptQueueError::Closed)
    }

    /// Deliver a targeted owner event through the required-delivery path.
    pub async fn enqueue_targeted_event(&self, event: ScriptEvent) -> Result<(), ScriptQueueError> {
        self.enqueue_required_event(event).await
    }

    /// Stop accepting new host events while allowing already admitted events to drain.
    pub fn close_event_admission(&self) {
        self.event_admission.close();
        self.player_command_owners.clear();
    }

    /// Return a sorted snapshot of currently active plugin command roots.
    pub fn player_command_roots(&self) -> Vec<String> {
        self.player_command_owners.roots(false)
    }

    /// Return a sorted snapshot of active operator-only plugin command roots.
    pub fn operator_command_roots(&self) -> Vec<String> {
        self.player_command_owners.roots(true)
    }

    /// Route a raw player command with the immutable context observed by the server.
    ///
    /// Permission is checked before the bounded event queue so an operator-only
    /// root always reports denial instead of inheriting queue backpressure.
    pub fn try_enqueue_player_command_with_context(
        &self,
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        raw: &str,
    ) -> Result<PlayerCommandAdmission, ScriptQueueError> {
        let Some((root, arguments)) = split_player_command(raw) else {
            return Ok(PlayerCommandAdmission::NotOwned);
        };
        let Some(owner) = self.player_command_owners.owner(root) else {
            return Ok(PlayerCommandAdmission::NotOwned);
        };
        if owner.operator_only && !context.operator() {
            return Ok(PlayerCommandAdmission::PermissionDenied);
        };
        let event = match ScriptEvent::try_player_command_with_context(
            owner.plugin_id,
            player_id,
            context,
            root,
            arguments,
        ) {
            Ok(event) => event,
            Err(error) => return Ok(PlayerCommandAdmission::OwnedRejected { error }),
        };
        match self.try_enqueue_event(event) {
            Ok(()) => Ok(PlayerCommandAdmission::Enqueued),
            Err(error @ ScriptQueueError::Full) => Err(error),
            Err(error @ ScriptQueueError::Closed) => {
                self.player_command_owners.clear();
                Err(error)
            }
        }
    }

    /// Wait for the next command emitted by the script host.
    pub async fn recv_command(&self) -> Option<ScriptCommand> {
        self.command_rx.lock().await.recv().await
    }

    /// Consume one exact host-issued admission ticket.
    pub fn accept_host_command(
        &self,
        command: ScriptCommand,
    ) -> Result<AdmittedScriptCommand, ScriptCommandAcceptanceError> {
        let ScriptCommand::HostAttached {
            provenance,
            request,
        } = command
        else {
            return Err(ScriptCommandAcceptanceError::NotHostAttached);
        };
        self.host_admissions.accept(provenance, request)
    }
}

/// Script-host side of the bounded boundary.
#[derive(Debug)]
pub struct ScriptHostEndpoint {
    event_rx: mpsc::Receiver<ScriptEvent>,
    coalesced_server_tick: Arc<StdMutex<CoalescedServerTick>>,
    coalesced_tick_due: bool,
    highest_delivered_tick: Option<u64>,
    command_tx: mpsc::Sender<ScriptCommand>,
    player_command_owners: PlayerCommandOwners,
    #[cfg(any(test, feature = "lua-runtime"))]
    host_admissions: Arc<HostAdmissionLedger>,
}

impl ScriptHostEndpoint {
    /// Wait asynchronously until an event arrives or the server side closes.
    pub async fn recv_event(&mut self) -> Option<ScriptEvent> {
        loop {
            if self.coalesced_tick_due {
                self.coalesced_tick_due = false;
                if let Some(event) = take_coalesced_server_tick(&self.coalesced_server_tick) {
                    if let Some(event) = self.accept_monotonic_event(event) {
                        return Some(event);
                    }
                    continue;
                }
            }
            match self.event_rx.try_recv() {
                Ok(event) => {
                    self.coalesced_tick_due =
                        has_coalesced_server_tick(&self.coalesced_server_tick);
                    if let Some(event) = self.accept_monotonic_event(event) {
                        return Some(event);
                    }
                    continue;
                }
                Err(mpsc::error::TryRecvError::Empty) => {}
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    let event = take_coalesced_server_tick(&self.coalesced_server_tick)?;
                    if let Some(event) = self.accept_monotonic_event(event) {
                        return Some(event);
                    }
                    continue;
                }
            }
            if let Some(event) = take_coalesced_server_tick(&self.coalesced_server_tick) {
                if let Some(event) = self.accept_monotonic_event(event) {
                    return Some(event);
                }
                continue;
            }
            let Some(event) = self.event_rx.recv().await else {
                continue;
            };
            self.coalesced_tick_due = has_coalesced_server_tick(&self.coalesced_server_tick);
            if let Some(event) = self.accept_monotonic_event(event) {
                return Some(event);
            }
        }
    }

    /// Block the dedicated host thread until an event arrives or the server side closes.
    pub fn recv_event_blocking(&mut self) -> Option<ScriptEvent> {
        loop {
            if self.coalesced_tick_due {
                self.coalesced_tick_due = false;
                if let Some(event) = take_coalesced_server_tick(&self.coalesced_server_tick) {
                    if let Some(event) = self.accept_monotonic_event(event) {
                        return Some(event);
                    }
                    continue;
                }
            }
            match self.event_rx.try_recv() {
                Ok(event) => {
                    self.coalesced_tick_due =
                        has_coalesced_server_tick(&self.coalesced_server_tick);
                    if let Some(event) = self.accept_monotonic_event(event) {
                        return Some(event);
                    }
                    continue;
                }
                Err(mpsc::error::TryRecvError::Empty) => {}
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    let event = take_coalesced_server_tick(&self.coalesced_server_tick)?;
                    if let Some(event) = self.accept_monotonic_event(event) {
                        return Some(event);
                    }
                    continue;
                }
            }
            if let Some(event) = take_coalesced_server_tick(&self.coalesced_server_tick) {
                if let Some(event) = self.accept_monotonic_event(event) {
                    return Some(event);
                }
                continue;
            }
            let Some(event) = self.event_rx.blocking_recv() else {
                continue;
            };
            self.coalesced_tick_due = has_coalesced_server_tick(&self.coalesced_server_tick);
            if let Some(event) = self.accept_monotonic_event(event) {
                return Some(event);
            }
        }
    }

    fn accept_monotonic_event(&mut self, event: ScriptEvent) -> Option<ScriptEvent> {
        let ScriptEventKind::ServerTick { tick } = event.kind() else {
            return Some(event);
        };
        if self
            .highest_delivered_tick
            .is_some_and(|highest| *tick <= highest)
        {
            return None;
        }
        self.highest_delivered_tick = Some(*tick);
        Some(event)
    }

    /// Submit a command without blocking the host thread.
    pub fn try_submit_command(
        &self,
        command: ScriptCommand,
    ) -> Result<(), ScriptCommandSubmissionError> {
        if matches!(command, ScriptCommand::HostAttached { .. }) {
            return Err(ScriptCommandSubmissionError::ProvenanceRejected);
        }
        if let Err(error) = command.validate_contract() {
            return Err(ScriptCommandSubmissionError::InvalidCommand { error });
        }
        if let Some(capability) = command.required_capability_kind() {
            return Err(ScriptCommandSubmissionError::PermissionDenied { capability });
        }
        self.command_tx
            .try_send(command)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => ScriptCommandSubmissionError::QueueFull,
                mpsc::error::TrySendError::Closed(_) => ScriptCommandSubmissionError::QueueClosed,
            })
    }

    /// Attach the currently executing Lua plugin identity before crossing into
    /// server-owned command handling. This is intentionally unavailable outside
    /// this crate: a plugin must never choose its own provenance.
    #[cfg(any(test, feature = "lua-runtime"))]
    pub(crate) fn try_submit_plugin_batch(
        &self,
        admission: &HostCommandAdmission,
        batch: CommandBatch,
    ) -> Result<(), ScriptBatchSubmissionError> {
        for command in batch.commands() {
            if matches!(command, ScriptCommand::HostAttached { .. }) {
                return Err(ScriptBatchSubmissionError::Rejected {
                    batch,
                    error: CommandBatchError::ProvenanceRejected,
                });
            }
            if let Err(error) = command.validate_contract() {
                return Err(ScriptBatchSubmissionError::Rejected {
                    batch,
                    error: CommandBatchError::InvalidCommand { error },
                });
            }
            let denied_capability = command
                .required_capability()
                .filter(|capability| !admission.capabilities.allows(*capability))
                .map(RequiredCommandCapability::kind);
            if let Some(capability) = denied_capability {
                return Err(ScriptBatchSubmissionError::Rejected {
                    batch,
                    error: CommandBatchError::PermissionDenied { capability },
                });
            }
        }

        let command_count = batch.commands().len();
        if command_count == 0 {
            return Ok(());
        }
        let permits = match self.command_tx.try_reserve_many(command_count) {
            Ok(permits) => permits,
            Err(mpsc::error::TrySendError::Full(())) => {
                return Err(ScriptBatchSubmissionError::Full(batch));
            }
            Err(mpsc::error::TrySendError::Closed(())) => {
                return Err(ScriptBatchSubmissionError::Closed(batch));
            }
        };
        let attached = match self
            .host_admissions
            .issue(Arc::clone(&admission.plugin_id), batch)
        {
            Ok(attached) => attached,
            Err(batch) => {
                return Err(ScriptBatchSubmissionError::Rejected {
                    batch,
                    error: CommandBatchError::AdmissionUnavailable,
                });
            }
        };
        for (permit, command) in permits.zip(attached) {
            permit.send(command);
        }
        Ok(())
    }

    /// Register the player command roots from one validated plugin manifest.
    pub fn register_player_commands(
        &self,
        manifest: &ValidatedScriptPluginManifest,
    ) -> Result<(), PlayerCommandRegistrationError> {
        self.player_command_owners.register(
            manifest.plugin_id(),
            manifest.player_command_roots(),
            manifest.operator_command_roots(),
        )
    }

    /// Remove every active player command root owned by one plugin.
    pub fn unregister_player_commands(&self, plugin_id: &str) {
        self.player_command_owners.unregister(plugin_id);
    }
}

/// Error returned when active player-command roots cannot be registered.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlayerCommandRegistrationError {
    RootConflict {
        root: String,
        owner_plugin_id: String,
    },
    RootLimitExceeded {
        limit: usize,
        requested: usize,
    },
    AuthorityPoisoned,
}

#[derive(Debug, Clone, Default)]
struct PlayerCommandOwners {
    owners: Arc<RwLock<BTreeMap<String, PlayerCommandOwner>>>,
    disabled: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
struct PlayerCommandOwner {
    plugin_id: String,
    operator_only: bool,
}

impl PlayerCommandOwners {
    fn roots(&self, operator_only: bool) -> Vec<String> {
        if self.disabled.load(Ordering::Acquire) {
            return Vec::new();
        }
        match self.owners.read() {
            Ok(owners) => owners
                .iter()
                .filter(|(_, owner)| owner.operator_only == operator_only)
                .map(|(root, _)| root.clone())
                .collect(),
            Err(poisoned) => {
                drop(poisoned);
                self.disable();
                Vec::new()
            }
        }
    }

    fn owner(&self, root: &str) -> Option<PlayerCommandOwner> {
        if self.disabled.load(Ordering::Acquire) {
            return None;
        }
        match self.owners.read() {
            Ok(owners) => owners.get(root).cloned(),
            Err(poisoned) => {
                drop(poisoned);
                self.disable();
                None
            }
        }
    }

    fn register(
        &self,
        plugin_id: &str,
        player_roots: &[String],
        operator_roots: &[String],
    ) -> Result<(), PlayerCommandRegistrationError> {
        if self.disabled.load(Ordering::Acquire) {
            return Err(PlayerCommandRegistrationError::AuthorityPoisoned);
        }
        let mut owners = match self.owners.write() {
            Ok(owners) => owners,
            Err(poisoned) => {
                poisoned.into_inner().clear();
                self.disabled.store(true, Ordering::Release);
                return Err(PlayerCommandRegistrationError::AuthorityPoisoned);
            }
        };
        let requested_roots = player_roots
            .iter()
            .chain(operator_roots)
            .collect::<Vec<_>>();
        let requested = owners.len().saturating_add(requested_roots.len());
        if requested > MAX_PLAYER_COMMAND_ROOTS {
            return Err(PlayerCommandRegistrationError::RootLimitExceeded {
                limit: MAX_PLAYER_COMMAND_ROOTS,
                requested,
            });
        }
        if let Some((root, owner_plugin_id)) = requested_roots
            .iter()
            .find_map(|root| owners.get(*root).map(|owner| (root, &owner.plugin_id)))
        {
            return Err(PlayerCommandRegistrationError::RootConflict {
                root: (*root).clone(),
                owner_plugin_id: owner_plugin_id.clone(),
            });
        }
        for root in player_roots {
            owners.insert(
                root.clone(),
                PlayerCommandOwner {
                    plugin_id: plugin_id.to_owned(),
                    operator_only: false,
                },
            );
        }
        for root in operator_roots {
            owners.insert(
                root.clone(),
                PlayerCommandOwner {
                    plugin_id: plugin_id.to_owned(),
                    operator_only: true,
                },
            );
        }
        Ok(())
    }

    fn unregister(&self, plugin_id: &str) {
        if self.disabled.load(Ordering::Acquire) {
            self.clear_poisoned();
            return;
        }
        match self.owners.write() {
            Ok(mut owners) => owners.retain(|_, owner| owner.plugin_id != plugin_id),
            Err(poisoned) => {
                poisoned.into_inner().clear();
                self.disabled.store(true, Ordering::Release);
            }
        }
    }

    fn clear(&self) {
        self.disabled.store(true, Ordering::Release);
        self.clear_poisoned();
    }

    fn disable(&self) {
        self.disabled.store(true, Ordering::Release);
        self.clear_poisoned();
    }

    fn clear_poisoned(&self) {
        match self.owners.write() {
            Ok(mut owners) => owners.clear(),
            Err(poisoned) => poisoned.into_inner().clear(),
        }
    }
}

/// Construct the bounded server/host script boundary.
pub fn script_boundary_pair(
    event_capacity: NonZeroUsize,
    command_capacity: NonZeroUsize,
) -> (ScriptBoundary, ScriptHostEndpoint) {
    let event_capacity = event_capacity.get().min(MAX_SCRIPT_EVENT_QUEUE_CAPACITY);
    let command_capacity = command_capacity
        .get()
        .min(MAX_SCRIPT_COMMAND_QUEUE_CAPACITY);
    let (event_tx, event_rx) = mpsc::channel(event_capacity);
    let (command_tx, command_rx) = mpsc::channel(command_capacity);
    let player_command_owners = PlayerCommandOwners::default();
    let host_admissions = Arc::new(HostAdmissionLedger::default());
    let coalesced_server_tick = Arc::new(StdMutex::new(CoalescedServerTick::default()));
    let weak_event_tx = event_tx.downgrade();
    (
        ScriptBoundary {
            event_admission: Arc::new(ScriptEventAdmission {
                closed: AtomicBool::new(false),
                sender: StdMutex::new(Some(event_tx)),
                weak_sender: weak_event_tx,
                coalesced_server_tick: Arc::clone(&coalesced_server_tick),
            }),
            command_rx: Arc::new(Mutex::new(command_rx)),
            player_command_owners: player_command_owners.clone(),
            host_admissions: Arc::clone(&host_admissions),
        },
        ScriptHostEndpoint {
            event_rx,
            coalesced_server_tick,
            coalesced_tick_due: false,
            highest_delivered_tick: None,
            command_tx,
            player_command_owners,
            #[cfg(any(test, feature = "lua-runtime"))]
            host_admissions,
        },
    )
}

fn take_coalesced_server_tick(slot: &StdMutex<CoalescedServerTick>) -> Option<ScriptEvent> {
    let tick = match slot.lock() {
        Ok(mut slot) => slot.pending.take(),
        Err(poisoned) => poisoned.into_inner().pending.take(),
    }?;
    Some(ScriptEvent::server_tick(tick))
}

fn has_coalesced_server_tick(slot: &StdMutex<CoalescedServerTick>) -> bool {
    match slot.lock() {
        Ok(slot) => slot.pending.is_some(),
        Err(poisoned) => poisoned.into_inner().pending.is_some(),
    }
}

/// Host capability required by privileged outbound script commands.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptCommandCapability {
    RunConsoleCommandRoot { root: String },
    SpawnEntityType { entity_type: String },
    PluginStorage,
    InventoryMenus,
    InventoryStorageTransactions,
    PlayerInventory,
    Zones,
    Villagers,
    PlayerTeleport,
    PlayerQueries,
}

/// Stable non-owning category used in public command-admission errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptCommandCapabilityKind {
    RunConsoleCommand,
    SpawnEntity,
    PluginStorage,
    InventoryMenus,
    InventoryStorageTransactions,
    PlayerInventory,
    Zones,
    Villagers,
    PlayerTeleport,
    PlayerQueries,
}

impl ScriptCommandCapabilityKind {
    pub const fn code(self) -> &'static str {
        match self {
            Self::RunConsoleCommand => "run_console_command",
            Self::SpawnEntity => "spawn_entity",
            Self::PluginStorage => "plugin_storage",
            Self::InventoryMenus => "inventory_menus",
            Self::InventoryStorageTransactions => "inventory_storage_transactions",
            Self::PlayerInventory => "player_inventory",
            Self::Zones => "zones",
            Self::Villagers => "villagers",
            Self::PlayerTeleport => "player_teleport",
            Self::PlayerQueries => "player_queries",
        }
    }

    pub const fn field(self) -> &'static str {
        match self {
            Self::RunConsoleCommand => "console command root",
            Self::SpawnEntity => "spawn entity type",
            Self::PluginStorage => "plugin storage",
            Self::InventoryMenus => "inventory menu",
            Self::InventoryStorageTransactions => "inventory storage transaction",
            Self::PlayerInventory => "player inventory transaction",
            Self::Zones => "zone",
            Self::Villagers => "villager",
            Self::PlayerTeleport => "player teleport",
            Self::PlayerQueries => "player query",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequiredCommandCapability<'a> {
    RunConsoleCommandRoot { root: &'a str },
    SpawnEntityType { entity_type: &'a str },
    PluginStorage,
    InventoryMenus,
    InventoryStorageTransactions,
    PlayerInventory,
    Zones,
    Villagers,
    PlayerTeleport,
    PlayerQueries,
}

impl RequiredCommandCapability<'_> {
    const fn kind(self) -> ScriptCommandCapabilityKind {
        match self {
            Self::RunConsoleCommandRoot { .. } => ScriptCommandCapabilityKind::RunConsoleCommand,
            Self::SpawnEntityType { .. } => ScriptCommandCapabilityKind::SpawnEntity,
            Self::PluginStorage => ScriptCommandCapabilityKind::PluginStorage,
            Self::InventoryMenus => ScriptCommandCapabilityKind::InventoryMenus,
            Self::InventoryStorageTransactions => {
                ScriptCommandCapabilityKind::InventoryStorageTransactions
            }
            Self::PlayerInventory => ScriptCommandCapabilityKind::PlayerInventory,
            Self::Zones => ScriptCommandCapabilityKind::Zones,
            Self::Villagers => ScriptCommandCapabilityKind::Villagers,
            Self::PlayerTeleport => ScriptCommandCapabilityKind::PlayerTeleport,
            Self::PlayerQueries => ScriptCommandCapabilityKind::PlayerQueries,
        }
    }
}

/// Declarative subscription to one Solaris script event name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ScriptEventSubscription {
    event_name: String,
}

impl ScriptEventSubscription {
    fn new(event_name: String) -> Self {
        Self { event_name }
    }

    pub fn event_name(&self) -> &str {
        &self.event_name
    }
}

/// Plugin load phase hint for a future script loader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ScriptPluginLoadPhase {
    Startup,
    #[default]
    PostWorld,
}

/// Relationship between this plugin and another Solaris plugin id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptPluginDependencyRelation {
    Required,
    Optional,
    LoadBefore,
}

/// Declarative dependency or load-order edge for a future script loader.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ScriptPluginDependency {
    plugin_id: String,
    relation: ScriptPluginDependencyRelation,
}

impl ScriptPluginDependency {
    fn new(plugin_id: String, relation: ScriptPluginDependencyRelation) -> Self {
        Self {
            plugin_id,
            relation,
        }
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub fn relation(&self) -> ScriptPluginDependencyRelation {
        self.relation
    }
}

/// Plugin manifest contract consumed by a future server-side script loader.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ScriptPluginManifest {
    plugin_id: String,
    display_name: String,
    version: String,
    requested_api_version: ScriptApiVersion,
    load_phase: ScriptPluginLoadPhase,
    event_subscriptions: Vec<ScriptEventSubscription>,
    dependencies: Vec<ScriptPluginDependency>,
    declared_command_capabilities: Vec<ScriptCommandCapability>,
    player_command_roots: Vec<String>,
    operator_command_roots: Vec<String>,
    declared_permissions: Vec<String>,
    preflight_error: Option<ScriptPluginManifestError>,
}

impl ScriptPluginManifest {
    /// Build a script plugin manifest DTO.
    pub fn new(
        plugin_id: impl AsRef<str>,
        display_name: impl AsRef<str>,
        version: impl AsRef<str>,
        requested_api_version: ScriptApiVersion,
    ) -> Self {
        let mut preflight_error = None;
        let plugin_id = bounded_manifest_owned(
            "plugin id",
            plugin_id.as_ref(),
            MAX_PLUGIN_ID_BYTES,
            &mut preflight_error,
        );
        let display_name = bounded_manifest_owned(
            "display name",
            display_name.as_ref(),
            MAX_PLUGIN_DISPLAY_NAME_BYTES,
            &mut preflight_error,
        );
        let version = bounded_manifest_owned(
            "version",
            version.as_ref(),
            MAX_PLUGIN_VERSION_BYTES,
            &mut preflight_error,
        );
        Self {
            plugin_id,
            display_name,
            version,
            requested_api_version,
            load_phase: ScriptPluginLoadPhase::default(),
            event_subscriptions: Vec::new(),
            dependencies: Vec::new(),
            declared_command_capabilities: Vec::new(),
            player_command_roots: Vec::new(),
            operator_command_roots: Vec::new(),
            declared_permissions: Vec::new(),
            preflight_error,
        }
    }

    /// Declare the preferred load phase for a future loader.
    pub fn with_load_phase(mut self, load_phase: ScriptPluginLoadPhase) -> Self {
        self.load_phase = load_phase;
        self
    }

    /// Declare interest in one Solaris-native script event name.
    pub fn subscribe_event(mut self, event_name: impl AsRef<str>) -> Self {
        if self.preflight_error.is_some() {
            return self;
        }
        if self.event_subscriptions.len() >= MAX_MANIFEST_EVENT_SUBSCRIPTIONS {
            self.preflight_error = Some(ScriptPluginManifestError::TooManyEntries {
                field: "event subscriptions",
                max: MAX_MANIFEST_EVENT_SUBSCRIPTIONS,
            });
            return self;
        }
        let event_name = bounded_manifest_owned(
            "event subscription",
            event_name.as_ref(),
            MAX_MANIFEST_FIELD_BYTES,
            &mut self.preflight_error,
        );
        if self.preflight_error.is_none() {
            self.event_subscriptions
                .push(ScriptEventSubscription::new(event_name));
        }
        self
    }

    /// Declare a plugin dependency or load-order edge.
    pub fn declare_dependency(
        mut self,
        plugin_id: impl AsRef<str>,
        relation: ScriptPluginDependencyRelation,
    ) -> Self {
        if self.preflight_error.is_some() {
            return self;
        }
        if self.dependencies.len() >= MAX_MANIFEST_DEPENDENCIES {
            self.preflight_error = Some(ScriptPluginManifestError::TooManyEntries {
                field: "dependencies",
                max: MAX_MANIFEST_DEPENDENCIES,
            });
            return self;
        }
        let plugin_id = bounded_manifest_owned(
            "dependency plugin id",
            plugin_id.as_ref(),
            MAX_PLUGIN_ID_BYTES,
            &mut self.preflight_error,
        );
        if self.preflight_error.is_none() {
            self.dependencies
                .push(ScriptPluginDependency::new(plugin_id, relation));
        }
        self
    }

    /// Declare that this plugin requests access to a console command root.
    pub fn declare_console_command_root(mut self, root: impl AsRef<str>) -> Self {
        let root = bounded_manifest_owned(
            "console command root",
            root.as_ref(),
            MAX_PLAYER_COMMAND_ROOT_BYTES,
            &mut self.preflight_error,
        );
        if self.preflight_error.is_none() {
            self.push_capability(ScriptCommandCapability::RunConsoleCommandRoot { root });
        }
        self
    }

    /// Declare one exact entity type this plugin may spawn.
    pub fn declare_spawn_entity_type(mut self, entity_type: impl AsRef<str>) -> Self {
        let entity_type = bounded_manifest_owned(
            "spawn entity type",
            entity_type.as_ref(),
            MAX_SCRIPT_RESOURCE_ID_BYTES,
            &mut self.preflight_error,
        );
        if self.preflight_error.is_none() {
            self.push_capability(ScriptCommandCapability::SpawnEntityType { entity_type });
        }
        self
    }

    /// Declare access to the plugin-owned key/value store.
    pub fn declare_plugin_storage(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::PluginStorage);
        self
    }

    /// Declare access to server-owned inventory menu requests and click events.
    pub fn declare_inventory_menus(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::InventoryMenus);
        self
    }

    /// Declare access to atomic player-inventory and plugin-storage requests.
    pub fn declare_inventory_storage_transactions(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::InventoryStorageTransactions);
        self
    }

    /// Declare access to atomic player main-inventory and hotbar mutations.
    pub fn declare_player_inventory(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::PlayerInventory);
        self
    }

    /// Declare access to plugin-owned axis-aligned zones.
    pub fn declare_zones(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::Zones);
        self
    }

    /// Declare access to opaque villager bindings and bounded goal requests.
    pub fn declare_villagers(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::Villagers);
        self
    }

    /// Declare access to same-dimension authoritative player teleports.
    pub fn declare_player_teleport(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::PlayerTeleport);
        self
    }

    /// Declare access to bounded connected-player snapshots.
    pub fn declare_player_queries(mut self) -> Self {
        self.push_capability(ScriptCommandCapability::PlayerQueries);
        self
    }

    /// Declare a literal command root that players may invoke for this plugin.
    pub fn declare_player_command_root(mut self, root: impl AsRef<str>) -> Self {
        let root = bounded_manifest_owned(
            "player command root",
            root.as_ref(),
            MAX_PLAYER_COMMAND_ROOT_BYTES,
            &mut self.preflight_error,
        );
        if self.preflight_error.is_none() {
            if self.player_command_roots.len() >= MAX_PLAYER_COMMAND_ROOTS {
                self.preflight_error = Some(ScriptPluginManifestError::TooManyEntries {
                    field: "player command roots",
                    max: MAX_PLAYER_COMMAND_ROOTS,
                });
            } else {
                self.player_command_roots.push(root);
            }
        }
        self
    }

    /// Declare a literal player command root that only operators may invoke.
    pub fn declare_operator_command_root(mut self, root: impl AsRef<str>) -> Self {
        let root = bounded_manifest_owned(
            "operator command root",
            root.as_ref(),
            MAX_PLAYER_COMMAND_ROOT_BYTES,
            &mut self.preflight_error,
        );
        if self.preflight_error.is_none() {
            if self.operator_command_roots.len() >= MAX_PLAYER_COMMAND_ROOTS {
                self.preflight_error = Some(ScriptPluginManifestError::TooManyEntries {
                    field: "operator command roots",
                    max: MAX_PLAYER_COMMAND_ROOTS,
                });
            } else {
                self.operator_command_roots.push(root);
            }
        }
        self
    }

    /// Declare an opaque plugin permission string for a future loader.
    pub fn declare_permission(mut self, permission: impl AsRef<str>) -> Self {
        let permission = bounded_manifest_owned(
            "permission",
            permission.as_ref(),
            MAX_MANIFEST_FIELD_BYTES,
            &mut self.preflight_error,
        );
        if self.preflight_error.is_none() {
            if self.declared_permissions.len() >= MAX_MANIFEST_PERMISSIONS {
                self.preflight_error = Some(ScriptPluginManifestError::TooManyEntries {
                    field: "permissions",
                    max: MAX_MANIFEST_PERMISSIONS,
                });
            } else {
                self.declared_permissions.push(permission);
            }
        }
        self
    }

    fn push_capability(&mut self, capability: ScriptCommandCapability) {
        if self.preflight_error.is_some() {
            return;
        }
        if self.declared_command_capabilities.len() >= MAX_MANIFEST_CAPABILITIES {
            self.preflight_error = Some(ScriptPluginManifestError::TooManyEntries {
                field: "command capabilities",
                max: MAX_MANIFEST_CAPABILITIES,
            });
            return;
        }
        self.declared_command_capabilities.push(capability);
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn requested_api_version(&self) -> ScriptApiVersion {
        self.requested_api_version
    }

    pub fn load_phase(&self) -> ScriptPluginLoadPhase {
        self.load_phase
    }

    pub fn event_subscriptions(&self) -> &[ScriptEventSubscription] {
        &self.event_subscriptions
    }

    pub fn dependencies(&self) -> &[ScriptPluginDependency] {
        &self.dependencies
    }

    pub fn declared_command_capabilities(&self) -> &[ScriptCommandCapability] {
        &self.declared_command_capabilities
    }

    pub fn player_command_roots(&self) -> &[String] {
        &self.player_command_roots
    }

    pub fn operator_command_roots(&self) -> &[String] {
        &self.operator_command_roots
    }

    pub fn declared_permissions(&self) -> &[String] {
        &self.declared_permissions
    }

    /// Validate and normalize this manifest for trusted host-side use.
    pub fn validate(&self) -> Result<ValidatedScriptPluginManifest, ScriptPluginManifestError> {
        if let Some(error) = &self.preflight_error {
            return Err(error.clone());
        }
        validate_manifest_field("plugin id", &self.plugin_id, MAX_PLUGIN_ID_BYTES, false)?;
        validate_manifest_field(
            "display name",
            &self.display_name,
            MAX_PLUGIN_DISPLAY_NAME_BYTES,
            false,
        )?;
        validate_manifest_field("version", &self.version, MAX_PLUGIN_VERSION_BYTES, false)?;
        validate_manifest_count(
            "event subscriptions",
            self.event_subscriptions.len(),
            MAX_MANIFEST_EVENT_SUBSCRIPTIONS,
        )?;
        validate_manifest_count(
            "dependencies",
            self.dependencies.len(),
            MAX_MANIFEST_DEPENDENCIES,
        )?;
        validate_manifest_count(
            "command capabilities",
            self.declared_command_capabilities.len(),
            MAX_MANIFEST_CAPABILITIES,
        )?;
        validate_manifest_count(
            "player command roots",
            self.player_command_roots.len(),
            MAX_PLAYER_COMMAND_ROOTS,
        )?;
        validate_manifest_count(
            "operator command roots",
            self.operator_command_roots.len(),
            MAX_PLAYER_COMMAND_ROOTS,
        )?;
        validate_manifest_count(
            "permissions",
            self.declared_permissions.len(),
            MAX_MANIFEST_PERMISSIONS,
        )?;
        for subscription in &self.event_subscriptions {
            validate_manifest_field(
                "event subscription",
                subscription.event_name(),
                MAX_MANIFEST_FIELD_BYTES,
                false,
            )?;
        }
        for dependency in &self.dependencies {
            validate_manifest_field(
                "dependency plugin id",
                dependency.plugin_id(),
                MAX_PLUGIN_ID_BYTES,
                false,
            )?;
        }
        for capability in &self.declared_command_capabilities {
            match capability {
                ScriptCommandCapability::RunConsoleCommandRoot { root } => {
                    validate_manifest_field(
                        "console command root",
                        root,
                        MAX_PLAYER_COMMAND_ROOT_BYTES,
                        false,
                    )?;
                }
                ScriptCommandCapability::SpawnEntityType { entity_type } => {
                    validate_manifest_field(
                        "spawn entity type",
                        entity_type,
                        MAX_SCRIPT_RESOURCE_ID_BYTES,
                        false,
                    )?;
                }
                _ => {}
            }
        }
        for permission in &self.declared_permissions {
            validate_manifest_field("permission", permission, MAX_MANIFEST_FIELD_BYTES, false)?;
            if !permission.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b':' | b'.' | b'_' | b'-' | b'/')
            }) {
                return Err(ScriptPluginManifestError::InvalidField {
                    field: "permission",
                });
            }
        }
        if self.plugin_id.trim().is_empty() {
            return Err(ScriptPluginManifestError::BlankPluginId);
        }

        if !is_valid_plugin_id(&self.plugin_id) {
            return Err(ScriptPluginManifestError::InvalidPluginId {
                plugin_id: self.plugin_id.clone(),
            });
        }
        if self.display_name.trim().is_empty()
            || self
                .display_name
                .chars()
                .any(|character| character.is_control())
        {
            return Err(ScriptPluginManifestError::InvalidField {
                field: "display name",
            });
        }
        if !is_valid_plugin_version(&self.version) {
            return Err(ScriptPluginManifestError::InvalidField { field: "version" });
        }

        if !supports_script_api_version(self.requested_api_version) {
            return Err(ScriptPluginManifestError::UnsupportedScriptApiVersion {
                requested: self.requested_api_version,
                supported: SCRIPT_API_VERSION,
            });
        }

        let mut normalized_event_subscriptions = Vec::with_capacity(self.event_subscriptions.len());
        for subscription in &self.event_subscriptions {
            let event_name = normalize_event_name(subscription.event_name());
            if !is_supported_event_name(&event_name) {
                return Err(ScriptPluginManifestError::InvalidEventName { event_name });
            }
            if normalized_event_subscriptions.iter().any(
                |subscription: &ScriptEventSubscription| subscription.event_name() == event_name,
            ) {
                return Err(ScriptPluginManifestError::DuplicateEventSubscription { event_name });
            }
            normalized_event_subscriptions.push(ScriptEventSubscription::new(event_name));
        }

        let mut normalized_dependencies = Vec::with_capacity(self.dependencies.len());
        for dependency in &self.dependencies {
            let plugin_id = normalize_plugin_id(dependency.plugin_id());
            if plugin_id.is_empty() {
                return Err(ScriptPluginManifestError::BlankDependencyPluginId);
            }
            if !is_valid_plugin_id(&plugin_id) {
                return Err(ScriptPluginManifestError::InvalidDependencyPluginId { plugin_id });
            }
            if plugin_id == self.plugin_id {
                return Err(ScriptPluginManifestError::SelfDependency { plugin_id });
            }
            if normalized_dependencies
                .iter()
                .any(|dependency: &ScriptPluginDependency| dependency.plugin_id() == plugin_id)
            {
                return Err(ScriptPluginManifestError::DuplicateDependency { plugin_id });
            }
            normalized_dependencies.push(ScriptPluginDependency::new(
                plugin_id,
                dependency.relation(),
            ));
        }

        let mut normalized_capabilities =
            Vec::with_capacity(self.declared_command_capabilities.len());
        for capability in &self.declared_command_capabilities {
            match capability {
                ScriptCommandCapability::RunConsoleCommandRoot { root } => {
                    let root = validate_command_root(root)?;
                    if normalized_capabilities.iter().any(
                        |capability| matches!(capability, ScriptCommandCapability::RunConsoleCommandRoot { root: existing } if existing == &root),
                    ) {
                        return Err(ScriptPluginManifestError::DuplicateCommandRoot { root });
                    }
                    normalized_capabilities
                        .push(ScriptCommandCapability::RunConsoleCommandRoot { root });
                }
                ScriptCommandCapability::SpawnEntityType { entity_type } => {
                    let entity_type = validate_script_resource_id(entity_type)?;
                    let spawn_count = normalized_capabilities
                        .iter()
                        .filter(|capability| {
                            matches!(capability, ScriptCommandCapability::SpawnEntityType { .. })
                        })
                        .count();
                    if spawn_count >= MAX_SPAWN_ENTITY_TYPES {
                        return Err(ScriptPluginManifestError::TooManySpawnEntityTypes {
                            max: MAX_SPAWN_ENTITY_TYPES,
                        });
                    }
                    if normalized_capabilities.iter().any(|capability| {
                        matches!(capability, ScriptCommandCapability::SpawnEntityType { entity_type: existing } if existing == &entity_type)
                    }) {
                        return Err(ScriptPluginManifestError::DuplicateSpawnEntityType {
                            entity_type,
                        });
                    }
                    normalized_capabilities
                        .push(ScriptCommandCapability::SpawnEntityType { entity_type });
                }
                ScriptCommandCapability::PluginStorage
                | ScriptCommandCapability::InventoryMenus
                | ScriptCommandCapability::InventoryStorageTransactions
                | ScriptCommandCapability::PlayerInventory
                | ScriptCommandCapability::Zones
                | ScriptCommandCapability::Villagers
                | ScriptCommandCapability::PlayerTeleport
                | ScriptCommandCapability::PlayerQueries => {
                    if normalized_capabilities.contains(capability) {
                        return Err(ScriptPluginManifestError::DuplicateCapability {
                            capability: capability.clone(),
                        });
                    }
                    normalized_capabilities.push(capability.clone());
                }
            }
        }

        let mut player_command_roots = Vec::with_capacity(self.player_command_roots.len());
        for root in &self.player_command_roots {
            validate_player_command_root(root)?;
            if BUILT_IN_PLAYER_COMMAND_ROOTS.contains(&root.as_str()) {
                return Err(ScriptPluginManifestError::ReservedPlayerCommandRoot {
                    root: root.clone(),
                });
            }
            if !player_command_roots.contains(root) {
                player_command_roots.push(root.clone());
            }
        }
        let mut operator_command_roots = Vec::with_capacity(self.operator_command_roots.len());
        for root in &self.operator_command_roots {
            validate_player_command_root(root)?;
            if BUILT_IN_PLAYER_COMMAND_ROOTS.contains(&root.as_str()) {
                return Err(ScriptPluginManifestError::ReservedPlayerCommandRoot {
                    root: root.clone(),
                });
            }
            if player_command_roots.contains(root) {
                return Err(ScriptPluginManifestError::ConflictingPlayerCommandRoot {
                    root: root.clone(),
                });
            }
            if !operator_command_roots.contains(root) {
                operator_command_roots.push(root.clone());
            }
        }

        Ok(ValidatedScriptPluginManifest {
            plugin_id: self.plugin_id.clone(),
            display_name: self.display_name.clone(),
            version: self.version.clone(),
            requested_api_version: self.requested_api_version,
            load_phase: self.load_phase,
            event_subscriptions: normalized_event_subscriptions,
            dependencies: normalized_dependencies,
            declared_command_capabilities: normalized_capabilities,
            player_command_roots,
            operator_command_roots,
            declared_permissions: self.declared_permissions.clone(),
        })
    }
}

/// Validated and normalized script plugin manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ValidatedScriptPluginManifest {
    plugin_id: String,
    display_name: String,
    version: String,
    requested_api_version: ScriptApiVersion,
    load_phase: ScriptPluginLoadPhase,
    event_subscriptions: Vec<ScriptEventSubscription>,
    dependencies: Vec<ScriptPluginDependency>,
    declared_command_capabilities: Vec<ScriptCommandCapability>,
    player_command_roots: Vec<String>,
    operator_command_roots: Vec<String>,
    declared_permissions: Vec<String>,
}

impl ValidatedScriptPluginManifest {
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn requested_api_version(&self) -> ScriptApiVersion {
        self.requested_api_version
    }

    pub fn load_phase(&self) -> ScriptPluginLoadPhase {
        self.load_phase
    }

    pub fn event_subscriptions(&self) -> &[ScriptEventSubscription] {
        &self.event_subscriptions
    }

    pub fn dependencies(&self) -> &[ScriptPluginDependency] {
        &self.dependencies
    }

    pub fn declared_command_capabilities(&self) -> &[ScriptCommandCapability] {
        &self.declared_command_capabilities
    }

    pub fn player_command_roots(&self) -> &[String] {
        &self.player_command_roots
    }

    pub fn operator_command_roots(&self) -> &[String] {
        &self.operator_command_roots
    }

    pub fn declared_permissions(&self) -> &[String] {
        &self.declared_permissions
    }

    /// Trusted host-side conversion from validated manifest declarations to
    /// executable command capabilities.
    #[cfg(any(test, feature = "lua-runtime"))]
    pub(crate) fn to_command_capabilities(&self) -> CommandCapabilities {
        let mut capabilities = CommandCapabilities::none();
        for capability in &self.declared_command_capabilities {
            match capability {
                ScriptCommandCapability::RunConsoleCommandRoot { root } => {
                    capabilities = capabilities.allow_console_command_root(root);
                }
                ScriptCommandCapability::SpawnEntityType { entity_type } => {
                    capabilities = capabilities.allow_spawn_entity_type(entity_type);
                }
                ScriptCommandCapability::PluginStorage => {
                    capabilities = capabilities.allow_plugin_storage();
                }
                ScriptCommandCapability::InventoryMenus => {
                    capabilities = capabilities.allow_inventory_menus();
                }
                ScriptCommandCapability::InventoryStorageTransactions => {
                    capabilities = capabilities.allow_inventory_storage_transactions();
                }
                ScriptCommandCapability::PlayerInventory => {
                    capabilities = capabilities.allow_player_inventory();
                }
                ScriptCommandCapability::Zones => {
                    capabilities = capabilities.allow_zones();
                }
                ScriptCommandCapability::Villagers => {
                    capabilities = capabilities.allow_villagers();
                }
                ScriptCommandCapability::PlayerTeleport => {
                    capabilities = capabilities.allow_player_teleport();
                }
                ScriptCommandCapability::PlayerQueries => {
                    capabilities = capabilities.allow_player_queries();
                }
            }
        }
        capabilities
    }
}

/// Error returned when validating a script plugin manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptPluginManifestError {
    FieldTooLong {
        field: &'static str,
        max_bytes: usize,
    },
    EmptyField {
        field: &'static str,
    },
    InvalidField {
        field: &'static str,
    },
    TooManyEntries {
        field: &'static str,
        max: usize,
    },
    BlankPluginId,
    InvalidPluginId {
        plugin_id: String,
    },
    UnsupportedScriptApiVersion {
        requested: ScriptApiVersion,
        supported: ScriptApiVersion,
    },
    InvalidEventName {
        event_name: String,
    },
    DuplicateEventSubscription {
        event_name: String,
    },
    BlankDependencyPluginId,
    InvalidDependencyPluginId {
        plugin_id: String,
    },
    SelfDependency {
        plugin_id: String,
    },
    DuplicateDependency {
        plugin_id: String,
    },
    BlankCommandRoot,
    UnboundedCommandRoot {
        root: String,
    },
    DuplicateCommandRoot {
        root: String,
    },
    DuplicateCapability {
        capability: ScriptCommandCapability,
    },
    InvalidSpawnEntityType {
        entity_type: String,
    },
    DuplicateSpawnEntityType {
        entity_type: String,
    },
    TooManySpawnEntityTypes {
        max: usize,
    },
    InvalidPlayerCommandRoot {
        root: String,
    },
    PlayerCommandRootTooLong {
        root: String,
        max_bytes: usize,
    },
    ReservedPlayerCommandRoot {
        root: String,
    },
    ConflictingPlayerCommandRoot {
        root: String,
    },
}

fn validate_manifest_field(
    field: &'static str,
    value: &str,
    max_bytes: usize,
    allow_empty: bool,
) -> Result<(), ScriptPluginManifestError> {
    if !allow_empty && value.is_empty() {
        return Err(ScriptPluginManifestError::EmptyField { field });
    }
    if value.len() > max_bytes {
        return Err(ScriptPluginManifestError::FieldTooLong { field, max_bytes });
    }
    Ok(())
}

fn bounded_manifest_owned(
    field: &'static str,
    value: &str,
    max_bytes: usize,
    error: &mut Option<ScriptPluginManifestError>,
) -> String {
    if error.is_some() {
        return String::new();
    }
    if value.is_empty() {
        *error = Some(ScriptPluginManifestError::EmptyField { field });
        return String::new();
    }
    if value.len() > max_bytes {
        *error = Some(ScriptPluginManifestError::FieldTooLong { field, max_bytes });
        return String::new();
    }
    value.to_owned()
}

fn validate_manifest_count(
    field: &'static str,
    count: usize,
    max: usize,
) -> Result<(), ScriptPluginManifestError> {
    if count > max {
        return Err(ScriptPluginManifestError::TooManyEntries { field, max });
    }
    Ok(())
}

fn is_valid_plugin_version(version: &str) -> bool {
    version.bytes().any(|byte| byte.is_ascii_digit())
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
}

/// Allow-list of privileged outbound command capabilities granted by the host.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct CommandCapabilities {
    console_command_roots: Vec<String>,
    spawn_entity_types: Vec<String>,
    plugin_storage: bool,
    inventory_menus: bool,
    inventory_storage_transactions: bool,
    player_inventory: bool,
    zones: bool,
    villagers: bool,
    player_teleport: bool,
    player_queries: bool,
}

impl CommandCapabilities {
    /// Return capabilities with no privileged console command roots allowed.
    pub fn none() -> Self {
        Self::default()
    }

    #[cfg(any(test, feature = "lua-runtime"))]
    pub(crate) fn allow_console_command_root(mut self, root: impl AsRef<str>) -> Self {
        let root = console_command_root(root.as_ref());
        if !self
            .console_command_roots
            .iter()
            .any(|allowed| allowed == root)
        {
            self.console_command_roots.push(root.to_owned());
        }
        self
    }

    #[cfg(any(test, feature = "lua-runtime"))]
    pub(crate) fn allow_spawn_entity_type(mut self, entity_type: impl AsRef<str>) -> Self {
        let entity_type = entity_type.as_ref().to_owned();
        if !self
            .spawn_entity_types
            .iter()
            .any(|allowed| allowed == &entity_type)
        {
            self.spawn_entity_types.push(entity_type);
        }
        self
    }

    #[cfg(any(test, feature = "lua-runtime"))]
    pub(crate) fn allow_plugin_storage(mut self) -> Self {
        self.plugin_storage = true;
        self
    }

    #[cfg(any(test, feature = "lua-runtime"))]
    pub(crate) fn allow_inventory_menus(mut self) -> Self {
        self.inventory_menus = true;
        self
    }

    #[cfg(any(test, feature = "lua-runtime"))]
    pub(crate) fn allow_inventory_storage_transactions(mut self) -> Self {
        self.inventory_storage_transactions = true;
        self
    }

    #[cfg(any(test, feature = "lua-runtime"))]
    pub(crate) fn allow_player_inventory(mut self) -> Self {
        self.player_inventory = true;
        self
    }

    #[cfg(any(test, feature = "lua-runtime"))]
    pub(crate) fn allow_zones(mut self) -> Self {
        self.zones = true;
        self
    }

    #[cfg(any(test, feature = "lua-runtime"))]
    pub(crate) fn allow_villagers(mut self) -> Self {
        self.villagers = true;
        self
    }

    #[cfg(any(test, feature = "lua-runtime"))]
    pub(crate) fn allow_player_teleport(mut self) -> Self {
        self.player_teleport = true;
        self
    }

    #[cfg(any(test, feature = "lua-runtime"))]
    pub(crate) fn allow_player_queries(mut self) -> Self {
        self.player_queries = true;
        self
    }

    fn allows(&self, capability: RequiredCommandCapability<'_>) -> bool {
        match capability {
            RequiredCommandCapability::RunConsoleCommandRoot { root } => self
                .console_command_roots
                .iter()
                .any(|allowed| allowed == root),
            RequiredCommandCapability::SpawnEntityType { entity_type } => self
                .spawn_entity_types
                .iter()
                .any(|allowed| allowed == entity_type),
            RequiredCommandCapability::PluginStorage => self.plugin_storage,
            RequiredCommandCapability::InventoryMenus => self.inventory_menus,
            RequiredCommandCapability::InventoryStorageTransactions => {
                self.inventory_storage_transactions
            }
            RequiredCommandCapability::PlayerInventory => self.player_inventory,
            RequiredCommandCapability::Zones => self.zones,
            RequiredCommandCapability::Villagers => self.villagers,
            RequiredCommandCapability::PlayerTeleport => self.player_teleport,
            RequiredCommandCapability::PlayerQueries => self.player_queries,
        }
    }
}

/// Error returned when a command batch cannot accept another command.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommandBatchError {
    Full {
        limit: NonZeroUsize,
    },
    PermissionDenied {
        capability: ScriptCommandCapabilityKind,
    },
    ProvenanceRejected,
    InvalidCommand {
        error: ScriptDtoError,
    },
    AdmissionUnavailable,
}

/// Bounded list of commands produced by one script event invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandBatch {
    limit: NonZeroUsize,
    commands: Vec<ScriptCommand>,
}

impl CommandBatch {
    /// Create an empty command batch with a fixed command count limit.
    pub fn new(limit: NonZeroUsize) -> Self {
        let limit = NonZeroUsize::new(limit.get().min(MAX_SCRIPT_COMMAND_BATCH))
            .expect("script command batch limit is non-zero");
        Self {
            limit,
            commands: Vec::new(),
        }
    }

    /// Return the maximum number of commands this batch may contain.
    pub fn limit(&self) -> NonZeroUsize {
        self.limit
    }

    /// Return queued commands as an immutable slice.
    pub fn commands(&self) -> &[ScriptCommand] {
        &self.commands
    }

    /// Consume this batch and return the queued commands.
    pub fn into_commands(self) -> Vec<ScriptCommand> {
        self.commands
    }

    /// Try to append one command without exceeding the batch limit.
    pub fn try_push(&mut self, command: ScriptCommand) -> Result<(), CommandBatchError> {
        if matches!(command, ScriptCommand::HostAttached { .. }) {
            return Err(CommandBatchError::ProvenanceRejected);
        }
        command
            .validate_contract()
            .map_err(|error| CommandBatchError::InvalidCommand { error })?;
        if let Some(capability) = command.required_capability_kind() {
            return Err(CommandBatchError::PermissionDenied { capability });
        }
        self.try_push_validated(command)
    }

    fn try_push_validated(&mut self, command: ScriptCommand) -> Result<(), CommandBatchError> {
        if self.commands.len() >= self.limit.get() {
            return Err(CommandBatchError::Full { limit: self.limit });
        }
        self.commands.push(command);
        Ok(())
    }

    /// Try to append one command if granted by the host capability allow-list.
    pub fn try_push_authorized(
        &mut self,
        command: ScriptCommand,
        capabilities: &CommandCapabilities,
    ) -> Result<(), CommandBatchError> {
        if matches!(command, ScriptCommand::HostAttached { .. }) {
            return Err(CommandBatchError::ProvenanceRejected);
        }
        command
            .validate_contract()
            .map_err(|error| CommandBatchError::InvalidCommand { error })?;
        if let Some(capability) = command.required_capability()
            && !capabilities.allows(capability)
        {
            return Err(CommandBatchError::PermissionDenied {
                capability: capability.kind(),
            });
        }
        self.try_push_validated(command)
    }
}

fn console_command_root(command: &str) -> &str {
    command
        .trim()
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or_default()
}

fn normalize_event_name(event_name: &str) -> String {
    event_name.trim().to_ascii_lowercase()
}

fn is_supported_event_name(event_name: &str) -> bool {
    matches!(
        event_name,
        "server.started"
            | "server.stopping"
            | "player.joined"
            | "player.left"
            | "player.chat"
            | "player.block_broken"
            | "player.block_placed"
            | "player.item_crafted"
            | "player.item_picked_up"
            | "player.entity_killed"
            | "player.entity_interacted"
            | "player.died"
            | "server.tick"
            | "plugin.storage.get_result"
            | "plugin.storage.cas_result"
            | "plugin.storage.delete_result"
            | "inventory.menu.clicked"
            | "inventory.storage_transaction.result"
            | "player.inventory_transaction_result"
            | "player.zone_entered"
            | "player.zone_exited"
            | "zone.command_result"
            | "player.teleport_result"
            | "villager.binding_result"
            | "villager.goal_result"
    )
}

fn normalize_plugin_id(plugin_id: &str) -> String {
    plugin_id.trim().to_ascii_lowercase()
}

fn is_valid_plugin_id(plugin_id: &str) -> bool {
    let mut chars = plugin_id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }

    chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '.' | '_' | '-'))
}

fn validate_command_root(root: &str) -> Result<String, ScriptPluginManifestError> {
    let root = console_command_root(root);
    if root.is_empty() {
        return Err(ScriptPluginManifestError::BlankCommandRoot);
    }
    if root.contains('*') {
        return Err(ScriptPluginManifestError::UnboundedCommandRoot {
            root: root.to_owned(),
        });
    }
    Ok(root.to_owned())
}

fn validate_script_resource_id(value: &str) -> Result<String, ScriptPluginManifestError> {
    if value.len() > MAX_SCRIPT_RESOURCE_ID_BYTES {
        return Err(ScriptPluginManifestError::InvalidSpawnEntityType {
            entity_type: value.to_owned(),
        });
    }
    let Some((namespace, path)) = value.split_once(':') else {
        return Err(ScriptPluginManifestError::InvalidSpawnEntityType {
            entity_type: value.to_owned(),
        });
    };
    if namespace.is_empty()
        || path.is_empty()
        || path.contains(':')
        || !namespace.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
        || !path.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-' | b'.' | b'/')
        })
    {
        return Err(ScriptPluginManifestError::InvalidSpawnEntityType {
            entity_type: value.to_owned(),
        });
    }
    Ok(value.to_owned())
}

fn validate_script_id(value: &str) -> Result<String, ScriptDtoError> {
    if value.is_empty() {
        return Err(ScriptDtoError::EmptyValue { field: "script id" });
    }
    if value.len() > MAX_SCRIPT_ID_BYTES {
        return Err(ScriptDtoError::ValueTooLong {
            field: "script id",
            max_bytes: MAX_SCRIPT_ID_BYTES,
            actual_bytes: value.len(),
        });
    }
    if !value.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
    }) {
        return Err(ScriptDtoError::InvalidId {
            field: "script id",
            actual_bytes: value.len(),
        });
    }
    Ok(value.to_owned())
}

fn validate_bounded_value(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), ScriptDtoError> {
    if value.len() > max_bytes {
        return Err(ScriptDtoError::ValueTooLong {
            field,
            max_bytes,
            actual_bytes: value.len(),
        });
    }
    Ok(())
}

fn validate_bounded_nonempty(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), ScriptDtoError> {
    if value.is_empty() {
        return Err(ScriptDtoError::EmptyValue { field });
    }
    validate_bounded_value(field, value, max_bytes)
}

fn validate_player_identity(uuid: &str, username: &str) -> Result<(), ScriptDtoError> {
    if !uuid
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(ScriptDtoError::InvalidId {
            field: "player uuid",
            actual_bytes: uuid.len(),
        });
    }
    if !username
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(ScriptDtoError::InvalidId {
            field: "player username",
            actual_bytes: username.len(),
        });
    }
    Ok(())
}

fn normalize_player_uuid(uuid: &str) -> Result<String, ScriptDtoError> {
    let normalized = uuid
        .bytes()
        .filter(|byte| *byte != b'-')
        .map(char::from)
        .collect::<String>()
        .to_ascii_lowercase();
    if normalized.len() != 32 || !normalized.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ScriptDtoError::InvalidId {
            field: "protection actor uuid",
            actual_bytes: uuid.len(),
        });
    }
    Ok(normalized)
}

fn validate_contract_resource_id(value: &str) -> Result<String, ScriptDtoError> {
    if value.len() > MAX_SCRIPT_RESOURCE_ID_BYTES {
        return Err(ScriptDtoError::ValueTooLong {
            field: "resource id",
            max_bytes: MAX_SCRIPT_RESOURCE_ID_BYTES,
            actual_bytes: value.len(),
        });
    }
    let Some((namespace, path)) = value.split_once(':') else {
        return Err(ScriptDtoError::InvalidResourceId {
            field: "resource id",
            actual_bytes: value.len(),
        });
    };
    if namespace.is_empty()
        || path.is_empty()
        || path.contains(':')
        || !namespace.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
        || !path.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-' | b'.' | b'/')
        })
    {
        return Err(ScriptDtoError::InvalidResourceId {
            field: "resource id",
            actual_bytes: value.len(),
        });
    }
    Ok(value.to_owned())
}

fn validate_plugin_storage_key(value: &str) -> Result<String, ScriptDtoError> {
    if value.is_empty() {
        return Err(ScriptDtoError::EmptyValue {
            field: "plugin storage key",
        });
    }
    if value.len() > MAX_PLUGIN_STORAGE_KEY_BYTES {
        return Err(ScriptDtoError::ValueTooLong {
            field: "plugin storage key",
            max_bytes: MAX_PLUGIN_STORAGE_KEY_BYTES,
            actual_bytes: value.len(),
        });
    }
    Ok(value.to_owned())
}

fn validate_plugin_storage_value(value: &str) -> Result<(), ScriptDtoError> {
    if value.is_empty() {
        return Err(ScriptDtoError::EmptyValue {
            field: "plugin storage value",
        });
    }
    if value.len() > MAX_PLUGIN_STORAGE_VALUE_BYTES {
        return Err(ScriptDtoError::ValueTooLong {
            field: "plugin storage value",
            max_bytes: MAX_PLUGIN_STORAGE_VALUE_BYTES,
            actual_bytes: value.len(),
        });
    }
    Ok(())
}

fn validate_target_plugin_id(value: &str) -> Result<String, ScriptDtoError> {
    if value.is_empty() {
        return Err(ScriptDtoError::EmptyValue {
            field: "target plugin id",
        });
    }
    if value.len() > MAX_SCRIPT_ID_BYTES {
        return Err(ScriptDtoError::ValueTooLong {
            field: "target plugin id",
            max_bytes: MAX_SCRIPT_ID_BYTES,
            actual_bytes: value.len(),
        });
    }
    if !is_valid_plugin_id(value) {
        return Err(ScriptDtoError::InvalidId {
            field: "target plugin id",
            actual_bytes: value.len(),
        });
    }
    Ok(value.to_owned())
}

fn validate_player_command_root(root: &str) -> Result<(), ScriptPluginManifestError> {
    if root.is_empty()
        || !root.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
    {
        return Err(ScriptPluginManifestError::InvalidPlayerCommandRoot {
            root: root.to_owned(),
        });
    }
    if root.len() > MAX_PLAYER_COMMAND_ROOT_BYTES {
        return Err(ScriptPluginManifestError::PlayerCommandRootTooLong {
            root: root.to_owned(),
            max_bytes: MAX_PLAYER_COMMAND_ROOT_BYTES,
        });
    }
    Ok(())
}

fn split_player_command(raw: &str) -> Option<(&str, &str)> {
    let command = raw.trim_start();
    let command = command.strip_prefix('/').unwrap_or(command);
    let root_end = command.find(char::is_whitespace).unwrap_or(command.len());
    let root = &command[..root_end];
    if root.is_empty() {
        return None;
    }
    Some((root, command[root_end..].trim_start()))
}

/// Future VM resource and lifecycle controls reserved in the host contract.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimeControls {
    fuel: Option<NonZeroU64>,
    memory_bytes: Option<NonZeroUsize>,
    timeout: Option<Duration>,
    shutdown_requested: bool,
}

impl RuntimeControls {
    /// Create controls with no active limits or shutdown request.
    pub fn unrestricted() -> Self {
        Self::default()
    }

    /// Set the maximum instruction/fuel budget for a future VM invocation.
    pub fn with_fuel(mut self, fuel: NonZeroU64) -> Self {
        self.fuel = Some(fuel);
        self
    }

    /// Set the maximum script memory budget for a future VM invocation.
    pub fn with_memory_bytes(mut self, memory_bytes: NonZeroUsize) -> Self {
        self.memory_bytes = Some(memory_bytes);
        self
    }

    /// Set the wall-clock timeout budget for a future VM invocation.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Mark that the host is requesting cooperative runtime shutdown.
    pub fn with_shutdown_requested(mut self) -> Self {
        self.shutdown_requested = true;
        self
    }

    pub fn fuel(&self) -> Option<NonZeroU64> {
        self.fuel
    }

    pub fn memory_bytes(&self) -> Option<NonZeroUsize> {
        self.memory_bytes
    }

    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    pub fn shutdown_requested(&self) -> bool {
        self.shutdown_requested
    }
}

/// Per-event host context passed into a script runtime invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeContext<'a> {
    controls: &'a RuntimeControls,
    command_limit: NonZeroUsize,
}

impl<'a> RuntimeContext<'a> {
    /// Create a runtime context from host-provided controls and command limit.
    pub fn new(controls: &'a RuntimeControls, command_limit: NonZeroUsize) -> Self {
        let command_limit = NonZeroUsize::new(command_limit.get().min(MAX_SCRIPT_COMMAND_BATCH))
            .expect("script command limit is non-zero");
        Self {
            controls,
            command_limit,
        }
    }

    /// Return the resource and lifecycle controls for this invocation.
    pub fn controls(&self) -> &'a RuntimeControls {
        self.controls
    }

    /// Return the maximum command count this invocation may emit.
    pub fn command_limit(&self) -> NonZeroUsize {
        self.command_limit
    }

    /// Build an empty command batch capped by this context's command limit.
    pub fn command_batch(&self) -> CommandBatch {
        CommandBatch::new(self.command_limit)
    }
}

/// Error returned by a script runtime implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeError {
    Trap { message: String },
    ShutdownRequested,
}

/// Result type returned by script runtime invocations.
pub type RuntimeResult<T> = Result<T, RuntimeError>;

/// Minimal runtime trait implemented by future script engines.
pub trait ScriptRuntime {
    /// Handle one immutable event snapshot and return a bounded command batch.
    fn handle_event(
        &mut self,
        event: &ScriptEvent,
        context: RuntimeContext<'_>,
    ) -> RuntimeResult<CommandBatch>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nonzero(value: usize) -> NonZeroUsize {
        NonZeroUsize::new(value).expect("test value is non-zero")
    }

    fn nonzero_u64(value: u64) -> NonZeroU64 {
        NonZeroU64::new(value).expect("test value is non-zero")
    }

    #[test]
    fn script_boundary_is_bounded_and_preserves_the_first_event() {
        let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
        let first = ScriptEvent::server_started();
        let second = ScriptEvent::server_tick(1);

        boundary.try_enqueue_event(first.clone()).unwrap();

        assert_eq!(
            boundary.try_enqueue_event(second),
            Err(ScriptQueueError::Full)
        );
        assert_eq!(endpoint.recv_event_blocking(), Some(first));
    }

    #[test]
    fn closing_event_admission_rejects_new_events_and_drains_buffered_events() {
        let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
        let buffered = ScriptEvent::server_started();
        boundary.try_enqueue_event(buffered.clone()).unwrap();

        boundary.close_event_admission();

        assert_eq!(
            boundary.try_enqueue_event(ScriptEvent::server_tick(1)),
            Err(ScriptQueueError::Closed)
        );
        assert_eq!(endpoint.recv_event_blocking(), Some(buffered));
        assert_eq!(endpoint.recv_event_blocking(), None);
    }

    #[tokio::test]
    async fn targeted_event_delivery_waits_for_host_consumer_progress() {
        let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
        boundary
            .try_enqueue_event(ScriptEvent::server_started())
            .unwrap();
        let request = ScriptPluginStorageGetRequest::try_new("read", "balance").unwrap();
        let result = ScriptEvent::plugin_storage_get_result("shop", &request, None, None).unwrap();
        use std::future::Future;
        use std::pin::Pin;
        use std::task::{Context, Poll, Waker};

        let mut delivery = Box::pin(boundary.enqueue_targeted_event(result));
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        assert!(matches!(
            Future::poll(Pin::as_mut(&mut delivery), &mut cx),
            Poll::Pending
        ));

        assert!(
            matches!(endpoint.recv_event().await, Some(event) if event.event_name() == "server.started")
        );
        assert!(matches!(
            Future::poll(Pin::as_mut(&mut delivery), &mut cx),
            Poll::Ready(Ok(()))
        ));
        assert!(
            matches!(endpoint.recv_event().await, Some(event) if event.event_name() == "plugin.storage.get_result")
        );
    }

    #[tokio::test]
    async fn targeted_event_delivery_exits_when_host_receiver_is_closed() {
        let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
        drop(endpoint);
        let request = ScriptPluginStorageGetRequest::try_new("read", "balance").unwrap();
        let result = ScriptEvent::plugin_storage_get_result("shop", &request, None, None).unwrap();

        assert!(matches!(
            boundary.enqueue_targeted_event(result).await,
            Err(ScriptQueueError::Closed)
        ));
    }

    #[tokio::test]
    async fn required_event_delivery_waits_for_receiver_capacity_notification() {
        let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
        let first = ScriptEvent::server_started();
        let second = ScriptEvent::server_tick(1);
        boundary.try_enqueue_event(first.clone()).unwrap();
        let (receiver_ready_tx, receiver_ready_rx) = tokio::sync::oneshot::channel();
        let (release_receiver_tx, release_receiver_rx) = tokio::sync::oneshot::channel();
        let (received_tx, received_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            receiver_ready_tx.send(()).unwrap();
            release_receiver_rx.await.unwrap();
            let first = endpoint.recv_event().await;
            let second = endpoint.recv_event().await;
            received_tx.send((first, second)).unwrap();
        });

        let delivery = boundary.enqueue_required_event(second.clone());
        tokio::pin!(delivery);
        tokio::select! {
            result = &mut delivery => panic!("required delivery completed without capacity: {result:?}"),
            result = receiver_ready_rx => result.unwrap(),
        }

        release_receiver_tx.send(()).unwrap();
        delivery.await.unwrap();
        assert_eq!(received_rx.await.unwrap(), (Some(first), Some(second)));
    }

    #[tokio::test]
    async fn required_event_delivery_wakes_closed_when_saturated_receiver_closes() {
        let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
        boundary
            .try_enqueue_event(ScriptEvent::server_started())
            .unwrap();
        let (receiver_ready_tx, receiver_ready_rx) = tokio::sync::oneshot::channel();
        let (close_receiver_tx, close_receiver_rx) = tokio::sync::oneshot::channel();
        let (receiver_closed_tx, receiver_closed_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            receiver_ready_tx.send(()).unwrap();
            close_receiver_rx.await.unwrap();
            drop(endpoint);
            receiver_closed_tx.send(()).unwrap();
        });

        let delivery = boundary.enqueue_required_event(ScriptEvent::server_tick(1));
        tokio::pin!(delivery);
        tokio::select! {
            result = &mut delivery => panic!("required delivery completed before closure: {result:?}"),
            result = receiver_ready_rx => result.unwrap(),
        }

        close_receiver_tx.send(()).unwrap();
        receiver_closed_rx.await.unwrap();
        assert_eq!(delivery.await, Err(ScriptQueueError::Closed));
    }

    #[test]
    fn script_event_queue_errors_stay_below_the_result_size_threshold() {
        assert!(
            std::mem::size_of::<ScriptQueueError>() <= 1,
            "queue errors must contain only bounded state metadata"
        );
    }

    #[test]
    fn event_dtos_are_stable_snapshots_without_host_handles() {
        let event = ScriptEvent::player_joined_with_context(
            ScriptPlayerId::new(42),
            ScriptPlayerContext::new("player-42", "kaiser", false, 0.0, 64.0, 0.0),
        );

        assert!(matches!(
            event.kind(),
            ScriptEventKind::PlayerJoined {
                player_id,
                username,
                ..
            } if *player_id == ScriptPlayerId::new(42) && username == "kaiser"
        ));
        assert_eq!(ScriptPlayerId::new(42).value(), 42);
        assert_eq!(ScriptEntityId::new(99).value(), 99);
    }

    #[test]
    fn player_block_broken_event_is_a_validated_post_commit_snapshot() {
        let context = ScriptPlayerContext::new(
            "123e4567-e89b-12d3-a456-426614174000",
            "kaiser",
            true,
            12.25,
            70.0,
            -4.5,
        );
        let event = ScriptEvent::try_player_block_broken_with_context(
            ScriptPlayerId::new(42),
            context.clone(),
            "minecraft:overworld",
            "minecraft:deepslate/diamond_ore",
            3,
            -64,
            -9,
            ScriptGameMode::Survival,
        )
        .unwrap();

        assert_eq!(event.event_name(), "player.block_broken");
        assert_eq!(event.target_plugin_id(), None);
        assert_eq!(event.validate(), Ok(()));
        assert!(matches!(
            event.kind(),
            ScriptEventKind::PlayerBlockBroken {
                player_id,
                context: event_context,
                dimension,
                block_id,
                x,
                y,
                z,
                game_mode,
            } if *player_id == ScriptPlayerId::new(42)
                && event_context == &context
                && dimension == "minecraft:overworld"
                && block_id == "minecraft:deepslate/diamond_ore"
                && (*x, *y, *z) == (3, -64, -9)
                && *game_mode == ScriptGameMode::Survival
        ));
    }

    #[test]
    fn player_block_broken_rejects_invalid_resource_identifiers() {
        let context = ScriptPlayerContext::new("player-42", "kaiser", false, 0.0, 64.0, 0.0);
        let oversized_block_id = format!("minecraft:{}", "a".repeat(MAX_SCRIPT_RESOURCE_ID_BYTES));
        for (dimension, block_id) in [
            ("overworld", "minecraft:stone"),
            ("minecraft:overworld", "minecraft:Stone"),
            ("minecraft:overworld", oversized_block_id.as_str()),
        ] {
            assert!(
                ScriptEvent::try_player_block_broken_with_context(
                    ScriptPlayerId::new(42),
                    context.clone(),
                    dimension,
                    block_id,
                    0,
                    64,
                    0,
                    ScriptGameMode::Creative,
                )
                .is_err()
            );
        }
    }

    #[test]
    fn player_block_placed_event_is_a_validated_snapshot() {
        let context = ScriptPlayerContext::new(
            "123e4567-e89b-12d3-a456-426614174000",
            "kaiser",
            true,
            12.25,
            70.0,
            -4.5,
        );
        let event = ScriptEvent::try_player_block_placed_with_context(
            ScriptPlayerId::new(42),
            context.clone(),
            "minecraft:overworld",
            "minecraft:oak_log",
            3,
            -64,
            -9,
            ScriptGameMode::Survival,
        )
        .unwrap();

        assert_eq!(event.event_name(), "player.block_placed");
        assert_eq!(event.target_plugin_id(), None);
        assert_eq!(event.validate(), Ok(()));
        assert!(matches!(
            event.kind(),
            ScriptEventKind::PlayerBlockPlaced {
                player_id,
                context: event_context,
                dimension,
                block_id,
                x,
                y,
                z,
                game_mode,
            } if *player_id == ScriptPlayerId::new(42)
                && event_context == &context
                && dimension == "minecraft:overworld"
                && block_id == "minecraft:oak_log"
                && (*x, *y, *z) == (3, -64, -9)
                && *game_mode == ScriptGameMode::Survival
        ));
    }

    #[test]
    fn player_block_placed_rejects_invalid_resource_identifiers() {
        let context = ScriptPlayerContext::new("player-42", "kaiser", false, 0.0, 64.0, 0.0);
        let oversized_block_id = format!("minecraft:{}", "a".repeat(MAX_SCRIPT_RESOURCE_ID_BYTES));
        for (dimension, block_id) in [
            ("overworld", "minecraft:stone"),
            ("minecraft:overworld", "minecraft:Stone"),
            ("minecraft:overworld", oversized_block_id.as_str()),
        ] {
            assert!(
                ScriptEvent::try_player_block_placed_with_context(
                    ScriptPlayerId::new(42),
                    context.clone(),
                    dimension,
                    block_id,
                    0,
                    64,
                    0,
                    ScriptGameMode::Creative,
                )
                .is_err()
            );
        }
    }

    #[test]
    fn block_break_game_modes_are_closed_stable_strings() {
        assert_eq!(ScriptGameMode::Survival.as_str(), "survival");
        assert_eq!(ScriptGameMode::Creative.as_str(), "creative");
        assert_eq!(ScriptGameMode::Adventure.as_str(), "adventure");
    }

    #[test]
    fn player_item_crafted_event_is_a_validated_snapshot_without_integer_caps() {
        let context = ScriptPlayerContext::new(
            "123e4567-e89b-12d3-a456-426614174000",
            "kaiser",
            true,
            12.25,
            70.0,
            -4.5,
        );
        let event = ScriptEvent::try_player_item_crafted_with_context(
            ScriptPlayerId::new(42),
            context.clone(),
            "minecraft:overworld",
            "minecraft:oak_planks",
            u64::from(u32::MAX) + 1,
            u32::MAX,
            ScriptCraftingSource::CraftingTable,
            ScriptGameMode::Adventure,
        )
        .unwrap();

        assert_eq!(event.event_name(), "player.item_crafted");
        assert_eq!(event.target_plugin_id(), None);
        assert_eq!(event.validate(), Ok(()));
        assert!(matches!(
            event.kind(),
            ScriptEventKind::PlayerItemCrafted {
                player_id,
                context: event_context,
                dimension,
                item_id,
                count,
                craft_count,
                source,
                game_mode,
            } if *player_id == ScriptPlayerId::new(42)
                && event_context == &context
                && dimension == "minecraft:overworld"
                && item_id == "minecraft:oak_planks"
                && *count == u64::from(u32::MAX) + 1
                && *craft_count == u32::MAX
                && *source == ScriptCraftingSource::CraftingTable
                && *game_mode == ScriptGameMode::Adventure
        ));
    }

    #[test]
    fn player_item_crafted_rejects_invalid_ids_and_zero_counts() {
        let context = ScriptPlayerContext::new("player-42", "kaiser", false, 0.0, 64.0, 0.0);
        for (dimension, item_id, count, craft_count) in [
            ("overworld", "minecraft:stick", 1, 1),
            ("minecraft:overworld", "minecraft:Stick", 1, 1),
            ("minecraft:overworld", "minecraft:stick", 0, 1),
            ("minecraft:overworld", "minecraft:stick", 1, 0),
        ] {
            assert!(
                ScriptEvent::try_player_item_crafted_with_context(
                    ScriptPlayerId::new(42),
                    context.clone(),
                    dimension,
                    item_id,
                    count,
                    craft_count,
                    ScriptCraftingSource::Inventory,
                    ScriptGameMode::Survival,
                )
                .is_err()
            );
        }
    }

    #[test]
    fn crafting_sources_are_closed_stable_strings() {
        assert_eq!(ScriptCraftingSource::Inventory.as_str(), "inventory");
        assert_eq!(
            ScriptCraftingSource::CraftingTable.as_str(),
            "crafting_table"
        );
    }

    #[test]
    fn queued_player_context_keeps_the_pose_observed_at_publication() {
        let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
        let mut latest_accepted_pose = (12.25, 70.0, -4.5);
        boundary
            .try_enqueue_event(ScriptEvent::player_chat_with_context(
                ScriptPlayerId::new(42),
                "snapshot",
                ScriptPlayerContext::new(
                    "123e4567-e89b-12d3-a456-426614174000",
                    "kaiser",
                    true,
                    latest_accepted_pose.0,
                    latest_accepted_pose.1,
                    latest_accepted_pose.2,
                ),
            ))
            .unwrap();

        latest_accepted_pose = (23.5, 71.0, 8.75);
        assert_eq!(latest_accepted_pose, (23.5, 71.0, 8.75));

        let event = endpoint.recv_event_blocking().unwrap();
        let ScriptEventKind::PlayerChat { context, .. } = event.kind() else {
            panic!("expected player chat event");
        };
        assert_eq!(context.uuid(), "123e4567-e89b-12d3-a456-426614174000");
        assert_eq!(context.username(), "kaiser");
        assert!(context.operator());
        assert_eq!((context.x(), context.y(), context.z()), (12.25, 70.0, -4.5));
    }

    #[test]
    fn command_batch_reports_full_without_dropping_existing_commands() {
        let mut batch = CommandBatch::new(nonzero(1));
        let first = ScriptCommand::BroadcastChatMessage {
            message: "first".to_owned(),
        };
        let second = ScriptCommand::BroadcastChatMessage {
            message: "second".to_owned(),
        };

        batch.try_push(first.clone()).unwrap();

        assert_eq!(
            batch.try_push(second),
            Err(CommandBatchError::Full { limit: nonzero(1) })
        );
        assert_eq!(batch.commands(), &[first]);
    }

    #[test]
    fn command_capabilities_deny_unlisted_console_commands_without_mutating_batch() {
        let mut batch = CommandBatch::new(nonzero(1));
        let denied = ScriptCommand::RunConsoleCommand {
            command: "stop".to_owned(),
        };

        assert_eq!(
            batch.try_push_authorized(denied, &CommandCapabilities::default()),
            Err(CommandBatchError::PermissionDenied {
                capability: ScriptCommandCapabilityKind::RunConsoleCommand,
            })
        );
        assert!(batch.commands().is_empty());

        let raw_denied = ScriptCommand::RunConsoleCommand {
            command: "/stop".to_owned(),
        };
        assert_eq!(
            batch.try_push(raw_denied),
            Err(CommandBatchError::PermissionDenied {
                capability: ScriptCommandCapabilityKind::RunConsoleCommand,
            })
        );
        assert!(batch.commands().is_empty());

        let capabilities = CommandCapabilities::default().allow_console_command_root("time");
        let allowed = ScriptCommand::RunConsoleCommand {
            command: "/time set day".to_owned(),
        };

        assert_eq!(
            allowed.required_capability_kind(),
            Some(ScriptCommandCapabilityKind::RunConsoleCommand)
        );
        batch
            .try_push_authorized(allowed.clone(), &capabilities)
            .unwrap();
        assert_eq!(batch.commands(), std::slice::from_ref(&allowed));

        let extra = ScriptCommand::RunConsoleCommand {
            command: "/time set noon".to_owned(),
        };
        assert_eq!(
            batch.try_push_authorized(extra, &capabilities),
            Err(CommandBatchError::Full { limit: nonzero(1) })
        );
        assert_eq!(batch.commands(), std::slice::from_ref(&allowed));
    }

    #[test]
    fn manifest_validation_rejects_unsupported_requested_api_version() {
        let requested = ScriptApiVersion::new(0, 7, 0);
        let manifest = ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", requested)
            .declare_console_command_root("time");

        assert_eq!(
            manifest.validate(),
            Err(ScriptPluginManifestError::UnsupportedScriptApiVersion {
                requested,
                supported: SCRIPT_API_VERSION,
            })
        );
    }

    #[test]
    fn extended_plugin_contract_is_available_at_0_6_0() {
        assert_eq!(SCRIPT_API_VERSION, ScriptApiVersion::new(0, 6, 0));
        for event_name in [
            "player.block_broken",
            "player.block_placed",
            "player.item_crafted",
            "player.item_picked_up",
            "player.entity_killed",
            "player.died",
            "plugin.storage.get_result",
            "plugin.storage.cas_result",
            "plugin.storage.delete_result",
            "inventory.menu.clicked",
            "inventory.storage_transaction.result",
            "player.zone_entered",
            "player.zone_exited",
            "zone.command_result",
            "player.teleport_result",
            "villager.binding_result",
            "villager.goal_result",
        ] {
            assert!(is_supported_event_name(event_name), "missing {event_name}");
        }

        let manifest = ScriptPluginManifest::new("pickup", "Pickup", "0.1.0", SCRIPT_API_VERSION)
            .subscribe_event(" PLAYER.ITEM_PICKED_UP ")
            .validate()
            .unwrap();
        assert_eq!(
            manifest.event_subscriptions()[0].event_name(),
            "player.item_picked_up"
        );
    }

    #[test]
    fn spawn_entity_capability_is_exact_and_manifest_bounded() {
        let invalid =
            ScriptPluginManifest::new("spawn-test", "Spawn Test", "0.1.0", SCRIPT_API_VERSION)
                .declare_spawn_entity_type("pig");
        assert!(matches!(
            invalid.validate(),
            Err(ScriptPluginManifestError::InvalidSpawnEntityType { .. })
        ));

        for invalid_type in [
            "minecraft:Pig".to_owned(),
            ":pig".to_owned(),
            "minecraft:".to_owned(),
            format!("minecraft:{}", "a".repeat(MAX_SCRIPT_RESOURCE_ID_BYTES)),
        ] {
            let invalid =
                ScriptPluginManifest::new("spawn-test", "Spawn Test", "0.1.0", SCRIPT_API_VERSION)
                    .declare_spawn_entity_type(invalid_type);
            assert!(matches!(
                invalid.validate(),
                Err(ScriptPluginManifestError::InvalidSpawnEntityType { .. })
                    | Err(ScriptPluginManifestError::FieldTooLong {
                        field: "spawn entity type",
                        ..
                    })
            ));
        }

        let duplicate =
            ScriptPluginManifest::new("spawn-test", "Spawn Test", "0.1.0", SCRIPT_API_VERSION)
                .declare_spawn_entity_type("minecraft:pig")
                .declare_spawn_entity_type("minecraft:pig");
        assert!(matches!(
            duplicate.validate(),
            Err(ScriptPluginManifestError::DuplicateSpawnEntityType { .. })
        ));

        let mut bounded =
            ScriptPluginManifest::new("spawn-test", "Spawn Test", "0.1.0", SCRIPT_API_VERSION);
        for index in 0..=MAX_SPAWN_ENTITY_TYPES {
            bounded = bounded.declare_spawn_entity_type(format!("minecraft:test_{index}"));
        }
        assert!(matches!(
            bounded.validate(),
            Err(ScriptPluginManifestError::TooManySpawnEntityTypes { .. })
        ));

        let position = ScriptPosition::try_new(1.25, 64.0, -2.5).unwrap();
        let pig = ScriptCommand::SpawnEntity {
            actor: ScriptPlayerId::new(7),
            entity_type: "minecraft:pig".to_owned(),
            position,
        };
        let cow = ScriptCommand::SpawnEntity {
            actor: ScriptPlayerId::new(7),
            entity_type: "minecraft:cow".to_owned(),
            position,
        };
        let capabilities = CommandCapabilities::none().allow_spawn_entity_type("minecraft:pig");
        let mut batch = CommandBatch::new(nonzero(2));

        assert_eq!(
            pig.required_capability_kind(),
            Some(ScriptCommandCapabilityKind::SpawnEntity)
        );
        batch
            .try_push_authorized(pig.clone(), &capabilities)
            .unwrap();
        assert_eq!(
            batch.try_push_authorized(cow, &capabilities),
            Err(CommandBatchError::PermissionDenied {
                capability: ScriptCommandCapabilityKind::SpawnEntity,
            })
        );
        assert_eq!(batch.commands(), std::slice::from_ref(&pig));
    }

    #[test]
    fn manifest_validation_rejects_duplicate_command_roots_after_normalization() {
        let manifest = ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", SCRIPT_API_VERSION)
            .declare_console_command_root("time")
            .declare_console_command_root("/time set day");

        assert_eq!(
            manifest.validate(),
            Err(ScriptPluginManifestError::DuplicateCommandRoot {
                root: "time".to_owned(),
            })
        );
    }

    #[test]
    fn manifest_validation_rejects_invalid_ids_and_unbounded_command_roots() {
        let blank_id = ScriptPluginManifest::new(" ", "Daytime", "0.1.0", SCRIPT_API_VERSION);
        assert_eq!(
            blank_id.validate(),
            Err(ScriptPluginManifestError::BlankPluginId)
        );

        let invalid_id =
            ScriptPluginManifest::new("Day Time", "Daytime", "0.1.0", SCRIPT_API_VERSION);
        assert_eq!(
            invalid_id.validate(),
            Err(ScriptPluginManifestError::InvalidPluginId {
                plugin_id: "Day Time".to_owned(),
            })
        );

        let unbounded =
            ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", SCRIPT_API_VERSION)
                .declare_console_command_root("*");
        assert_eq!(
            unbounded.validate(),
            Err(ScriptPluginManifestError::UnboundedCommandRoot {
                root: "*".to_owned(),
            })
        );

        let blank = ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", SCRIPT_API_VERSION)
            .declare_console_command_root(" / ");
        assert_eq!(
            blank.validate(),
            Err(ScriptPluginManifestError::BlankCommandRoot)
        );
    }

    #[test]
    fn manifest_validation_accepts_and_deduplicates_safe_player_command_roots() {
        let manifest =
            ScriptPluginManifest::new("greetings", "Greetings", "0.1.0", SCRIPT_API_VERSION)
                .declare_player_command_root("hello")
                .declare_player_command_root("hello")
                .declare_player_command_root("warp_home-2");

        assert_eq!(
            manifest.validate().unwrap().player_command_roots(),
            &["hello".to_owned(), "warp_home-2".to_owned()]
        );
    }

    #[test]
    fn manifest_validation_enforces_player_command_root_byte_limit() {
        let boundary_root = "a".repeat(MAX_PLAYER_COMMAND_ROOT_BYTES);
        let boundary =
            ScriptPluginManifest::new("greetings", "Greetings", "0.1.0", SCRIPT_API_VERSION)
                .declare_player_command_root(&boundary_root);
        assert_eq!(
            boundary.validate().unwrap().player_command_roots(),
            &[boundary_root]
        );

        let over_limit_root = "a".repeat(MAX_PLAYER_COMMAND_ROOT_BYTES + 1);
        let over_limit =
            ScriptPluginManifest::new("greetings", "Greetings", "0.1.0", SCRIPT_API_VERSION)
                .declare_player_command_root(&over_limit_root);
        assert_eq!(
            over_limit.validate(),
            Err(ScriptPluginManifestError::FieldTooLong {
                field: "player command root",
                max_bytes: MAX_PLAYER_COMMAND_ROOT_BYTES,
            })
        );
    }

    #[test]
    fn manifest_validation_rejects_unsafe_and_reserved_player_command_roots() {
        for root in ["Hello", "/hello", "hello there", "hello.world"] {
            let manifest =
                ScriptPluginManifest::new("greetings", "Greetings", "0.1.0", SCRIPT_API_VERSION)
                    .declare_player_command_root(root);

            assert_eq!(
                manifest.validate(),
                Err(ScriptPluginManifestError::InvalidPlayerCommandRoot {
                    root: root.to_owned(),
                })
            );
        }
        assert_eq!(
            ScriptPluginManifest::new("greetings", "Greetings", "0.1.0", SCRIPT_API_VERSION)
                .declare_player_command_root("")
                .validate(),
            Err(ScriptPluginManifestError::EmptyField {
                field: "player command root",
            })
        );

        for root in ["gamemode", "defaultgamemode", "tp", "teleport"] {
            let manifest =
                ScriptPluginManifest::new("greetings", "Greetings", "0.1.0", SCRIPT_API_VERSION)
                    .declare_player_command_root(root);

            assert_eq!(
                manifest.validate(),
                Err(ScriptPluginManifestError::ReservedPlayerCommandRoot {
                    root: root.to_owned(),
                })
            );
        }
    }

    #[test]
    fn player_command_event_is_an_immutable_targeted_snapshot() {
        let event = ScriptEvent::try_player_command_with_context(
            "greetings",
            ScriptPlayerId::new(42),
            ScriptPlayerContext::new("player-42", "Alex", false, 0.0, 64.0, 0.0),
            "hello",
            "one  two ",
        )
        .unwrap();

        assert_eq!(event.event_name(), "player.command");
        assert_eq!(event.target_plugin_id(), Some("greetings"));
        assert!(matches!(
            event.kind(),
            ScriptEventKind::PlayerCommand {
                player_id,
                username,
                root,
                arguments,
                ..
            } if *player_id == ScriptPlayerId::new(42)
                && username == "Alex"
                && root == "hello"
                && arguments == "one  two "
        ));
    }

    #[test]
    fn player_command_boundary_reports_full_and_closed_without_retaining_events() {
        let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
        let manifest =
            ScriptPluginManifest::new("greetings", "Greetings", "0.1.0", SCRIPT_API_VERSION)
                .declare_player_command_root("hello")
                .validate()
                .unwrap();
        endpoint.register_player_commands(&manifest).unwrap();

        assert_eq!(boundary.player_command_roots(), vec!["hello".to_owned()]);
        let context = || ScriptPlayerContext::new("player-7", "Alex", false, 0.0, 64.0, 0.0);
        assert_eq!(
            boundary.try_enqueue_player_command_with_context(
                ScriptPlayerId::new(7),
                context(),
                "missing arg",
            ),
            Ok(PlayerCommandAdmission::NotOwned)
        );
        assert_eq!(
            boundary.try_enqueue_player_command_with_context(
                ScriptPlayerId::new(7),
                context(),
                "/hello one  two ",
            ),
            Ok(PlayerCommandAdmission::Enqueued)
        );
        assert_eq!(
            boundary.try_enqueue_player_command_with_context(
                ScriptPlayerId::new(7),
                context(),
                "hello later",
            ),
            Err(ScriptQueueError::Full)
        );
        assert!(matches!(
            endpoint.recv_event_blocking(),
            Some(ScriptEvent {
                kind: ScriptEventKind::PlayerCommand { username, root, arguments, .. },
                ..
            }) if username == "Alex" && root == "hello" && arguments == "one  two "
        ));
        drop(endpoint);
        assert_eq!(
            boundary.try_enqueue_player_command_with_context(
                ScriptPlayerId::new(7),
                context(),
                "hello after-close",
            ),
            Err(ScriptQueueError::Closed)
        );
        assert_eq!(
            boundary.try_enqueue_player_command_with_context(
                ScriptPlayerId::new(7),
                context(),
                "hello after-clear",
            ),
            Ok(PlayerCommandAdmission::NotOwned)
        );
    }

    #[test]
    fn owned_player_command_arguments_are_bounded_without_panicking_or_queuing_rejections() {
        let (boundary, mut endpoint) = script_boundary_pair(nonzero(4), nonzero(1));
        let manifest = ScriptPluginManifest::new("owner", "Owner", "0.1.0", SCRIPT_API_VERSION)
            .declare_player_command_root("owned")
            .validate()
            .unwrap();
        endpoint.register_player_commands(&manifest).unwrap();
        let context =
            ScriptPlayerContext::try_new("player-7", "Alex", false, 0.0, 64.0, 0.0).unwrap();

        let accepted = format!("owned {}", "a".repeat(MAX_SCRIPT_CHAT_MESSAGE_BYTES));
        assert_eq!(
            boundary.try_enqueue_player_command_with_context(
                ScriptPlayerId::new(7),
                context.clone(),
                &accepted,
            ),
            Ok(PlayerCommandAdmission::Enqueued)
        );
        let event = endpoint.recv_event_blocking().unwrap();
        assert!(matches!(
            event.kind(),
            ScriptEventKind::PlayerCommand { arguments, .. }
                if arguments.len() == MAX_SCRIPT_CHAT_MESSAGE_BYTES
        ));

        for rejected_len in [MAX_SCRIPT_CHAT_MESSAGE_BYTES + 1, 32_767] {
            let rejected = format!("owned {}", "b".repeat(rejected_len));
            assert_eq!(
                boundary.try_enqueue_player_command_with_context(
                    ScriptPlayerId::new(7),
                    context.clone(),
                    &rejected,
                ),
                Ok(PlayerCommandAdmission::OwnedRejected {
                    error: ScriptDtoError::ValueTooLong {
                        field: "player command arguments",
                        max_bytes: MAX_SCRIPT_CHAT_MESSAGE_BYTES,
                        actual_bytes: rejected_len,
                    },
                })
            );

            assert_eq!(
                boundary.try_enqueue_player_command_with_context(
                    ScriptPlayerId::new(7),
                    context.clone(),
                    "owned sentinel",
                ),
                Ok(PlayerCommandAdmission::Enqueued)
            );
            let event = endpoint.recv_event_blocking().unwrap();
            assert!(matches!(
                event.kind(),
                ScriptEventKind::PlayerCommand { arguments, .. } if arguments == "sentinel"
            ));
        }

        let constructor_error = ScriptEvent::try_player_command_with_context(
            "owner",
            ScriptPlayerId::new(7),
            context,
            "owned",
            "x".repeat(MAX_SCRIPT_CHAT_MESSAGE_BYTES + 1),
        )
        .unwrap_err();
        assert_eq!(
            constructor_error,
            ScriptDtoError::ValueTooLong {
                field: "player command arguments",
                max_bytes: MAX_SCRIPT_CHAT_MESSAGE_BYTES,
                actual_bytes: MAX_SCRIPT_CHAT_MESSAGE_BYTES + 1,
            }
        );
    }

    #[test]
    fn operator_player_commands_require_verified_context_and_are_denied_before_a_full_queue() {
        let manifest =
            ScriptPluginManifest::new("admin-day", "Admin Day", "0.1.0", SCRIPT_API_VERSION)
                .declare_operator_command_root("adminday")
                .validate()
                .unwrap();
        assert!(manifest.player_command_roots().is_empty());
        assert_eq!(manifest.operator_command_roots(), ["adminday"]);
        let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
        endpoint.register_player_commands(&manifest).unwrap();
        boundary
            .try_enqueue_event(ScriptEvent::server_started())
            .unwrap();

        assert_eq!(
            boundary.try_enqueue_player_command_with_context(
                ScriptPlayerId::new(7),
                ScriptPlayerContext::new("player-7", "Alex", false, 0.0, 64.0, 0.0),
                "adminday",
            ),
            Ok(PlayerCommandAdmission::PermissionDenied)
        );
        assert_eq!(
            endpoint.recv_event_blocking(),
            Some(ScriptEvent::server_started()),
            "denied command must not be enqueued behind a full queue"
        );
        assert_eq!(
            boundary.try_enqueue_player_command_with_context(
                ScriptPlayerId::new(7),
                ScriptPlayerContext::new("verified-id", "Alex", true, 0.0, 0.0, 0.0),
                "adminday",
            ),
            Ok(PlayerCommandAdmission::Enqueued)
        );
    }

    #[test]
    fn player_command_registration_enforces_aggregate_root_limit_atomically() {
        let mut boundary_manifest =
            ScriptPluginManifest::new("boundary", "Boundary", "0.1.0", SCRIPT_API_VERSION);
        for index in 0..MAX_PLAYER_COMMAND_ROOTS {
            boundary_manifest =
                boundary_manifest.declare_player_command_root(format!("command{index}"));
        }
        let boundary_manifest = boundary_manifest.validate().unwrap();
        let over_limit_manifest =
            ScriptPluginManifest::new("over-limit", "Over Limit", "0.1.0", SCRIPT_API_VERSION)
                .declare_player_command_root("one_more")
                .validate()
                .unwrap();
        let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(1));

        endpoint
            .register_player_commands(&boundary_manifest)
            .unwrap();
        assert_eq!(
            boundary.player_command_roots().len(),
            MAX_PLAYER_COMMAND_ROOTS
        );
        assert_eq!(
            endpoint.register_player_commands(&over_limit_manifest),
            Err(PlayerCommandRegistrationError::RootLimitExceeded {
                limit: MAX_PLAYER_COMMAND_ROOTS,
                requested: MAX_PLAYER_COMMAND_ROOTS + 1,
            })
        );
        assert_eq!(
            boundary.player_command_roots().len(),
            MAX_PLAYER_COMMAND_ROOTS,
            "over-limit registration must not partially mutate ownership"
        );
    }

    #[test]
    fn manifest_validation_normalizes_event_subscriptions_and_dependencies() {
        let manifest = ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", SCRIPT_API_VERSION)
            .with_load_phase(ScriptPluginLoadPhase::Startup)
            .subscribe_event(" Player.Chat ")
            .subscribe_event("server.tick")
            .declare_dependency(" Economy ", ScriptPluginDependencyRelation::Required)
            .declare_dependency("chat-tools", ScriptPluginDependencyRelation::Optional)
            .declare_dependency("spawn-protect", ScriptPluginDependencyRelation::LoadBefore);

        let validated = manifest.validate().unwrap();

        assert_eq!(validated.load_phase(), ScriptPluginLoadPhase::Startup);
        assert_eq!(
            validated.event_subscriptions(),
            &[
                ScriptEventSubscription::new("player.chat".to_owned()),
                ScriptEventSubscription::new("server.tick".to_owned()),
            ]
        );
        assert_eq!(
            validated.dependencies(),
            &[
                ScriptPluginDependency::new(
                    "economy".to_owned(),
                    ScriptPluginDependencyRelation::Required
                ),
                ScriptPluginDependency::new(
                    "chat-tools".to_owned(),
                    ScriptPluginDependencyRelation::Optional
                ),
                ScriptPluginDependency::new(
                    "spawn-protect".to_owned(),
                    ScriptPluginDependencyRelation::LoadBefore,
                ),
            ]
        );
    }

    #[test]
    fn manifest_validation_rejects_unsafe_event_subscriptions() {
        let invalid = ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", SCRIPT_API_VERSION)
            .subscribe_event("player.inventory.clicked");
        assert_eq!(
            invalid.validate(),
            Err(ScriptPluginManifestError::InvalidEventName {
                event_name: "player.inventory.clicked".to_owned(),
            })
        );

        let duplicate =
            ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", SCRIPT_API_VERSION)
                .subscribe_event("player.chat")
                .subscribe_event(" Player.Chat ");
        assert_eq!(
            duplicate.validate(),
            Err(ScriptPluginManifestError::DuplicateEventSubscription {
                event_name: "player.chat".to_owned(),
            })
        );
    }

    #[test]
    fn manifest_validation_rejects_unsafe_dependency_declarations() {
        let blank = ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", SCRIPT_API_VERSION)
            .declare_dependency(" ", ScriptPluginDependencyRelation::Required);
        assert_eq!(
            blank.validate(),
            Err(ScriptPluginManifestError::BlankDependencyPluginId)
        );

        let invalid = ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", SCRIPT_API_VERSION)
            .declare_dependency("Economy Tools", ScriptPluginDependencyRelation::Required);
        assert_eq!(
            invalid.validate(),
            Err(ScriptPluginManifestError::InvalidDependencyPluginId {
                plugin_id: "economy tools".to_owned(),
            })
        );

        let self_dependency =
            ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", SCRIPT_API_VERSION)
                .declare_dependency("Daytime", ScriptPluginDependencyRelation::Required);
        assert_eq!(
            self_dependency.validate(),
            Err(ScriptPluginManifestError::SelfDependency {
                plugin_id: "daytime".to_owned(),
            })
        );

        let duplicate =
            ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", SCRIPT_API_VERSION)
                .declare_dependency("Economy", ScriptPluginDependencyRelation::Required)
                .declare_dependency(" economy ", ScriptPluginDependencyRelation::Optional);
        assert_eq!(
            duplicate.validate(),
            Err(ScriptPluginManifestError::DuplicateDependency {
                plugin_id: "economy".to_owned(),
            })
        );
    }

    #[test]
    fn trusted_host_derives_command_capabilities_from_validated_manifest() {
        let manifest = ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", SCRIPT_API_VERSION)
            .declare_console_command_root("time");
        let validated = manifest.validate().unwrap();
        let capabilities = validated.to_command_capabilities();

        let time_command = ScriptCommand::RunConsoleCommand {
            command: "/time set day".to_owned(),
        };
        let time = time_command.required_capability().unwrap();
        assert!(capabilities.allows(time));
        assert!(
            !capabilities.allows(
                ScriptCommand::RunConsoleCommand {
                    command: "/stop".to_owned(),
                }
                .required_capability()
                .unwrap()
            )
        );
    }

    #[test]
    fn runtime_controls_reserve_fuel_memory_timeout_and_shutdown() {
        let controls = RuntimeControls::unrestricted()
            .with_fuel(nonzero_u64(100))
            .with_memory_bytes(nonzero(4096))
            .with_timeout(Duration::from_millis(50))
            .with_shutdown_requested();

        assert_eq!(controls.fuel(), Some(nonzero_u64(100)));
        assert_eq!(controls.memory_bytes(), Some(nonzero(4096)));
        assert_eq!(controls.timeout(), Some(Duration::from_millis(50)));
        assert!(controls.shutdown_requested());
    }

    #[test]
    fn runtime_contract_accepts_event_reference_and_returns_bounded_batch() {
        struct EchoRuntime;

        impl ScriptRuntime for EchoRuntime {
            fn handle_event(
                &mut self,
                event: &ScriptEvent,
                context: RuntimeContext<'_>,
            ) -> RuntimeResult<CommandBatch> {
                let mut batch = context.command_batch();
                if let ScriptEventKind::PlayerChat {
                    player_id, message, ..
                } = event.kind()
                {
                    batch
                        .try_push(ScriptCommand::SendChatMessage {
                            player_id: *player_id,
                            message: format!("echo: {message}"),
                        })
                        .expect("context-created batch has remaining capacity");
                }
                Ok(batch)
            }
        }

        let controls = RuntimeControls::unrestricted();
        let context = RuntimeContext::new(&controls, nonzero(1));
        let event = ScriptEvent::player_chat_with_context(
            ScriptPlayerId::new(7),
            "hello",
            ScriptPlayerContext::new("player-7", "Alex", false, 0.0, 64.0, 0.0),
        );

        let commands = EchoRuntime
            .handle_event(&event, context)
            .unwrap()
            .into_commands();

        assert_eq!(
            commands,
            vec![ScriptCommand::SendChatMessage {
                player_id: ScriptPlayerId::new(7),
                message: "echo: hello".to_owned(),
            }]
        );
        assert!(matches!(
            event.kind(),
            ScriptEventKind::PlayerChat {
                player_id, message, ..
            } if *player_id == ScriptPlayerId::new(7) && message == "hello"
        ));
    }

    #[test]
    fn plugin_capabilities_reject_undeclared_storage_and_transaction_requests() {
        let mut batch = CommandBatch::new(nonzero(2));
        let storage = ScriptCommand::PluginStorageGet {
            request: ScriptPluginStorageGetRequest::try_new("read-balance", "balance:player-7")
                .unwrap(),
        };
        let transaction = ScriptCommand::InventoryStorageTransaction {
            transaction: ScriptInventoryStorageTransaction::try_new(
                "buy-1",
                ScriptPlayerId::new(7),
                vec![ScriptInventoryResourceDelta::try_new("minecraft:apple", 1).unwrap()],
                vec![
                    ScriptStorageMutation::compare_and_swap("balance:player-7", Some(2), "9")
                        .unwrap(),
                ],
            )
            .unwrap(),
        };

        assert_eq!(
            batch.try_push_authorized(storage, &CommandCapabilities::none()),
            Err(CommandBatchError::PermissionDenied {
                capability: ScriptCommandCapabilityKind::PluginStorage,
            })
        );
        assert_eq!(
            batch.try_push_authorized(transaction, &CommandCapabilities::none()),
            Err(CommandBatchError::PermissionDenied {
                capability: ScriptCommandCapabilityKind::InventoryStorageTransactions,
            })
        );
        assert!(batch.commands().is_empty());
    }

    #[test]
    fn transaction_revalidates_directly_constructed_storage_mutations() {
        for mutation in [
            ScriptStorageMutation::CompareAndSwap {
                key: String::new(),
                expected_version: None,
                value: "value".to_owned(),
            },
            ScriptStorageMutation::CompareAndSwap {
                key: "key".to_owned(),
                expected_version: None,
                value: String::new(),
            },
            ScriptStorageMutation::Delete {
                key: "x".repeat(MAX_PLUGIN_STORAGE_KEY_BYTES + 1),
                expected_version: None,
            },
        ] {
            assert!(
                ScriptInventoryStorageTransaction::try_new(
                    "transaction",
                    ScriptPlayerId::new(7),
                    vec![ScriptInventoryResourceDelta::try_new("minecraft:apple", 1).unwrap(),],
                    vec![mutation],
                )
                .is_err()
            );
        }
    }

    #[test]
    fn public_command_submission_denies_privileged_raw_commands_and_provenance_replay() {
        let (boundary, endpoint) = script_boundary_pair(nonzero(2), nonzero(2));
        assert!(
            endpoint
                .try_submit_command(ScriptCommand::PluginStorageGet {
                    request: ScriptPluginStorageGetRequest::try_new("read", "balance").unwrap(),
                })
                .is_err()
        );

        let replay = ScriptCommand::HostAttached {
            provenance: ScriptCommandProvenance::for_host_plugin(Arc::from("owner"), 1),
            request: Arc::new(ScriptCommand::BroadcastChatMessage {
                message: "owned".to_owned(),
            }),
        };
        let mut forged_batch = CommandBatch::new(nonzero(1));
        assert_eq!(
            forged_batch.try_push(replay.clone()),
            Err(CommandBatchError::ProvenanceRejected)
        );
        assert!(endpoint.try_submit_command(replay).is_err());
        assert!(matches!(
            boundary.command_rx.try_lock().unwrap().try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn sealed_admission_rechecks_manifest_capabilities_and_rejects_the_whole_batch() {
        let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(2));
        let manifest = ScriptPluginManifest::new("plain", "Plain", "0.1.0", SCRIPT_API_VERSION)
            .validate()
            .unwrap();
        let admission = HostCommandAdmission::from_manifest(&manifest);
        let forged_capabilities = CommandCapabilities::none().allow_plugin_storage();
        let mut batch = CommandBatch::new(nonzero(1));
        batch
            .try_push_authorized(
                ScriptCommand::PluginStorageGet {
                    request: ScriptPluginStorageGetRequest::try_new("read", "balance").unwrap(),
                },
                &forged_capabilities,
            )
            .unwrap();

        assert!(matches!(
            endpoint.try_submit_plugin_batch(&admission, batch),
            Err(ScriptBatchSubmissionError::Rejected {
                error: CommandBatchError::PermissionDenied {
                    capability: ScriptCommandCapabilityKind::PluginStorage,
                },
                ..
            })
        ));
        assert!(matches!(
            boundary.command_rx.try_lock().unwrap().try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn saturated_command_queue_rejects_an_atomic_batch_without_publishing_a_prefix() {
        let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(2));
        let existing = ScriptCommand::BroadcastChatMessage {
            message: "existing".to_owned(),
        };
        endpoint.try_submit_command(existing.clone()).unwrap();

        let manifest = ScriptPluginManifest::new("shop", "Shop", "0.1.0", SCRIPT_API_VERSION)
            .declare_plugin_storage()
            .validate()
            .unwrap();
        let admission = HostCommandAdmission::from_manifest(&manifest);
        let capabilities = manifest.to_command_capabilities();
        let mut batch = CommandBatch::new(nonzero(2));
        for request_id in ["first", "second"] {
            batch
                .try_push_authorized(
                    ScriptCommand::PluginStorageGet {
                        request: ScriptPluginStorageGetRequest::try_new(request_id, "balance")
                            .unwrap(),
                    },
                    &capabilities,
                )
                .unwrap();
        }

        assert!(matches!(
            endpoint.try_submit_plugin_batch(&admission, batch),
            Err(ScriptBatchSubmissionError::Full(rejected))
                if rejected.commands().len() == 2
        ));
        let mut command_rx = boundary.command_rx.try_lock().unwrap();
        assert_eq!(command_rx.try_recv(), Ok(existing));
        assert_eq!(command_rx.try_recv(), Err(mpsc::error::TryRecvError::Empty));
    }

    #[test]
    fn bounded_dtos_reject_oversized_ids_values_invalid_bounds_and_resources() {
        let oversized_id = "x".repeat(MAX_SCRIPT_ID_BYTES + 1);
        assert!(matches!(
            ScriptAxisAlignedZone::try_new(
                &oversized_id,
                "minecraft:overworld",
                ScriptPosition::try_new(0.0, 0.0, 0.0).unwrap(),
                ScriptPosition::try_new(1.0, 1.0, 1.0).unwrap(),
            ),
            Err(ScriptDtoError::ValueTooLong { .. })
        ));
        assert!(matches!(
            ScriptStorageMutation::compare_and_swap(
                "balance",
                None,
                "x".repeat(MAX_PLUGIN_STORAGE_VALUE_BYTES + 1),
            ),
            Err(ScriptDtoError::ValueTooLong { .. })
        ));
        assert!(matches!(
            ScriptAxisAlignedZone::try_new(
                "shop",
                "minecraft:Overworld",
                ScriptPosition::try_new(2.0, 0.0, 0.0).unwrap(),
                ScriptPosition::try_new(1.0, 1.0, 1.0).unwrap(),
            ),
            Err(ScriptDtoError::InvalidBounds)
        ));
        assert!(matches!(
            ScriptInventoryMenuItem::try_new("invalid", 1, None),
            Err(ScriptDtoError::InvalidResourceId { .. })
        ));
        assert!(matches!(
            ScriptVillagerBindingRequest::try_new(
                "bind",
                ScriptPosition::try_new(0.0, 64.0, 0.0).unwrap(),
                MAX_VILLAGER_BINDING_RADIUS + 1.0,
            ),
            Err(ScriptDtoError::InvalidBounds)
        ));
    }

    #[test]
    fn zone_protection_is_explicit_normalized_and_plugin_agnostic() {
        let protection =
            ScriptZoneProtection::try_actor_or_operator("12345678-1234-5678-1234-567812345678")
                .unwrap();
        assert_eq!(
            protection.allowed_actor_uuid(),
            "12345678123456781234567812345678"
        );
        assert!(protection.allows_actor("12345678123456781234567812345678", false));
        assert!(protection.allows_actor("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", true));
        assert!(!protection.allows_actor("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", false));
        assert!(matches!(
            ScriptZoneProtection::try_actor_or_operator("claim-owner"),
            Err(ScriptDtoError::InvalidId {
                field: "protection actor uuid",
                ..
            })
        ));
    }

    #[test]
    fn targeted_results_validate_correlation_fields_and_coherent_outcomes() {
        assert!(matches!(
            ScriptPluginStorageGetRequest::try_new("", "balance"),
            Err(ScriptDtoError::EmptyValue { .. })
        ));
        assert!(matches!(
            ScriptPluginStorageCompareAndSwapRequest::try_new("write", "balance", None, ""),
            Err(ScriptDtoError::EmptyValue { .. })
        ));
        let get = ScriptPluginStorageGetRequest::try_new("read", "balance").unwrap();
        let cas =
            ScriptPluginStorageCompareAndSwapRequest::try_new("write", "balance", Some(3), "9")
                .unwrap();
        let delete =
            ScriptPluginStorageDeleteRequest::try_new("delete", "balance", Some(3)).unwrap();
        assert!(matches!(
            ScriptEvent::plugin_storage_get_result("owner", &get, Some("9".into()), None),
            Err(ScriptDtoError::InconsistentResult { .. })
        ));
        assert!(matches!(
            ScriptEvent::plugin_storage_get_result(
                "owner",
                &get,
                Some("x".repeat(MAX_PLUGIN_STORAGE_VALUE_BYTES + 1)),
                Some(1),
            ),
            Err(ScriptDtoError::ValueTooLong { .. })
        ));
        assert!(matches!(
            ScriptEvent::plugin_storage_cas_result("owner", &cas, true, None),
            Err(ScriptDtoError::InconsistentResult { .. })
        ));
        assert!(matches!(
            ScriptEvent::plugin_storage_delete_result("owner", &delete, true, None),
            Err(ScriptDtoError::InconsistentResult { .. })
        ));
        let binding_request = ScriptVillagerBindingRequest::try_new(
            "bind",
            ScriptPosition::try_new(0.0, 64.0, 0.0).unwrap(),
            16.0,
        )
        .unwrap();
        assert!(matches!(
            ScriptEvent::villager_binding_result(
                "invalid owner",
                &binding_request,
                None,
                Some(ScriptVillagerBindingFailure::NotFound),
            ),
            Err(ScriptDtoError::InvalidId { .. })
        ));
        assert!(matches!(
            ScriptEvent::villager_binding_result("owner", &binding_request, None, None),
            Err(ScriptDtoError::InvalidBounds)
        ));

        let event =
            ScriptEvent::plugin_storage_get_result("owner.plugin", &get, Some("9".into()), Some(4))
                .unwrap();
        assert_eq!(event.target_plugin_id(), Some("owner.plugin"));
        assert!(matches!(
            event.kind(),
            ScriptEventKind::PluginStorageGetResult {
                request_id,
                key,
                value: Some(value),
                version: Some(4),
                failure: None,
            } if request_id == "read" && key == "balance" && value == "9"
        ));
    }

    #[test]
    fn villager_goal_requests_are_bounded_and_keep_domain_vocabulary_out_of_rust() {
        let idle = ScriptVillagerGoalRequest::try_new(
            "goal-idle",
            "binding-1",
            ScriptVillagerGoal::idle(),
        )
        .unwrap();
        assert_eq!(idle.request_id(), "goal-idle");
        assert_eq!(idle.binding_token(), "binding-1");
        assert_eq!(idle.goal().kind(), "idle");
        assert_eq!(idle.goal().target(), None);
        assert_eq!(idle.goal().speed(), None);

        let target = ScriptPosition::try_new(8.5, 64.0, -3.5).unwrap();
        let moving = ScriptVillagerGoalRequest::try_new(
            "goal-home",
            "binding-2",
            ScriptVillagerGoal::follow_position(target, 0.3).unwrap(),
        )
        .unwrap();
        assert_eq!(moving.goal().kind(), "follow_position");
        assert_eq!(moving.goal().target(), Some(target));
        assert_eq!(moving.goal().speed(), Some(0.3));

        assert!(matches!(
            ScriptVillagerGoal::follow_position(target, 0.0),
            Err(ScriptDtoError::InvalidBounds)
        ));
        assert!(matches!(
            ScriptVillagerGoal::follow_position(target, MAX_VILLAGER_GOAL_SPEED + 0.1),
            Err(ScriptDtoError::InvalidBounds)
        ));
        for rejected in [
            ScriptVillagerGoalRequest::try_new("Goal", "binding-1", ScriptVillagerGoal::idle()),
            ScriptVillagerGoalRequest::try_new("goal", "Binding-1", ScriptVillagerGoal::idle()),
            ScriptVillagerGoalRequest::try_new(
                "x".repeat(MAX_SCRIPT_ID_BYTES + 1),
                "binding-1",
                ScriptVillagerGoal::idle(),
            ),
            ScriptVillagerGoalRequest::try_new(
                "goal",
                "x".repeat(MAX_SCRIPT_ID_BYTES + 1),
                ScriptVillagerGoal::idle(),
            ),
        ] {
            assert!(matches!(
                rejected,
                Err(ScriptDtoError::InvalidId { .. } | ScriptDtoError::ValueTooLong { .. })
            ));
        }
    }

    #[test]
    fn inventory_menu_and_transaction_reject_duplicate_ids_and_saturation() {
        let item = ScriptInventoryMenuItem::try_new("minecraft:apple", 1, None).unwrap();
        assert!(matches!(
            ScriptInventoryMenu::try_new(
                "catalog",
                "Catalog",
                vec![
                    ScriptInventoryMenuSlot::new(0, item.clone()),
                    ScriptInventoryMenuSlot::new(0, item),
                ],
            ),
            Err(ScriptDtoError::DuplicateId { .. })
        ));

        let mut inventory = Vec::new();
        for index in 0..=MAX_INVENTORY_STORAGE_MUTATIONS {
            inventory.push(
                ScriptInventoryResourceDelta::try_new(format!("minecraft:item_{index}"), 1)
                    .unwrap(),
            );
        }
        assert!(matches!(
            ScriptInventoryStorageTransaction::try_new(
                "saturated",
                ScriptPlayerId::new(7),
                inventory,
                vec![ScriptStorageMutation::compare_and_swap("balance", None, "0").unwrap()],
            ),
            Err(ScriptDtoError::TooManyEntries { .. })
        ));
        let duplicate = ScriptInventoryStorageTransaction::try_new(
            "duplicate",
            ScriptPlayerId::new(7),
            vec![
                ScriptInventoryResourceDelta::try_new("minecraft:apple", 1).unwrap(),
                ScriptInventoryResourceDelta::try_new("minecraft:apple", -1).unwrap(),
            ],
            vec![ScriptStorageMutation::compare_and_swap("balance", None, "0").unwrap()],
        );
        assert!(matches!(duplicate, Err(ScriptDtoError::DuplicateId { .. })));
    }

    #[tokio::test]
    async fn host_attaches_plugin_provenance_to_lua_command_submissions() {
        let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
        let manifest = ScriptPluginManifest::new("shop", "Shop", "0.1.0", SCRIPT_API_VERSION)
            .declare_plugin_storage()
            .validate()
            .unwrap();
        let admission = HostCommandAdmission::from_manifest(&manifest);
        let mut batch = CommandBatch::new(nonzero(1));
        batch
            .try_push_authorized(
                ScriptCommand::PluginStorageGet {
                    request: ScriptPluginStorageGetRequest::try_new("read", "balance:player-7")
                        .unwrap(),
                },
                &manifest.to_command_capabilities(),
            )
            .unwrap();
        endpoint.try_submit_plugin_batch(&admission, batch).unwrap();

        let Some(ScriptCommand::HostAttached {
            provenance,
            request,
        }) = boundary.recv_command().await
        else {
            panic!("host-attached command missing");
        };
        assert_eq!(provenance.plugin_id(), "shop");
        assert!(matches!(*request, ScriptCommand::PluginStorageGet { .. }));
    }

    #[test]
    fn raw_command_admission_rejects_every_unbounded_string_variant() {
        let (_boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(8));
        for command in [
            ScriptCommand::SendChatMessage {
                player_id: ScriptPlayerId::new(1),
                message: "x".repeat(MAX_SCRIPT_CHAT_MESSAGE_BYTES + 1),
            },
            ScriptCommand::BroadcastChatMessage {
                message: "x".repeat(MAX_SCRIPT_CHAT_MESSAGE_BYTES + 1),
            },
            ScriptCommand::DisconnectPlayer {
                player_id: ScriptPlayerId::new(1),
                reason: "x".repeat(MAX_SCRIPT_DISCONNECT_REASON_BYTES + 1),
            },
            ScriptCommand::RunConsoleCommand {
                command: "x".repeat(MAX_SCRIPT_CONSOLE_COMMAND_BYTES + 1),
            },
            ScriptCommand::SpawnEntity {
                actor: ScriptPlayerId::new(1),
                entity_type: "x".repeat(MAX_SCRIPT_RESOURCE_ID_BYTES + 1),
                position: ScriptPosition::try_new(0.0, 64.0, 0.0).unwrap(),
            },
            ScriptCommand::CloseInventoryMenu {
                player_id: ScriptPlayerId::new(1),
                menu_id: "x".repeat(MAX_SCRIPT_ID_BYTES + 1),
            },
            ScriptCommand::RemoveZone {
                zone_id: "x".repeat(MAX_SCRIPT_ID_BYTES + 1),
            },
        ] {
            assert!(matches!(
                endpoint.try_submit_command(command),
                Err(ScriptCommandSubmissionError::InvalidCommand { .. })
            ));
        }
    }

    #[test]
    fn public_validation_errors_do_not_retain_or_render_rejected_input() {
        let rejected = "secret".repeat(200_000);
        let dto_error = ScriptPluginStorageGetRequest::try_new(&rejected, "balance").unwrap_err();
        let dto_debug = format!("{dto_error:?}");
        assert!(dto_debug.len() < 256);
        assert!(!dto_debug.contains("secret"));

        let (_boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
        let command_error = endpoint
            .try_submit_command(ScriptCommand::BroadcastChatMessage { message: rejected })
            .unwrap_err();
        let command_debug = format!("{command_error:?}");
        assert!(command_debug.len() < 256);
        assert!(!command_debug.contains("secret"));

        let denied_root = "secret_console_root";
        let denied = endpoint
            .try_submit_command(ScriptCommand::RunConsoleCommand {
                command: denied_root.to_owned(),
            })
            .unwrap_err();
        assert_eq!(
            denied,
            ScriptCommandSubmissionError::PermissionDenied {
                capability: ScriptCommandCapabilityKind::RunConsoleCommand,
            }
        );
        assert!(!format!("{denied:?}").contains(denied_root));

        let denied_entity = "minecraft:secret_entity";
        let mut batch = CommandBatch::new(nonzero(1));
        let denied = batch
            .try_push_authorized(
                ScriptCommand::SpawnEntity {
                    actor: ScriptPlayerId::new(1),
                    entity_type: denied_entity.to_owned(),
                    position: ScriptPosition::try_new(0.0, 64.0, 0.0).unwrap(),
                },
                &CommandCapabilities::none(),
            )
            .unwrap_err();
        assert_eq!(
            denied,
            CommandBatchError::PermissionDenied {
                capability: ScriptCommandCapabilityKind::SpawnEntity,
            }
        );
        assert!(!format!("{denied:?}").contains(denied_entity));
    }

    #[test]
    fn manifest_preflight_rejects_oversized_identity_and_collections() {
        assert!(matches!(
            ScriptPluginManifest::new(
                "x".repeat(MAX_PLUGIN_ID_BYTES + 1),
                "Plugin",
                "0.1.0",
                SCRIPT_API_VERSION,
            )
            .validate(),
            Err(ScriptPluginManifestError::FieldTooLong {
                field: "plugin id",
                ..
            })
        ));

        let mut manifest =
            ScriptPluginManifest::new("bounded", "Plugin", "0.1.0", SCRIPT_API_VERSION);
        for index in 0..=MAX_MANIFEST_EVENT_SUBSCRIPTIONS {
            manifest = manifest.subscribe_event(format!("event.{index}"));
        }
        assert!(matches!(
            manifest.validate(),
            Err(ScriptPluginManifestError::TooManyEntries {
                field: "event subscriptions",
                ..
            })
        ));
    }

    #[tokio::test]
    async fn host_admission_ticket_is_exact_and_one_shot() {
        let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
        let manifest = ScriptPluginManifest::new("shop", "Shop", "0.1.0", SCRIPT_API_VERSION)
            .declare_plugin_storage()
            .validate()
            .unwrap();
        let admission = HostCommandAdmission::from_manifest(&manifest);
        let mut batch = CommandBatch::new(nonzero(1));
        batch
            .try_push_authorized(
                ScriptCommand::PluginStorageGet {
                    request: ScriptPluginStorageGetRequest::try_new("read", "balance").unwrap(),
                },
                &manifest.to_command_capabilities(),
            )
            .unwrap();
        endpoint.try_submit_plugin_batch(&admission, batch).unwrap();

        let command = boundary.recv_command().await.unwrap();
        let replay = command.clone();
        let admitted = boundary.accept_host_command(command).unwrap();
        assert_eq!(admitted.plugin_id(), "shop");
        let result = admitted
            .plugin_storage_get_result(Some("9"), Some(1))
            .unwrap();
        assert_eq!(result.target_plugin_id(), Some("shop"));
        assert!(matches!(
            boundary.accept_host_command(replay),
            Err(ScriptCommandAcceptanceError::UnknownOrConsumed)
        ));
    }

    #[tokio::test]
    async fn admitted_villager_goal_builds_one_targeted_result() {
        let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
        let manifest =
            ScriptPluginManifest::new("settlement", "Settlement", "0.1.0", SCRIPT_API_VERSION)
                .declare_villagers()
                .validate()
                .unwrap();
        let admission = HostCommandAdmission::from_manifest(&manifest);
        let request =
            ScriptVillagerGoalRequest::try_new("goal-1", "binding-1", ScriptVillagerGoal::idle())
                .unwrap();
        let command = ScriptCommand::SetVillagerGoal { request };
        assert_eq!(
            command.required_capability_kind(),
            Some(ScriptCommandCapabilityKind::Villagers)
        );
        let mut batch = CommandBatch::new(nonzero(1));
        batch
            .try_push_authorized(command, &manifest.to_command_capabilities())
            .unwrap();
        endpoint.try_submit_plugin_batch(&admission, batch).unwrap();

        let raw = boundary.recv_command().await.unwrap();
        assert!(matches!(raw, ScriptCommand::HostAttached { .. }));
        let admitted = boundary.accept_host_command(raw).unwrap();
        let result = admitted.villager_goal_result(None).unwrap();
        assert_eq!(result.target_plugin_id(), Some("settlement"));
        assert_eq!(result.event_name(), "villager.goal_result");
        assert!(matches!(
            result.kind(),
            ScriptEventKind::VillagerGoalResult {
                request_id,
                goal: ScriptVillagerGoal::Idle,
                failure: None,
            } if request_id == "goal-1"
        ));
    }

    #[test]
    fn villager_goal_requires_declared_villagers_capability() {
        let command = ScriptCommand::SetVillagerGoal {
            request: ScriptVillagerGoalRequest::try_new(
                "goal-1",
                "binding-1",
                ScriptVillagerGoal::idle(),
            )
            .unwrap(),
        };
        let mut batch = CommandBatch::new(nonzero(1));
        assert_eq!(
            batch.try_push_authorized(command, &CommandCapabilities::default()),
            Err(CommandBatchError::PermissionDenied {
                capability: ScriptCommandCapabilityKind::Villagers,
            })
        );
        assert!(batch.commands().is_empty());
    }

    #[tokio::test]
    async fn admitted_storage_failures_are_explicit_for_every_request_variant() {
        let commands = [
            ScriptCommand::PluginStorageGet {
                request: ScriptPluginStorageGetRequest::try_new("read", "balance").unwrap(),
            },
            ScriptCommand::PluginStorageCompareAndSwap {
                request: ScriptPluginStorageCompareAndSwapRequest::try_new(
                    "write", "balance", None, "9",
                )
                .unwrap(),
            },
            ScriptCommand::PluginStorageDelete {
                request: ScriptPluginStorageDeleteRequest::try_new("delete", "balance", Some(1))
                    .unwrap(),
            },
        ];

        for (index, command) in commands.into_iter().enumerate() {
            let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
            let manifest = ScriptPluginManifest::new("shop", "Shop", "0.1.0", SCRIPT_API_VERSION)
                .declare_plugin_storage()
                .validate()
                .unwrap();
            let admission = HostCommandAdmission::from_manifest(&manifest);
            let mut batch = CommandBatch::new(nonzero(1));
            batch
                .try_push_authorized(command, &manifest.to_command_capabilities())
                .unwrap();
            endpoint.try_submit_plugin_batch(&admission, batch).unwrap();

            let admitted = boundary
                .accept_host_command(boundary.recv_command().await.unwrap())
                .unwrap();
            let event = admitted
                .plugin_storage_failure_result(ScriptPluginStorageFailure::Unavailable)
                .unwrap();
            assert_eq!(event.target_plugin_id(), Some("shop"));
            assert!(matches!(
                (index, event.kind()),
                (
                    0,
                    ScriptEventKind::PluginStorageGetResult {
                        value: None,
                        version: None,
                        failure: Some(ScriptPluginStorageFailure::Unavailable),
                        ..
                    },
                ) | (
                    1,
                    ScriptEventKind::PluginStorageCasResult {
                        applied: false,
                        version: None,
                        failure: Some(ScriptPluginStorageFailure::Unavailable),
                        ..
                    },
                ) | (
                    2,
                    ScriptEventKind::PluginStorageDeleteResult {
                        deleted: false,
                        version: None,
                        failure: Some(ScriptPluginStorageFailure::Unavailable),
                        ..
                    },
                )
            ));
        }
    }

    #[tokio::test]
    async fn host_admission_rejects_request_substitution_and_consumes_the_ticket() {
        let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
        let manifest = ScriptPluginManifest::new("shop", "Shop", "0.1.0", SCRIPT_API_VERSION)
            .declare_plugin_storage()
            .validate()
            .unwrap();
        let admission = HostCommandAdmission::from_manifest(&manifest);
        let mut batch = CommandBatch::new(nonzero(1));
        batch
            .try_push_authorized(
                ScriptCommand::PluginStorageGet {
                    request: ScriptPluginStorageGetRequest::try_new("read", "balance").unwrap(),
                },
                &manifest.to_command_capabilities(),
            )
            .unwrap();
        endpoint.try_submit_plugin_batch(&admission, batch).unwrap();
        let original = boundary.recv_command().await.unwrap();
        let ScriptCommand::HostAttached { provenance, .. } = original.clone() else {
            panic!("host-attached command missing");
        };
        let substituted = ScriptCommand::HostAttached {
            provenance,
            request: Arc::new(ScriptCommand::PluginStorageGet {
                request: ScriptPluginStorageGetRequest::try_new("read", "balance").unwrap(),
            }),
        };

        assert_eq!(
            boundary.accept_host_command(substituted),
            Err(ScriptCommandAcceptanceError::RequestMismatch)
        );
        assert_eq!(
            boundary.accept_host_command(original),
            Err(ScriptCommandAcceptanceError::UnknownOrConsumed)
        );
    }

    #[tokio::test]
    async fn unconsumed_host_tickets_are_bounded_and_recover_after_exact_acceptance() {
        let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(256));
        let manifest = ScriptPluginManifest::new("bounded", "Bounded", "0.1.0", SCRIPT_API_VERSION)
            .validate()
            .unwrap();
        let admission = HostCommandAdmission::from_manifest(&manifest);
        let mut unconsumed = Vec::new();
        for batch_index in 0..8 {
            let mut batch = CommandBatch::new(nonzero(32));
            for command_index in 0..32 {
                batch
                    .try_push(ScriptCommand::BroadcastChatMessage {
                        message: format!("{batch_index}:{command_index}"),
                    })
                    .unwrap();
            }
            endpoint.try_submit_plugin_batch(&admission, batch).unwrap();
            for _ in 0..32 {
                unconsumed.push(boundary.recv_command().await.unwrap());
            }
        }

        let mut overflow = CommandBatch::new(nonzero(1));
        overflow
            .try_push(ScriptCommand::BroadcastChatMessage {
                message: "overflow".to_owned(),
            })
            .unwrap();
        assert!(matches!(
            endpoint.try_submit_plugin_batch(&admission, overflow),
            Err(ScriptBatchSubmissionError::Rejected {
                error: CommandBatchError::AdmissionUnavailable,
                ..
            })
        ));

        boundary
            .accept_host_command(unconsumed.pop().unwrap())
            .unwrap();
        let mut recovered = CommandBatch::new(nonzero(1));
        recovered
            .try_push(ScriptCommand::BroadcastChatMessage {
                message: "recovered".to_owned(),
            })
            .unwrap();
        endpoint
            .try_submit_plugin_batch(&admission, recovered)
            .unwrap();
    }

    #[test]
    fn poisoned_player_command_authority_is_cleared_and_permanently_disabled() {
        let (_boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
        let manifest = ScriptPluginManifest::new("owner", "Owner", "0.1.0", SCRIPT_API_VERSION)
            .declare_player_command_root("owned")
            .validate()
            .unwrap();
        endpoint.register_player_commands(&manifest).unwrap();
        let owners = endpoint.player_command_owners.clone();
        std::thread::spawn(move || {
            let _guard = owners.owners.write().unwrap();
            panic!("poison player-command authority");
        })
        .join()
        .unwrap_err();

        assert!(endpoint.player_command_owners.roots(false).is_empty());
        assert_eq!(
            endpoint.register_player_commands(&manifest),
            Err(PlayerCommandRegistrationError::AuthorityPoisoned)
        );
        assert!(endpoint.player_command_owners.owner("owned").is_none());
    }

    #[test]
    fn player_context_rejects_oversized_identity_and_nonfinite_coordinates() {
        assert!(matches!(
            ScriptPlayerContext::try_new(
                "x".repeat(MAX_SCRIPT_PLAYER_UUID_BYTES + 1),
                "Alex",
                false,
                0.0,
                64.0,
                0.0,
            ),
            Err(ScriptDtoError::ValueTooLong {
                field: "player uuid",
                ..
            })
        ));
        assert!(matches!(
            ScriptPlayerContext::try_new("uuid", "Alex", false, f64::NAN, 64.0, 0.0),
            Err(ScriptDtoError::InvalidBounds)
        ));
    }

    #[test]
    fn script_api_version_requires_the_current_contract_version() {
        assert_eq!(SCRIPT_API_VERSION, ScriptApiVersion::new(0, 6, 0));
        assert!(supports_script_api_version(SCRIPT_API_VERSION));
        for requested in [
            ScriptApiVersion::new(0, 0, 0),
            ScriptApiVersion::new(0, 4, 9),
            ScriptApiVersion::new(0, 5, 1),
            ScriptApiVersion::new(0, 6, 1),
            ScriptApiVersion::new(0, 7, 0),
            ScriptApiVersion::new(1, 0, 0),
        ] {
            assert!(!supports_script_api_version(requested));
            assert_eq!(
                ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", requested).validate(),
                Err(ScriptPluginManifestError::UnsupportedScriptApiVersion {
                    requested,
                    supported: SCRIPT_API_VERSION,
                })
            );
        }
    }
}
