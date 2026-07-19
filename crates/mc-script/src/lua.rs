use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use mlua::{Function, HookTriggers, Lua, LuaOptions, StdLib, Table, VmState};
use serde::Deserialize;
use tracing::{info, warn};

use crate::{
    CommandBatch, CommandBatchError, CommandCapabilities, PlayerCommandRegistrationError,
    RuntimeContext, RuntimeError, RuntimeResult, ScriptApiVersion, ScriptBoundary, ScriptCommand,
    ScriptEvent, ScriptEventKind, ScriptHostEndpoint, ScriptPlayerId, ScriptPluginManifest,
    ScriptPosition, ScriptQueueError, ScriptRuntime, ValidatedScriptPluginManifest,
    script_boundary_pair,
};

const EVENT_QUEUE_CAPACITY: usize = 1_024;
const COMMAND_QUEUE_CAPACITY: usize = 256;
const COMMANDS_PER_EVENT: usize = 32;
const MEMORY_BYTES_PER_PLUGIN: usize = 16 * 1024 * 1024;
const INSTRUCTIONS_PER_EVENT: u64 = 100_000;
const HOOK_INSTRUCTION_STEP: u32 = 1_000;
const MAX_CHAT_MESSAGE_BYTES: usize = 4_096;
const MAX_DISCONNECT_REASON_BYTES: usize = 1_024;
const MAX_CONSOLE_COMMAND_BYTES: usize = 256;

/// Filesystem configuration for the built-in Lua plugin host.
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

/// Startup error for the Lua host itself. Individual broken plugins are skipped.
#[derive(Debug)]
#[non_exhaustive]
pub enum LuaHostError {
    Io { path: PathBuf, message: String },
    ThreadSpawn { message: String },
    StartupChannelClosed,
}

impl fmt::Display for LuaHostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(formatter, "{}: {message}", path.display()),
            Self::ThreadSpawn { message } => {
                write!(formatter, "starting Lua host thread: {message}")
            }
            Self::StartupChannelClosed => formatter.write_str("Lua host startup channel closed"),
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

impl LuaHost {
    pub fn loaded_plugins(&self) -> usize {
        self.loaded_plugins
    }

    pub fn join(self) -> thread::Result<()> {
        self.thread.join()
    }
}

/// Start one dedicated host thread for all Lua plugins in the configured directory.
pub fn start_lua_host(config: LuaHostConfig) -> Result<(ScriptBoundary, LuaHost), LuaHostError> {
    fs::create_dir_all(config.plugins_dir()).map_err(|error| LuaHostError::Io {
        path: config.plugins_dir().to_path_buf(),
        message: error.to_string(),
    })?;
    let sources = discover_plugins(config.plugins_dir())?;
    let (boundary, endpoint) = script_boundary_pair(
        NonZeroUsize::new(EVENT_QUEUE_CAPACITY).expect("event queue capacity is non-zero"),
        NonZeroUsize::new(COMMAND_QUEUE_CAPACITY).expect("command queue capacity is non-zero"),
    );
    let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);
    let thread = thread::Builder::new()
        .name("solaris-lua-host".to_owned())
        .spawn(move || run_lua_host(endpoint, sources, startup_tx))
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
    source: String,
    source_path: PathBuf,
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
}

fn discover_plugins(plugins_dir: &Path) -> Result<Vec<PluginSource>, LuaHostError> {
    let entries = fs::read_dir(plugins_dir).map_err(|error| LuaHostError::Io {
        path: plugins_dir.to_path_buf(),
        message: error.to_string(),
    })?;
    let mut directories = entries
        .filter_map(|entry| match entry {
            Ok(entry) if entry.file_type().is_ok_and(|kind| kind.is_dir()) => Some(entry.path()),
            Ok(_) => None,
            Err(error) => {
                warn!(%error, directory = %plugins_dir.display(), "plugin directory entry ignored");
                None
            }
        })
        .collect::<Vec<_>>();
    directories.sort();

    let mut sources = Vec::new();
    for directory in directories {
        match read_plugin_source(&directory) {
            Ok(source) => sources.push(source),
            Err(error) => warn!(
                directory = %directory.display(),
                %error,
                "Lua plugin skipped during discovery"
            ),
        }
    }
    Ok(sources)
}

