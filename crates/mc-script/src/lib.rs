//! # mc-script
//!
//! Safe script runtime contracts and the built-in Lua plugin host.
//!
//! Immutable event snapshots enter runtimes and bounded command batches leave
//! them. The optional `lua-runtime` feature adds an isolated Lua VM per plugin on
//! one dedicated host thread, with fixed memory and instruction limits.

use std::collections::BTreeMap;
use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::sync::{Mutex, mpsc};

#[cfg(feature = "lua-runtime")]
mod lua;

#[cfg(feature = "lua-runtime")]
pub use lua::{LuaHost, LuaHostConfig, LuaHostError, start_lua_host};

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Semantic version of the stable script API contract.
pub const SCRIPT_API_VERSION: ScriptApiVersion = ScriptApiVersion::new(0, 5, 0);

/// First script API version that exposes allow-listed entity spawns.
pub const ENTITY_SPAWN_API_VERSION: ScriptApiVersion = ScriptApiVersion::new(0, 5, 0);

/// First script API version that exposes plugin-owned player commands.
pub const PLAYER_COMMANDS_API_VERSION: ScriptApiVersion = ScriptApiVersion::new(0, 3, 0);

/// First script API version that exposes operator-only plugin player commands.
pub const OPERATOR_COMMANDS_API_VERSION: ScriptApiVersion = ScriptApiVersion::new(0, 4, 0);

/// Maximum entity types one plugin may allow-list for spawning.
pub const MAX_SPAWN_ENTITY_TYPES: usize = 32;

/// Maximum byte length of a script-visible namespaced resource identifier.
pub const MAX_SCRIPT_RESOURCE_ID_BYTES: usize = 128;

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

/// Result of admitting a player command to a plugin-owned root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlayerCommandAdmission {
    NotOwned,
    Enqueued,
    Dropped,
    PermissionDenied,
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

    pub const fn is_supported_by(&self, host: Self) -> bool {
        if self.major != host.major {
            return false;
        }
        if self.minor < host.minor {
            return true;
        }
        self.minor == host.minor && self.patch <= host.patch
    }
}

