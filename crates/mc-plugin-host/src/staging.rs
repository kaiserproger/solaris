//! Staging of one callback's commands.
//!
//! A callback's commands belong to the call that produced them until the host
//! has checked the whole batch. `CommandBatch` is that staging area: it refuses
//! to grow past the configured bound, so a guest cannot turn one callback into an
//! unbounded amount of host work, and it is dropped unapplied when the callback
//! traps, runs out of budget, or answers something the contract does not allow.

use crate::PluginLimits;
use crate::bindings::solaris::plugin::commands::Command;

/// Why a batch was refused while it was being staged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StagingError {
    /// The callback returned more commands than one call may produce.
    #[error("batch exceeds the command bound of this plugin")]
    TooManyCommands,
    /// One returned string exceeded the contract's text bound.
    #[error("batch carries text past the configured bound")]
    TextTooLong,
}

/// Commands one callback produced, not yet applied anywhere.
#[derive(Debug, Default)]
pub struct CommandBatch {
    commands: Vec<Command>,
    text_bytes: usize,
}

impl CommandBatch {
    /// An empty batch.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Stage one command, or refuse it and the whole batch.
    ///
    /// The check is on the *staged* size, so the caller cannot learn after the
    /// fact that it staged too much: the first refused command fails the batch.
    pub fn push(&mut self, command: Command, limits: &PluginLimits) -> Result<(), StagingError> {
        if self.commands.len() >= limits.commands_per_call {
            return Err(StagingError::TooManyCommands);
        }
        let text = command_text(&command);
        if text.len() > limits.text_bytes {
            return Err(StagingError::TextTooLong);
        }
        self.text_bytes += text.len();
        self.commands.push(command);
        Ok(())
    }

    /// Stage a whole lifted batch, refusing the same way.
    pub fn extend(
        &mut self,
        commands: Vec<Command>,
        limits: &PluginLimits,
    ) -> Result<(), StagingError> {
        for command in commands {
            self.push(command, limits)?;
        }
        Ok(())
    }

    /// How many commands are staged.
    #[must_use]
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Whether nothing was staged.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Total text bytes staged, which is the host allocation the guest can drive.
    #[must_use]
    pub fn text_bytes(&self) -> usize {
        self.text_bytes
    }

    /// Take the staged commands for the admissible path.
    #[must_use]
    pub fn into_commands(self) -> Vec<Command> {
        self.commands
    }
}

/// The text one command carries, for the bound above.
fn command_text(command: &Command) -> &str {
    match command {
        Command::SendMessage(message) => &message.text,
        // The correspondence id is bounded by the contract too, but it is the key
        // and the value that carry the plugin's data, so those are what counts
        // against the batch.
        Command::StorageGet(get) => &get.key,
        Command::StorageCas(cas) => &cas.value,
        // A query carries no text of its own: only the correlation id, which the
        // contract bounds at 64 bytes.
        Command::ListOnlinePlayers(query) => &query.request,
        // A teleport carries no text either: the correlation id is its only
        // string and the coordinates are numbers.
        Command::TeleportPlayer(teleport) => &teleport.request,
        // A batch carries many strings rather than one, and the DTO already bounds
        // every one of them: at most 16 keys of 128 bytes and 16 values of 4096, so
        // the whole batch is at most ~66 KiB by construction. This per-command text
        // bound is below that, so counting a key or a value here would refuse a
        // batch the server's own DTO admits; the one string the DTO does not bound
        // at a length of its own is the correlation id.
        Command::StorageBatchCas(batch) => &batch.request,
        // A probe carries the two ids and nothing else.
        Command::OperationStatus(status) => &status.request,
        // A zone command carries a box rather than data: its id, its dimension and
        // its protection uuid are all bounded by the server's own DTO (64, 128 and
        // 36 bytes) and the record has no list, so the whole answer is a few hundred
        // bytes by construction. The dimension is the longest string in it, and
        // counting that one against this bound is honest about the record's size
        // without refusing a zone the DTO admits.
        Command::UpsertZone(zone) => &zone.dimension,
        Command::UpsertProtectedZone(zone) => &zone.dimension,
        // A removal carries one id, the contract's 64-byte script id.
        Command::RemoveZone(remove) => &remove.zone,
    }
}