fn read_plugin_source(directory: &Path) -> Result<PluginSource, String> {
    let manifest_path = directory.join("plugin.toml");
    let raw_manifest = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("reading {}: {error}", manifest_path.display()))?;
    let disk: DiskManifest =
        toml::from_str(&raw_manifest).map_err(|error| format!("parsing manifest: {error}"))?;
    let requested_api_version = parse_api_version(&disk.api)?;
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
    let manifest = manifest
        .validate()
        .map_err(|error| format!("invalid manifest: {error:?}"))?;
    let source_path = directory.join("main.lua");
    let source = fs::read_to_string(&source_path)
        .map_err(|error| format!("reading {}: {error}", source_path.display()))?;
    Ok(PluginSource {
        manifest,
        source,
        source_path,
    })
}

fn parse_api_version(value: &str) -> Result<ScriptApiVersion, String> {
    let parts = value.split('.').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(format!(
            "api version must be MAJOR.MINOR.PATCH, got {value:?}"
        ));
    }
    let parse = |part: &str| {
        part.parse::<u16>()
            .map_err(|_| format!("invalid api version {value:?}"))
    };
    Ok(ScriptApiVersion::new(
        parse(parts[0])?,
        parse(parts[1])?,
        parse(parts[2])?,
    ))
}

fn run_lua_host(
    mut endpoint: ScriptHostEndpoint,
    sources: Vec<PluginSource>,
    startup: std::sync::mpsc::SyncSender<usize>,
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
            let commands = match plugin.handle_event(&event) {
                Ok(commands) => commands,
                Err(error) => {
                    warn!(plugin = %plugin.id, ?error, "Lua plugin disabled after handler failure");
                    endpoint.unregister_player_commands(&plugin.id);
                    plugin.disabled = true;
                    continue;
                }
            };
            for command in commands {
                match endpoint.try_submit_command(command) {
                    Ok(()) => {}
                    Err(ScriptQueueError::Full(_)) => {
                        warn!(plugin = %plugin.id, "Lua command queue full; command dropped");
                    }
                    Err(ScriptQueueError::Closed(_)) => return,
                }
            }
        }
    }
}

struct LuaPlugin {
    id: String,
    subscriptions: HashSet<String>,
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
        let runtime = LuaScriptRuntime::from_source(
            source.manifest,
            &source.source,
            LuaRuntimeLimits::default(),
        )
        .map_err(|error| format!("{}: {error}", source.source_path.display()))?;
        Ok(Self {
            id,
            subscriptions,
            runtime,
            disabled: false,
        })
    }

    fn handle_event(&mut self, event: &ScriptEvent) -> RuntimeResult<Vec<ScriptCommand>> {
        if self.disabled {
            return Ok(Vec::new());
        }
        if let Some(target_plugin_id) = event.target_plugin_id() {
            if target_plugin_id != self.id {
                return Ok(Vec::new());
            }
        } else if !self.subscriptions.contains(event.event_name()) {
            return Ok(Vec::new());
        }
        let controls = crate::RuntimeControls::unrestricted();
        self.runtime
            .handle_event(
                event,
                RuntimeContext::new(
                    &controls,
                    NonZeroUsize::new(COMMANDS_PER_EVENT).expect("commands per event is non-zero"),
                ),
            )
            .map(CommandBatch::into_commands)
    }
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
    capabilities: CommandCapabilities,
}

struct LuaScriptRuntime {
    lua: Lua,
    manifest: ValidatedScriptPluginManifest,
    invocation: Arc<Mutex<Option<InvocationState>>>,
    limits: LuaRuntimeLimits,
}

impl LuaScriptRuntime {
    fn from_source(
        manifest: ValidatedScriptPluginManifest,
        source: &str,
        limits: LuaRuntimeLimits,
    ) -> Result<Self, String> {
        let libraries = StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8;
        let lua = Lua::new_with(libraries, LuaOptions::default()).map_err(lua_error)?;
        lua.set_memory_limit(limits.memory_bytes.get())
            .map_err(lua_error)?;
        let invocation = Arc::new(Mutex::new(None));
        install_solaris_api(&lua, Arc::clone(&invocation)).map_err(lua_error)?;
        run_with_instruction_budget(&lua, limits.instructions_per_event, || {
            lua.load(source).set_name(manifest.plugin_id()).exec()
        })
        .map_err(lua_error)?;
        Ok(Self {
            lua,
            manifest,
            invocation,
            limits,
        })
    }