pub const fn supports_script_api_version(requested: ScriptApiVersion) -> bool {
    requested.is_supported_by(SCRIPT_API_VERSION)
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

/// Immutable server-authoritative player context attached to gameplay events.
///
/// This is a point-in-time value. It deliberately contains no connection or
/// network-address data and cannot be used to query live server state. Legacy
/// event constructors retain their signatures but carry unavailable context;
/// callers must not treat missing values as server authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPlayerContext {
    snapshot: Option<Box<ScriptPlayerContextSnapshot>>,
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
    #[must_use]
    pub fn new(
        uuid: impl Into<String>,
        username: impl Into<String>,
        operator: bool,
        x: f64,
        y: f64,
        z: f64,
    ) -> Self {
        Self {
            snapshot: Some(Box::new(ScriptPlayerContextSnapshot {
                uuid: uuid.into(),
                username: username.into(),
                operator,
                x_bits: x.to_bits(),
                y_bits: y.to_bits(),
                z_bits: z.to_bits(),
            })),
        }
    }

    #[must_use]
    pub fn is_verified(&self) -> bool {
        self.snapshot.is_some()
    }

    #[must_use]
    pub fn uuid(&self) -> Option<&str> {
        self.snapshot
            .as_deref()
            .map(|snapshot| snapshot.uuid.as_str())
    }

    #[must_use]
    pub fn username(&self) -> Option<&str> {
        self.snapshot
            .as_deref()
            .map(|snapshot| snapshot.username.as_str())
    }

    #[must_use]
    pub fn operator(&self) -> Option<bool> {
        self.snapshot.as_deref().map(|snapshot| snapshot.operator)
    }

    #[must_use]
    pub fn x(&self) -> Option<f64> {
        self.snapshot
            .as_deref()
            .map(|snapshot| f64::from_bits(snapshot.x_bits))
    }

    #[must_use]
    pub fn y(&self) -> Option<f64> {
        self.snapshot
            .as_deref()
            .map(|snapshot| f64::from_bits(snapshot.y_bits))
    }

    #[must_use]
    pub fn z(&self) -> Option<f64> {
        self.snapshot
            .as_deref()
            .map(|snapshot| f64::from_bits(snapshot.z_bits))
    }

    fn unavailable() -> Self {
        Self { snapshot: None }
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
    pub fn server_stopping(reason: impl Into<String>) -> Self {
        Self {
            target_plugin_id: None,
            kind: ScriptEventKind::ServerStopping {
                reason: reason.into(),
            },
        }
    }

    /// Build a player-joined event snapshot.
    pub fn player_joined(player_id: ScriptPlayerId, username: impl Into<String>) -> Self {
        Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerJoined {
                player_id,
                username: username.into(),
                context: ScriptPlayerContext::unavailable(),
            },
        }
    }

    /// Build a player-joined event snapshot with server-authoritative context.
    pub fn player_joined_with_context(
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
    ) -> Self {
        Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerJoined {
                player_id,
                username: context
                    .username()
                    .expect("player context must include a verified username")
                    .to_owned(),
                context,
            },
        }
    }

    /// Build a player-left event snapshot.
    pub fn player_left(player_id: ScriptPlayerId, reason: impl Into<String>) -> Self {
        Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerLeft {
                player_id,
                reason: reason.into(),
            },
        }
    }

    /// Build a player-chat event snapshot.
    pub fn player_chat(player_id: ScriptPlayerId, message: impl Into<String>) -> Self {
        Self::player_chat_with_context(player_id, message, ScriptPlayerContext::unavailable())
    }

    /// Build a player-chat event snapshot with server-authoritative context.
    pub fn player_chat_with_context(
        player_id: ScriptPlayerId,
        message: impl Into<String>,
        context: ScriptPlayerContext,
    ) -> Self {
        Self {
            target_plugin_id: None,
            kind: ScriptEventKind::PlayerChat {
                player_id,
                message: message.into(),
                context,
            },
        }
    }

    /// Build a player command event targeted to the plugin that owns its root.
    pub fn player_command(
        target_plugin_id: impl Into<String>,
        player_id: ScriptPlayerId,
        username: impl Into<String>,
        root: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            target_plugin_id: Some(target_plugin_id.into()),
            kind: ScriptEventKind::PlayerCommand {
                player_id,
                username: username.into(),
                root: root.into(),
                arguments: arguments.into(),
                context: ScriptPlayerContext::unavailable(),
            },
        }
    }

    /// Build a player command event with server-authoritative context.
    pub fn player_command_with_context(
        target_plugin_id: impl Into<String>,
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        root: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            target_plugin_id: Some(target_plugin_id.into()),
            kind: ScriptEventKind::PlayerCommand {
                player_id,
                username: context
                    .username()
                    .expect("player context must include a verified username")
                    .to_owned(),
                root: root.into(),
                arguments: arguments.into(),
                context,
            },
        }
    }

    /// Build a server-tick event snapshot.
    pub fn server_tick(tick: u64) -> Self {
        Self {
            target_plugin_id: None,
            kind: ScriptEventKind::ServerTick { tick },
        }
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
            ScriptEventKind::PlayerCommand { .. } => "player.command",
            ScriptEventKind::ServerTick { .. } => "server.tick",
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
}

/// Outbound command requests emitted by script code.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptCommand {
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
}

impl ScriptCommand {
    /// Return the host capability required before admitting this command.
    pub fn required_capability(&self) -> Option<ScriptCommandCapability> {
        match self {
            Self::SendChatMessage { .. }
            | Self::BroadcastChatMessage { .. }
            | Self::DisconnectPlayer { .. } => None,
            Self::RunConsoleCommand { command } => {
                Some(ScriptCommandCapability::RunConsoleCommandRoot {
                    root: console_command_root(command),
                })
            }
            Self::SpawnEntity { entity_type, .. } => {
                Some(ScriptCommandCapability::SpawnEntityType {
                    entity_type: entity_type.clone(),
                })
            }
        }
    }
}

/// Error returned when a bounded script queue cannot accept an item.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptQueueError<T> {
    Full(T),
    Closed(T),
}

impl<T> From<mpsc::error::TrySendError<T>> for ScriptQueueError<T> {
    fn from(error: mpsc::error::TrySendError<T>) -> Self {
        match error {
            mpsc::error::TrySendError::Full(item) => Self::Full(item),
            mpsc::error::TrySendError::Closed(item) => Self::Closed(item),
        }
    }
}

/// Server-owned side of the script boundary.
#[derive(Debug, Clone)]
pub struct ScriptBoundary {
    event_tx: mpsc::Sender<ScriptEvent>,
    command_rx: Arc<Mutex<mpsc::Receiver<ScriptCommand>>>,
    player_command_owners: PlayerCommandOwners,
}

