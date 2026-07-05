//! # mc-script
//!
//! Safe script runtime contract primitives.
//!
//! This crate intentionally does not embed Lua, WASM, or any other VM. It only
//! defines the lock-free API shape a future script host can use: immutable event
//! snapshots enter the runtime, and bounded command batches leave it. Runtime
//! controls for fuel, memory, deadlines, and cooperative shutdown are modeled now
//! so the eventual VM cannot grow an unbounded host-facing surface by default.

use std::num::{NonZeroU64, NonZeroUsize};
use std::time::Duration;

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Semantic version of the stable script API contract.
pub const SCRIPT_API_VERSION: ScriptApiVersion = ScriptApiVersion::new(0, 2, 0);

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
    kind: ScriptEventKind,
}

impl ScriptEvent {
    /// Build a server-started event snapshot.
    pub fn server_started() -> Self {
        Self {
            kind: ScriptEventKind::ServerStarted,
        }
    }

    /// Build a server-stopping event snapshot.
    pub fn server_stopping(reason: impl Into<String>) -> Self {
        Self {
            kind: ScriptEventKind::ServerStopping {
                reason: reason.into(),
            },
        }
    }

    /// Build a player-joined event snapshot.
    pub fn player_joined(player_id: ScriptPlayerId, username: impl Into<String>) -> Self {
        Self {
            kind: ScriptEventKind::PlayerJoined {
                player_id,
                username: username.into(),
            },
        }
    }

    /// Build a player-left event snapshot.
    pub fn player_left(player_id: ScriptPlayerId, reason: impl Into<String>) -> Self {
        Self {
            kind: ScriptEventKind::PlayerLeft {
                player_id,
                reason: reason.into(),
            },
        }
    }

    /// Build a player-chat event snapshot.
    pub fn player_chat(player_id: ScriptPlayerId, message: impl Into<String>) -> Self {
        Self {
            kind: ScriptEventKind::PlayerChat {
                player_id,
                message: message.into(),
            },
        }
    }

    /// Build a server-tick event snapshot.
    pub fn server_tick(tick: u64) -> Self {
        Self {
            kind: ScriptEventKind::ServerTick { tick },
        }
    }

    /// Return the immutable event kind.
    pub fn kind(&self) -> &ScriptEventKind {
        &self.kind
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
    },
    PlayerLeft {
        player_id: ScriptPlayerId,
        reason: String,
    },
    PlayerChat {
        player_id: ScriptPlayerId,
        message: String,
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
        }
    }
}

/// Host capability required by privileged outbound script commands.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptCommandCapability {
    RunConsoleCommandRoot { root: String },
}

/// Plugin manifest contract consumed by a future server-side script loader.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ScriptPluginManifest {
    plugin_id: String,
    display_name: String,
    version: String,
    requested_api_version: ScriptApiVersion,
    declared_command_capabilities: Vec<ScriptCommandCapability>,
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
            declared_command_capabilities: Vec::new(),
            declared_permissions: Vec::new(),
        }
    }

    /// Declare that this plugin requests access to a console command root.
    pub fn declare_console_command_root(mut self, root: impl Into<String>) -> Self {
        self.declared_command_capabilities
            .push(ScriptCommandCapability::RunConsoleCommandRoot { root: root.into() });
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

    pub fn declared_command_capabilities(&self) -> &[ScriptCommandCapability] {
        &self.declared_command_capabilities
    }

    pub fn declared_permissions(&self) -> &[String] {
        &self.declared_permissions
    }

    /// Validate and normalize this manifest for trusted host-side use.
    pub fn validate(&self) -> Result<ValidatedScriptPluginManifest, ScriptPluginManifestError> {
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
            }
        }

        Ok(ValidatedScriptPluginManifest {
            plugin_id: self.plugin_id.clone(),
            display_name: self.display_name.clone(),
            version: self.version.clone(),
            requested_api_version: self.requested_api_version,
            declared_command_capabilities: normalized_capabilities,
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
    declared_command_capabilities: Vec<ScriptCommandCapability>,
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

    pub fn declared_command_capabilities(&self) -> &[ScriptCommandCapability] {
        &self.declared_command_capabilities
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
            }
        }
        capabilities
    }
}

/// Error returned when validating a script plugin manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptPluginManifestError {
    InvalidPluginId {
        plugin_id: String,
    },
    UnsupportedScriptApiVersion {
        requested: ScriptApiVersion,
        supported: ScriptApiVersion,
    },
    BlankCommandRoot,
    UnboundedCommandRoot {
        root: String,
    },
    DuplicateCommandRoot {
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

    /// Return whether this allow-list grants the requested command capability.
    pub fn allows(&self, capability: &ScriptCommandCapability) -> bool {
        match capability {
            ScriptCommandCapability::RunConsoleCommandRoot { root } => self
                .console_command_roots
                .iter()
                .any(|allowed| allowed == root),
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
    fn event_dtos_are_stable_snapshots_without_host_handles() {
        let event = ScriptEvent::player_joined(ScriptPlayerId::new(42), "kaiser");

        assert_eq!(
            event.kind(),
            &ScriptEventKind::PlayerJoined {
                player_id: ScriptPlayerId::new(42),
                username: "kaiser".to_owned(),
            }
        );
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
        let requested = ScriptApiVersion::new(0, 3, 0);
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
                if let ScriptEventKind::PlayerChat { player_id, message } = event.kind() {
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
        assert_eq!(
            event.kind(),
            &ScriptEventKind::PlayerChat {
                player_id: ScriptPlayerId::new(7),
                message: "hello".to_owned(),
            }
        );
    }

    #[test]
    fn script_api_version_accepts_current_and_older_minor_only() {
        assert_eq!(SCRIPT_API_VERSION, ScriptApiVersion::new(0, 2, 0));
        assert!(supports_script_api_version(SCRIPT_API_VERSION));
        assert!(supports_script_api_version(ScriptApiVersion::new(0, 1, 0)));
        assert!(supports_script_api_version(ScriptApiVersion::new(0, 0, 0)));
        assert!(!supports_script_api_version(ScriptApiVersion::new(0, 2, 1)));
        assert!(!supports_script_api_version(ScriptApiVersion::new(0, 3, 0)));
        assert!(!supports_script_api_version(ScriptApiVersion::new(1, 0, 0)));
    }
}