    fn capabilities(&self) -> CommandCapabilities {
        let mut capabilities = CommandCapabilities::none();
        for capability in self.manifest.declared_command_capabilities() {
            match capability {
                crate::ScriptCommandCapability::RunConsoleCommandRoot { root } => {
                    capabilities = capabilities.allow_console_command_root(root);
                }
                crate::ScriptCommandCapability::SpawnEntityType { entity_type } => {
                    capabilities = capabilities.allow_spawn_entity_type(entity_type);
                }
            }
        }
        capabilities
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
        let handler_name = handler_name(event);
        let handler = self
            .lua
            .globals()
            .get::<Option<Function>>(handler_name)
            .map_err(runtime_error)?;
        let Some(handler) = handler else {
            return Ok(context.command_batch());
        };
        let event_table = event_table(&self.lua, event).map_err(runtime_error)?;
        let capabilities = self.capabilities();
        *lock_invocation(&self.invocation) = Some(InvocationState {
            batch: context.command_batch(),
            capabilities,
        });
        let configured_budget = self.limits.instructions_per_event;
        let budget = context.controls().fuel().map_or(configured_budget, |fuel| {
            NonZeroU64::new(fuel.get().min(configured_budget.get()))
                .expect("minimum of non-zero budgets is non-zero")
        });
        let result =
            run_with_instruction_budget(&self.lua, budget, || handler.call::<()>(event_table));
        let invocation = lock_invocation(&self.invocation).take();
        match result {
            Ok(()) => Ok(invocation
                .expect("Lua invocation state exists while a handler runs")
                .batch),
            Err(error) => Err(runtime_error(error)),
        }
    }
}