impl ScriptBoundary {
    /// Enqueue an immutable event without blocking a server task.
    pub fn try_enqueue_event(
        &self,
        event: ScriptEvent,
    ) -> Result<(), ScriptQueueError<ScriptEvent>> {
        self.event_tx
            .try_send(event)
            .map_err(ScriptQueueError::from)
    }

    /// Return a sorted snapshot of currently active plugin command roots.
    pub fn player_command_roots(&self) -> Vec<String> {
        self.player_command_owners.roots(false)
    }

    /// Return a sorted snapshot of active operator-only plugin command roots.
    pub fn operator_command_roots(&self) -> Vec<String> {
        self.player_command_owners.roots(true)
    }

    /// Route a raw player command to its active plugin owner without blocking.
    ///
    /// `Ok(false)` means no plugin owns the root. Queue errors retain the event
    /// so the caller can apply the existing full/closed backpressure policy.
    pub fn try_enqueue_player_command(
        &self,
        player_id: ScriptPlayerId,
        username: impl Into<String>,
        raw: &str,
    ) -> Result<bool, ScriptQueueError<ScriptEvent>> {
        match self.try_enqueue_player_command_with_operator(player_id, username, raw, false)? {
            PlayerCommandAdmission::Enqueued => Ok(true),
            PlayerCommandAdmission::NotOwned
            | PlayerCommandAdmission::Dropped
            | PlayerCommandAdmission::PermissionDenied => Ok(false),
        }
    }

    /// Route a raw player command through the legacy compatibility API.
    ///
    /// The boolean argument is retained for source compatibility but is not
    /// authoritative. Operator-only roots always report denial before the bounded
    /// event queue; callers must use `try_enqueue_player_command_with_context`
    /// with a verified server context to authorize them.
    pub fn try_enqueue_player_command_with_operator(
        &self,
        player_id: ScriptPlayerId,
        username: impl Into<String>,
        raw: &str,
        _is_operator: bool,
    ) -> Result<PlayerCommandAdmission, ScriptQueueError<ScriptEvent>> {
        let Some((root, arguments)) = split_player_command(raw) else {
            return Ok(PlayerCommandAdmission::NotOwned);
        };
        let Some(owner) = self.player_command_owners.owner(root) else {
            return Ok(PlayerCommandAdmission::NotOwned);
        };
        if owner.operator_only {
            return Ok(PlayerCommandAdmission::PermissionDenied);
        }
        let event =
            ScriptEvent::player_command(owner.plugin_id, player_id, username, root, arguments);
        match self.try_enqueue_event(event) {
            Ok(()) => Ok(PlayerCommandAdmission::Enqueued),
            Err(error @ ScriptQueueError::Full(_)) => Err(error),
            Err(error @ ScriptQueueError::Closed(_)) => {
                self.player_command_owners.clear();
                Err(error)
            }
        }
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
    ) -> Result<PlayerCommandAdmission, ScriptQueueError<ScriptEvent>> {
        let Some((root, arguments)) = split_player_command(raw) else {
            return Ok(PlayerCommandAdmission::NotOwned);
        };
        let Some(owner) = self.player_command_owners.owner(root) else {
            return Ok(PlayerCommandAdmission::NotOwned);
        };
        if owner.operator_only && context.operator() != Some(true) {
            return Ok(PlayerCommandAdmission::PermissionDenied);
        };
        let event = ScriptEvent::player_command_with_context(
            owner.plugin_id,
            player_id,
            context,
            root,
            arguments,
        );
        match self.try_enqueue_event(event) {
            Ok(()) => Ok(PlayerCommandAdmission::Enqueued),
            Err(error @ ScriptQueueError::Full(_)) => Err(error),
            Err(error @ ScriptQueueError::Closed(_)) => {
                self.player_command_owners.clear();
                Err(error)
            }
        }
    }

    /// Wait for the next command emitted by the script host.
    pub async fn recv_command(&self) -> Option<ScriptCommand> {
        self.command_rx.lock().await.recv().await
    }
}

/// Script-host side of the bounded boundary.
#[derive(Debug)]
pub struct ScriptHostEndpoint {
    event_rx: mpsc::Receiver<ScriptEvent>,
    command_tx: mpsc::Sender<ScriptCommand>,
    player_command_owners: PlayerCommandOwners,
}

