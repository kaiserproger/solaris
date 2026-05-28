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

/// Stable player identifier snapshot for script-visible DTOs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScriptPlayerId(pub u64);

/// Stable entity identifier snapshot for script-visible DTOs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScriptEntityId(pub u64);

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

/// Error returned when a command batch cannot accept another command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandBatchError {
    Full { limit: NonZeroUsize },
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
        if self.commands.len() >= self.limit.get() {
            return Err(CommandBatchError::Full { limit: self.limit });
        }

        self.commands.push(command);
        Ok(())
    }
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
        let event = ScriptEvent::player_joined(ScriptPlayerId(42), "kaiser");

        assert_eq!(
            event.kind(),
            &ScriptEventKind::PlayerJoined {
                player_id: ScriptPlayerId(42),
                username: "kaiser".to_owned(),
            }
        );

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
        let event = ScriptEvent::player_chat(ScriptPlayerId(7), "hello");

        let commands = EchoRuntime
            .handle_event(&event, context)
            .unwrap()
            .into_commands();

        assert_eq!(
            commands,
            vec![ScriptCommand::SendChatMessage {
                player_id: ScriptPlayerId(7),
                message: "echo: hello".to_owned(),
            }]
        );
        assert_eq!(
            event.kind(),
            &ScriptEventKind::PlayerChat {
                player_id: ScriptPlayerId(7),
                message: "hello".to_owned(),
            }
        );
    }
}