fn install_solaris_api(
    lua: &Lua,
    invocation: Arc<Mutex<Option<InvocationState>>>,
) -> mlua::Result<()> {
    let api = lua.create_table()?;
    let send_invocation = Arc::clone(&invocation);
    api.set(
        "send_message",
        lua.create_function(move |_, (player_id, message): (u64, String)| {
            ensure_string_limit("chat message", &message, MAX_CHAT_MESSAGE_BYTES)?;
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
        lua.create_function(move |_, message: String| {
            ensure_string_limit("chat message", &message, MAX_CHAT_MESSAGE_BYTES)?;
            push_command(
                &broadcast_invocation,
                ScriptCommand::BroadcastChatMessage { message },
            )
        })?,
    )?;
    let disconnect_invocation = Arc::clone(&invocation);
    api.set(
        "disconnect",
        lua.create_function(move |_, (player_id, reason): (u64, String)| {
            ensure_string_limit("disconnect reason", &reason, MAX_DISCONNECT_REASON_BYTES)?;
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
        lua.create_function(move |_, command: String| {
            ensure_string_limit("console command", &command, MAX_CONSOLE_COMMAND_BYTES)?;
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
            move |_, (actor, entity_type, x, y, z): (u64, String, f64, f64, f64)| {
                crate::validate_script_resource_id(&entity_type)
                    .map_err(|_| mlua::Error::runtime("invalid entity type"))?;
                let position = ScriptPosition::try_new(x, y, z)
                    .ok_or_else(|| mlua::Error::runtime("invalid entity spawn position"))?;
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
    lua.globals().set("solaris", api)
}

fn ensure_string_limit(label: &str, value: &str, max: usize) -> mlua::Result<()> {
    if value.len() > max {
        return Err(mlua::Error::runtime(format!("{label} exceeds {max} bytes")));
    }
    Ok(())
}

fn push_command(
    invocation: &Arc<Mutex<Option<InvocationState>>>,
    command: ScriptCommand,
) -> mlua::Result<()> {
    let mut invocation = lock_invocation(invocation);
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
            mlua::Error::runtime(format!("command capability denied: {capability:?}"))
        }
    }
}

fn lock_invocation(
    invocation: &Arc<Mutex<Option<InvocationState>>>,
) -> std::sync::MutexGuard<'_, Option<InvocationState>> {
    invocation
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn run_with_instruction_budget<T>(
    lua: &Lua,
    budget: NonZeroU64,
    run: impl FnOnce() -> mlua::Result<T>,
) -> mlua::Result<T> {
    let consumed = Arc::new(AtomicU64::new(0));
    let hook_consumed = Arc::clone(&consumed);
    let step =
        u64::from(HOOK_INSTRUCTION_STEP.min(u32::try_from(budget.get()).unwrap_or(u32::MAX)));
    lua.set_hook(
        HookTriggers::new()
            .every_nth_instruction(u32::try_from(step).expect("instruction hook step fits u32")),
        move |_, _| {
            let total = hook_consumed.fetch_add(step, Ordering::Relaxed) + step;
            if total >= budget.get() {
                return Err(mlua::Error::runtime("instruction budget exceeded"));
            }
            Ok(VmState::Continue)
        },
    )?;
    let result = run();
    lua.remove_hook();
    result
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
    }
    Ok(table)
}

fn set_player_context(table: &Table, context: &crate::ScriptPlayerContext) -> mlua::Result<()> {
    table.set("context_verified", context.is_verified())?;
    if let Some(uuid) = context.uuid() {
        table.set("uuid", uuid)?;
    }
    if let Some(username) = context.username() {
        table.set("username", username)?;
    }
    if let Some(operator) = context.operator() {
        table.set("operator", operator)?;
    }
    if let Some(x) = context.x() {
        table.set("x", x)?;
    }
    if let Some(y) = context.y() {
        table.set("y", y)?;
    }
    if let Some(z) = context.z() {
        table.set("z", z)?;
    }
    Ok(())
}

fn handler_name(event: &ScriptEvent) -> &'static str {
    match event.kind() {
        ScriptEventKind::ServerStarted => "on_server_started",
        ScriptEventKind::ServerStopping { .. } => "on_server_stopping",
        ScriptEventKind::PlayerJoined { .. } => "on_player_joined",
        ScriptEventKind::PlayerLeft { .. } => "on_player_left",
        ScriptEventKind::PlayerChat { .. } => "on_player_chat",
        ScriptEventKind::PlayerCommand { .. } => "on_player_command",
        ScriptEventKind::ServerTick { .. } => "on_server_tick",
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
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{
        MAX_SCRIPT_RESOURCE_ID_BYTES, RuntimeControls, SCRIPT_API_VERSION, ScriptCommand,
        ScriptEvent, ScriptPlayerId, ScriptPluginManifest,
    };

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
                &ScriptEvent::player_joined(ScriptPlayerId::new(7), "Alex"),
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
                &ScriptEvent::player_command(
                    "test-plugin",
                    ScriptPlayerId::new(7),
                    "Alex",
                    "hello",
                    "one two",
                ),
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
        let event =
            ScriptEvent::player_command("spawn-test", ScriptPlayerId::new(7), "Alex", "pet", "");
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
        let expected_context = "true:123e4567-e89b-12d3-a456-426614174000:Alex:true:1.5:64.0:-2.25";

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
                ScriptEvent::player_command_with_context(
                    "test-plugin",
                    ScriptPlayerId::new(7),
                    context,
                    "hello",
                    "one two",
                ),
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
    fn lua_legacy_player_events_mark_context_unavailable_and_omit_authority_fields() {
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
                        tostring(event.uuid) .. ":" .. tostring(event.username) .. ":" ..
                        tostring(event.operator) .. ":" .. tostring(event.x) .. ":" ..
                        tostring(event.y) .. ":" .. tostring(event.z)
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

        for (event, expected) in [
            (
                ScriptEvent::player_joined(ScriptPlayerId::new(7), "Alex"),
                "joined:false:nil:Alex:nil:nil:nil:nil",
            ),
            (
                ScriptEvent::player_chat(ScriptPlayerId::new(7), "hello"),
                "chat:false:nil:nil:nil:nil:nil:nil",
            ),
            (
                ScriptEvent::player_command(
                    "test-plugin",
                    ScriptPlayerId::new(7),
                    "Alex",
                    "hello",
                    "one two",
                ),
                "command:false:nil:Alex:nil:nil:nil:nil",
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
                    message: expected.to_owned(),
                }]
            );
        }
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

        assert!(
            matches!(error, crate::RuntimeError::Trap { message } if message.contains("chat message exceeds 4096 bytes"))
        );
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
            source: r#"
                function on_server_tick(_event)
                    error("broken plugin")
                end
            "#
            .to_owned(),
            source_path: PathBuf::from("bad/main.lua"),
        };
        let good_manifest =
            ScriptPluginManifest::new("good-plugin", "Good Plugin", "0.1.0", SCRIPT_API_VERSION)
                .subscribe_event("server.tick")
                .validate()
                .unwrap();
        let good = PluginSource {
            manifest: good_manifest,
            source: r#"
                function on_server_tick(event)
                    solaris.broadcast("tick " .. event.tick)
                end
            "#
            .to_owned(),
            source_path: PathBuf::from("good/main.lua"),
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
        assert_eq!(
            first,
            ScriptCommand::BroadcastChatMessage {
                message: "tick 1".to_owned(),
            }
        );

        boundary
            .try_enqueue_event(ScriptEvent::server_tick(2))
            .unwrap();
        let second = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .expect("good plugin did not answer second tick")
            .expect("script command queue closed");
        assert_eq!(
            second,
            ScriptCommand::BroadcastChatMessage {
                message: "tick 2".to_owned(),
            }
        );

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
            source: "function on_server_tick(_event) end".to_owned(),
            source_path: PathBuf::from(path),
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
                api = "0.3.0"
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
            source: "function on_player_command(_event) end".to_owned(),
            source_path: PathBuf::from(path),
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
            source: format!(
                r#"
                    function on_player_command(_event)
                        solaris.broadcast("{id}")
                    end
                "#
            ),
            source_path: PathBuf::from(format!("{id}/main.lua")),
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
            boundary.try_enqueue_player_command(ScriptPlayerId::new(7), "Alex", "hello one two",),
            Ok(true)
        );
        let command = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .expect("owning plugin did not handle player command")
            .expect("script command queue closed");
        assert_eq!(
            command,
            ScriptCommand::BroadcastChatMessage {
                message: "greetings".to_owned(),
            }
        );

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
            source: r#"
                function on_player_command(_event)
                    error("broken plugin")
                end
            "#
            .to_owned(),
            source_path: PathBuf::from("bad/main.lua"),
        };
        let good = PluginSource {
            manifest: ScriptPluginManifest::new("good", "good", "0.1.0", SCRIPT_API_VERSION)
                .subscribe_event("server.tick")
                .validate()
                .unwrap(),
            source: r#"
                function on_server_tick(_event)
                    solaris.broadcast("progressed")
                end
            "#
            .to_owned(),
            source_path: PathBuf::from("good/main.lua"),
        };
        let (boundary, endpoint) =
            script_boundary_pair(NonZeroUsize::new(4).unwrap(), NonZeroUsize::new(4).unwrap());
        let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);
        let host = thread::spawn(move || run_lua_host(endpoint, vec![bad, good], startup_tx));
        assert_eq!(startup_rx.recv().unwrap(), 2);

        assert_eq!(
            boundary.try_enqueue_player_command(ScriptPlayerId::new(7), "Alex", "hello"),
            Ok(true)
        );
        boundary
            .try_enqueue_event(ScriptEvent::server_tick(1))
            .unwrap();
        let progress = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .expect("host did not process the event after the failed command")
            .expect("script command queue closed");
        assert_eq!(
            progress,
            ScriptCommand::BroadcastChatMessage {
                message: "progressed".to_owned(),
            }
        );
        assert!(boundary.player_command_roots().is_empty());
        assert_eq!(
            boundary.try_enqueue_player_command(ScriptPlayerId::new(7), "Alex", "hello"),
            Ok(false)
        );

        drop(boundary);
        tokio::task::spawn_blocking(move || host.join())
            .await
            .unwrap()
            .unwrap();
    }
}