impl ScriptHostEndpoint {
    /// Wait asynchronously until an event arrives or the server side closes.
    pub async fn recv_event(&mut self) -> Option<ScriptEvent> {
        self.event_rx.recv().await
    }

    /// Block the dedicated host thread until an event arrives or the server side closes.
    pub fn recv_event_blocking(&mut self) -> Option<ScriptEvent> {
        self.event_rx.blocking_recv()
    }

    /// Submit a command without blocking the host thread.
    pub fn try_submit_command(
        &self,
        command: ScriptCommand,
    ) -> Result<(), ScriptQueueError<ScriptCommand>> {
        self.command_tx
            .try_send(command)
            .map_err(ScriptQueueError::from)
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
}

#[derive(Debug, Clone, Default)]
struct PlayerCommandOwners {
    owners: Arc<RwLock<BTreeMap<String, PlayerCommandOwner>>>,
}

#[derive(Debug, Clone)]
struct PlayerCommandOwner {
    plugin_id: String,
    operator_only: bool,
}

impl PlayerCommandOwners {
    fn roots(&self, operator_only: bool) -> Vec<String> {
        self.read()
            .iter()
            .filter(|(_, owner)| owner.operator_only == operator_only)
            .map(|(root, _)| root.clone())
            .collect()
    }

    fn owner(&self, root: &str) -> Option<PlayerCommandOwner> {
        self.read().get(root).cloned()
    }

    fn register(
        &self,
        plugin_id: &str,
        player_roots: &[String],
        operator_roots: &[String],
    ) -> Result<(), PlayerCommandRegistrationError> {
        let mut owners = self.write();
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
        self.write().retain(|_, owner| owner.plugin_id != plugin_id);
    }

    fn clear(&self) {
        self.write().clear();
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, BTreeMap<String, PlayerCommandOwner>> {
        self.owners
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, BTreeMap<String, PlayerCommandOwner>> {
        self.owners
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Construct the bounded server/host script boundary.
pub fn script_boundary_pair(
    event_capacity: NonZeroUsize,
    command_capacity: NonZeroUsize,
) -> (ScriptBoundary, ScriptHostEndpoint) {
    let (event_tx, event_rx) = mpsc::channel(event_capacity.get());
    let (command_tx, command_rx) = mpsc::channel(command_capacity.get());
    let player_command_owners = PlayerCommandOwners::default();
    (
        ScriptBoundary {
            event_tx,
            command_rx: Arc::new(Mutex::new(command_rx)),
            player_command_owners: player_command_owners.clone(),
        },
        ScriptHostEndpoint {
            event_rx,
            command_tx,
            player_command_owners,
        },
    )
}

/// Host capability required by privileged outbound script commands.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptCommandCapability {
    RunConsoleCommandRoot { root: String },
    SpawnEntityType { entity_type: String },
}

/// Declarative subscription to one Solaris script event name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ScriptEventSubscription {
    event_name: String,
}

impl ScriptEventSubscription {
    pub fn new(event_name: impl Into<String>) -> Self {
        Self {
            event_name: event_name.into(),
        }
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
    pub fn new(plugin_id: impl Into<String>, relation: ScriptPluginDependencyRelation) -> Self {
        Self {
            plugin_id: plugin_id.into(),
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
}

impl ScriptPluginManifest {
    /// Build a script plugin manifest DTO.
    pub fn new(
        plugin_id: impl Into<String>,
        display_name: impl Into<String>,
        version: impl Into<String>,
        requested_api_version: ScriptApiVersion,
    ) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            display_name: display_name.into(),
            version: version.into(),
            requested_api_version,
            load_phase: ScriptPluginLoadPhase::default(),
            event_subscriptions: Vec::new(),
            dependencies: Vec::new(),
            declared_command_capabilities: Vec::new(),
            player_command_roots: Vec::new(),
            operator_command_roots: Vec::new(),
            declared_permissions: Vec::new(),
        }
    }

    /// Declare the preferred load phase for a future loader.
    pub fn with_load_phase(mut self, load_phase: ScriptPluginLoadPhase) -> Self {
        self.load_phase = load_phase;
        self
    }

    /// Declare interest in one Solaris-native script event name.
    pub fn subscribe_event(mut self, event_name: impl Into<String>) -> Self {
        self.event_subscriptions
            .push(ScriptEventSubscription::new(event_name));
        self
    }

    /// Declare a plugin dependency or load-order edge.
    pub fn declare_dependency(
        mut self,
        plugin_id: impl Into<String>,
        relation: ScriptPluginDependencyRelation,
    ) -> Self {
        self.dependencies
            .push(ScriptPluginDependency::new(plugin_id, relation));
        self
    }

    /// Declare that this plugin requests access to a console command root.
    pub fn declare_console_command_root(mut self, root: impl Into<String>) -> Self {
        self.declared_command_capabilities
            .push(ScriptCommandCapability::RunConsoleCommandRoot { root: root.into() });
        self
    }

    /// Declare one exact entity type this plugin may spawn.
    pub fn declare_spawn_entity_type(mut self, entity_type: impl Into<String>) -> Self {
        self.declared_command_capabilities
            .push(ScriptCommandCapability::SpawnEntityType {
                entity_type: entity_type.into(),
            });
        self
    }

    /// Declare a literal command root that players may invoke for this plugin.
    pub fn declare_player_command_root(mut self, root: impl Into<String>) -> Self {
        self.player_command_roots.push(root.into());
        self
    }

    /// Declare a literal player command root that only operators may invoke.
    pub fn declare_operator_command_root(mut self, root: impl Into<String>) -> Self {
        self.operator_command_roots.push(root.into());
        self
    }

    /// Declare an opaque plugin permission string for a future loader.
    pub fn declare_permission(mut self, permission: impl Into<String>) -> Self {
        self.declared_permissions.push(permission.into());
        self
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
        if self.plugin_id.trim().is_empty() {
            return Err(ScriptPluginManifestError::BlankPluginId);
        }

        if !is_valid_plugin_id(&self.plugin_id) {
            return Err(ScriptPluginManifestError::InvalidPluginId {
                plugin_id: self.plugin_id.clone(),
            });
        }

        if !supports_script_api_version(self.requested_api_version) {
            return Err(ScriptPluginManifestError::UnsupportedScriptApiVersion {
                requested: self.requested_api_version,
                supported: SCRIPT_API_VERSION,
            });
        }

        if !self.player_command_roots.is_empty()
            && self.requested_api_version < PLAYER_COMMANDS_API_VERSION
        {
            return Err(
                ScriptPluginManifestError::PlayerCommandsRequireScriptApiVersion {
                    requested: self.requested_api_version,
                    minimum: PLAYER_COMMANDS_API_VERSION,
                },
            );
        }
        if !self.operator_command_roots.is_empty()
            && self.requested_api_version < OPERATOR_COMMANDS_API_VERSION
        {
            return Err(
                ScriptPluginManifestError::OperatorCommandsRequireScriptApiVersion {
                    requested: self.requested_api_version,
                    minimum: OPERATOR_COMMANDS_API_VERSION,
                },
            );
        }
        if self
            .declared_command_capabilities
            .iter()
            .any(|capability| matches!(capability, ScriptCommandCapability::SpawnEntityType { .. }))
            && self.requested_api_version < ENTITY_SPAWN_API_VERSION
        {
            return Err(
                ScriptPluginManifestError::SpawnEntitiesRequireScriptApiVersion {
                    requested: self.requested_api_version,
                    minimum: ENTITY_SPAWN_API_VERSION,
                },
            );
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
    #[cfg(feature = "host-api")]
    pub fn to_command_capabilities(&self) -> CommandCapabilities {
        let mut capabilities = CommandCapabilities::none();
        for capability in &self.declared_command_capabilities {
            match capability {
                ScriptCommandCapability::RunConsoleCommandRoot { root } => {
                    capabilities = capabilities.allow_console_command_root(root);
                }
                ScriptCommandCapability::SpawnEntityType { entity_type } => {
                    capabilities = capabilities.allow_spawn_entity_type(entity_type);
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
    BlankPluginId,
    InvalidPluginId {
        plugin_id: String,
    },
    UnsupportedScriptApiVersion {
        requested: ScriptApiVersion,
        supported: ScriptApiVersion,
    },
    PlayerCommandsRequireScriptApiVersion {
        requested: ScriptApiVersion,
        minimum: ScriptApiVersion,
    },
    OperatorCommandsRequireScriptApiVersion {
        requested: ScriptApiVersion,
        minimum: ScriptApiVersion,
    },
    SpawnEntitiesRequireScriptApiVersion {
        requested: ScriptApiVersion,
        minimum: ScriptApiVersion,
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

/// Allow-list of privileged outbound command capabilities granted by the host.
///
/// Default builds expose empty capabilities for script-facing callers. Trusted
/// host-side crates can enable the `host-api` Cargo feature to construct
/// non-empty allow-lists.
#[cfg_attr(
    not(feature = "host-api"),
    doc = r#"
The root-granting builder is absent from the default public API:

```compile_fail
use mc_script::CommandCapabilities;

let _forged = CommandCapabilities::none().allow_console_command_root("stop");
```
"#
)]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct CommandCapabilities {
    console_command_roots: Vec<String>,
    spawn_entity_types: Vec<String>,
}

impl CommandCapabilities {
    /// Return capabilities with no privileged console command roots allowed.
    pub fn none() -> Self {
        Self::default()
    }

    /// Trusted host-side builder for allowing console commands with a root token.
    ///
    /// Available to unit tests and to crates that opt into the `host-api`
    /// feature.
    ///
    /// ```
    /// use mc_script::{CommandCapabilities, ScriptCommandCapability};
    ///
    /// let capabilities = CommandCapabilities::none().allow_console_command_root("time");
    /// assert!(capabilities.allows(&ScriptCommandCapability::RunConsoleCommandRoot {
    ///     root: "time".to_owned(),
    /// }));
    /// ```
    #[cfg(any(test, feature = "host-api"))]
    pub fn allow_console_command_root(mut self, root: impl AsRef<str>) -> Self {
        let root = console_command_root(root.as_ref());
        if !self
            .console_command_roots
            .iter()
            .any(|allowed| allowed == &root)
        {
            self.console_command_roots.push(root);
        }
        self
    }

    /// Trusted host-side builder for allowing one exact entity type.
    #[cfg(any(test, feature = "host-api"))]
    pub fn allow_spawn_entity_type(mut self, entity_type: impl AsRef<str>) -> Self {
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

    /// Return whether this allow-list grants the requested command capability.
    pub fn allows(&self, capability: &ScriptCommandCapability) -> bool {
        match capability {
            ScriptCommandCapability::RunConsoleCommandRoot { root } => self
                .console_command_roots
                .iter()
                .any(|allowed| allowed == root),
            ScriptCommandCapability::SpawnEntityType { entity_type } => self
                .spawn_entity_types
                .iter()
                .any(|allowed| allowed == entity_type),
        }
    }
}

/// Error returned when a command batch cannot accept another command.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommandBatchError {
    Full { limit: NonZeroUsize },
    PermissionDenied { capability: ScriptCommandCapability },
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
        Self {
            limit,
            commands: Vec::with_capacity(limit.get()),
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
        if let Some(capability) = command.required_capability() {
            return Err(CommandBatchError::PermissionDenied { capability });
        }

        self.try_push_unchecked(command)
    }

    fn try_push_unchecked(&mut self, command: ScriptCommand) -> Result<(), CommandBatchError> {
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
        if let Some(capability) = command.required_capability()
            && !capabilities.allows(&capability)
        {
            return Err(CommandBatchError::PermissionDenied { capability });
        }

        self.try_push_unchecked(command)
    }
}

fn console_command_root(command: &str) -> String {
    command
        .trim()
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned()
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
            | "server.tick"
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
        return Err(ScriptPluginManifestError::UnboundedCommandRoot { root });
    }
    Ok(root)
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
            boundary.try_enqueue_event(second.clone()),
            Err(ScriptQueueError::Full(second))
        );
        assert_eq!(endpoint.recv_event_blocking(), Some(first));
    }

    #[test]
    fn script_event_queue_errors_stay_below_the_result_size_threshold() {
        assert!(
            std::mem::size_of::<ScriptQueueError<ScriptEvent>>() <= 128,
            "queue errors must remain small enough to return directly"
        );
    }

    #[test]
    fn event_dtos_are_stable_snapshots_without_host_handles() {
        let event = ScriptEvent::player_joined(ScriptPlayerId::new(42), "kaiser");

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

        let source = include_str!("lib.rs");
        let forbidden = [
            format!("{}{}", "World", "Handle"),
            format!("{}{}", "Session", "Registry"),
            format!("{}{}", "World", "Storage"),
        ];
        for name in forbidden {
            assert!(
                !source.contains(&name),
                "script contract must not expose {name}"
            );
        }
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
        assert!(context.is_verified());
        assert_eq!(context.uuid(), Some("123e4567-e89b-12d3-a456-426614174000"));
        assert_eq!(context.username(), Some("kaiser"));
        assert_eq!(context.operator(), Some(true));
        assert_eq!(
            (context.x(), context.y(), context.z()),
            (Some(12.25), Some(70.0), Some(-4.5))
        );
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
                capability: ScriptCommandCapability::RunConsoleCommandRoot {
                    root: "stop".to_owned(),
                },
            })
        );
        assert!(batch.commands().is_empty());

        let raw_denied = ScriptCommand::RunConsoleCommand {
            command: "/stop".to_owned(),
        };
        assert_eq!(
            batch.try_push(raw_denied),
            Err(CommandBatchError::PermissionDenied {
                capability: ScriptCommandCapability::RunConsoleCommandRoot {
                    root: "stop".to_owned(),
                },
            })
        );
        assert!(batch.commands().is_empty());

        let capabilities = CommandCapabilities::default().allow_console_command_root("time");
        let allowed = ScriptCommand::RunConsoleCommand {
            command: "/time set day".to_owned(),
        };

        assert_eq!(
            allowed.required_capability(),
            Some(ScriptCommandCapability::RunConsoleCommandRoot {
                root: "time".to_owned(),
            })
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
        let requested = ScriptApiVersion::new(0, 6, 0);
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
    fn entity_spawn_api_is_available_at_0_5_0() {
        assert_eq!(SCRIPT_API_VERSION, ScriptApiVersion::new(0, 5, 0));
    }

    #[test]
    fn spawn_entity_capability_is_exact_and_manifest_bounded() {
        let legacy = ScriptPluginManifest::new(
            "spawn-test",
            "Spawn Test",
            "0.1.0",
            ScriptApiVersion::new(0, 4, 0),
        )
        .declare_spawn_entity_type("minecraft:pig");
        assert!(matches!(
            legacy.validate(),
            Err(ScriptPluginManifestError::SpawnEntitiesRequireScriptApiVersion { .. })
        ));

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
            pig.required_capability(),
            Some(ScriptCommandCapability::SpawnEntityType {
                entity_type: "minecraft:pig".to_owned(),
            })
        );
        batch
            .try_push_authorized(pig.clone(), &capabilities)
            .unwrap();
        assert_eq!(
            batch.try_push_authorized(cow, &capabilities),
            Err(CommandBatchError::PermissionDenied {
                capability: ScriptCommandCapability::SpawnEntityType {
                    entity_type: "minecraft:cow".to_owned(),
                },
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
            Err(ScriptPluginManifestError::PlayerCommandRootTooLong {
                root: over_limit_root,
                max_bytes: MAX_PLAYER_COMMAND_ROOT_BYTES,
            })
        );
    }

    #[test]
    fn manifest_validation_rejects_unsafe_and_reserved_player_command_roots() {
        for root in ["Hello", "/hello", "hello there", "hello.world", ""] {
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
        let event = ScriptEvent::player_command(
            "greetings",
            ScriptPlayerId::new(42),
            "Alex",
            "hello",
            "one  two ",
        );

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
    fn player_command_boundary_routes_one_owner_and_retains_queue_full_policy() {
        let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
        let manifest =
            ScriptPluginManifest::new("greetings", "Greetings", "0.1.0", SCRIPT_API_VERSION)
                .declare_player_command_root("hello")
                .validate()
                .unwrap();
        endpoint.register_player_commands(&manifest).unwrap();

        assert_eq!(boundary.player_command_roots(), vec!["hello".to_owned()]);
        assert_eq!(
            boundary.try_enqueue_player_command(ScriptPlayerId::new(7), "Alex", "missing arg"),
            Ok(false)
        );
        assert_eq!(
            boundary.try_enqueue_player_command(ScriptPlayerId::new(7), "Alex", "/hello one  two "),
            Ok(true)
        );
        let full = boundary
            .try_enqueue_player_command(ScriptPlayerId::new(7), "Alex", "hello later")
            .unwrap_err();
        assert!(matches!(
            full,
            ScriptQueueError::Full(event)
                if event.target_plugin_id() == Some("greetings")
                    && matches!(
                        event.kind(),
                        ScriptEventKind::PlayerCommand { root, arguments, .. }
                            if root == "hello" && arguments == "later"
                    )
        ));
        assert_eq!(
            endpoint.recv_event_blocking(),
            Some(ScriptEvent::player_command(
                "greetings",
                ScriptPlayerId::new(7),
                "Alex",
                "hello",
                "one  two ",
            ))
        );
    }

    #[test]
    fn operator_player_commands_require_verified_context_and_are_denied_before_a_full_queue() {
        let api_0_3 = ScriptPluginManifest::new(
            "admin-day",
            "Admin Day",
            "0.1.0",
            ScriptApiVersion::new(0, 3, 0),
        )
        .declare_operator_command_root("adminday");
        assert_eq!(
            api_0_3.validate(),
            Err(
                ScriptPluginManifestError::OperatorCommandsRequireScriptApiVersion {
                    requested: ScriptApiVersion::new(0, 3, 0),
                    minimum: OPERATOR_COMMANDS_API_VERSION,
                }
            )
        );

        let manifest = ScriptPluginManifest::new(
            "admin-day",
            "Admin Day",
            "0.1.0",
            ScriptApiVersion::new(0, 4, 0),
        )
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
            boundary.try_enqueue_player_command_with_operator(
                ScriptPlayerId::new(7),
                "Alex",
                "adminday",
                false,
            ),
            Ok(PlayerCommandAdmission::PermissionDenied)
        );
        assert_eq!(
            endpoint.recv_event_blocking(),
            Some(ScriptEvent::server_started()),
            "denied command must not be enqueued behind a full queue"
        );
        assert_eq!(
            boundary.try_enqueue_player_command_with_operator(
                ScriptPlayerId::new(7),
                "Alex",
                "adminday",
                true,
            ),
            Ok(PlayerCommandAdmission::PermissionDenied)
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
                ScriptEventSubscription::new("player.chat"),
                ScriptEventSubscription::new("server.tick"),
            ]
        );
        assert_eq!(
            validated.dependencies(),
            &[
                ScriptPluginDependency::new("economy", ScriptPluginDependencyRelation::Required),
                ScriptPluginDependency::new("chat-tools", ScriptPluginDependencyRelation::Optional),
                ScriptPluginDependency::new(
                    "spawn-protect",
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

    #[cfg(feature = "host-api")]
    #[test]
    fn trusted_host_derives_command_capabilities_from_validated_manifest() {
        let manifest = ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", SCRIPT_API_VERSION)
            .declare_console_command_root("time");
        let validated = manifest.validate().unwrap();
        let capabilities = validated.to_command_capabilities();

        let time = ScriptCommand::RunConsoleCommand {
            command: "/time set day".to_owned(),
        }
        .required_capability()
        .unwrap();
        assert!(capabilities.allows(&time));
        assert!(
            !capabilities.allows(
                &ScriptCommand::RunConsoleCommand {
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
        let event = ScriptEvent::player_chat(ScriptPlayerId::new(7), "hello");

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
    fn script_api_version_accepts_current_and_older_minor_only() {
        assert_eq!(SCRIPT_API_VERSION, ScriptApiVersion::new(0, 5, 0));
        assert!(supports_script_api_version(SCRIPT_API_VERSION));
        assert!(supports_script_api_version(ScriptApiVersion::new(0, 2, 0)));
        assert!(supports_script_api_version(ScriptApiVersion::new(0, 2, 99)));
        assert!(supports_script_api_version(ScriptApiVersion::new(0, 1, 0)));
        assert!(supports_script_api_version(ScriptApiVersion::new(0, 0, 0)));
        assert!(supports_script_api_version(ScriptApiVersion::new(0, 3, 1)));
        assert!(!supports_script_api_version(ScriptApiVersion::new(0, 5, 1)));
        assert!(!supports_script_api_version(ScriptApiVersion::new(0, 6, 0)));
        assert!(!supports_script_api_version(ScriptApiVersion::new(1, 0, 0)));
    }

    #[test]
    fn manifest_without_player_commands_remains_compatible_with_api_0_2() {
        let manifest =
            ScriptPluginManifest::new("legacy", "Legacy", "0.1.0", ScriptApiVersion::new(0, 2, 0));

        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn manifest_player_commands_require_api_0_3() {
        let manifest = ScriptPluginManifest::new(
            "legacy-commands",
            "Legacy Commands",
            "0.1.0",
            ScriptApiVersion::new(0, 2, 0),
        )
        .declare_player_command_root("hello");

        assert_eq!(
            manifest.validate(),
            Err(
                ScriptPluginManifestError::PlayerCommandsRequireScriptApiVersion {
                    requested: ScriptApiVersion::new(0, 2, 0),
                    minimum: PLAYER_COMMANDS_API_VERSION,
                }
            )
        );
    }
}
