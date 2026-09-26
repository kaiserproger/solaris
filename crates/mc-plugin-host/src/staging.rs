//! Staging of one callback's commands.
//!
//! A callback's commands belong to the call that produced them until the host
//! has checked the whole batch. `CommandBatch` is that staging area: it refuses
//! to grow past the configured bound, so a guest cannot turn one callback into an
//! unbounded amount of host work, and it is dropped unapplied when the callback
//! traps, runs out of budget, or answers something the contract does not allow.

use crate::PluginLimits;
use crate::bindings::solaris::plugin::client_presentation::{
    ClientCommand, ViewFieldValue, ViewModel,
};
use crate::bindings::solaris::plugin::commands::{Command, StorageMutation};

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
        if command_text_bytes(&command) > limits.text_bytes {
            return Err(StagingError::TextTooLong);
        }
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

    /// Take the staged commands for the admissible path.
    #[must_use]
    pub fn into_commands(self) -> Vec<Command> {
        self.commands
    }

    /// Remove host-local timer requests without rebuilding the validated batch.
    pub(crate) fn split_timers(
        mut self,
        tick: u64,
    ) -> Result<(Self, Vec<crate::timers::TimerMutation>), crate::timers::TimerRefusal> {
        let (commands, mutations) = crate::timers::split(self.commands, tick)?;
        self.commands = commands;
        Ok((self, mutations))
    }
}

/// Per-string text size checked before conversion.
fn command_text_bytes(command: &Command) -> usize {
    match command {
        Command::SendMessage(message) => message.text.len(),
        Command::Broadcast(message) => message.len(),
        Command::DisconnectPlayer(disconnect) => disconnect.reason.len(),
        // The correspondence id is bounded by the contract too, but it is the key
        // and the value that carry the plugin's data, so those are what counts
        // against the batch.
        Command::StorageGet(get) => get.key.len(),
        Command::StorageCas(cas) => cas.value.len(),
        // A query carries no text of its own: only the correlation id, which the
        // contract bounds at 64 bytes.
        Command::ListOnlinePlayers(query) => query.request.len(),
        // A teleport carries no text either: the correlation id is its only
        // string and the coordinates are numbers.
        Command::TeleportPlayer(teleport) => teleport.request.len(),
        // A batch carries many strings rather than one, and the DTO already bounds
        // every one of them: at most 16 keys of 128 bytes and 16 values of 4096, so
        // the whole batch is at most ~66 KiB by construction. This per-command text
        // bound is below that, so counting a key or a value here would refuse a
        // batch the server's own DTO admits; the one string the DTO does not bound
        // at a length of its own is the correlation id.
        Command::StorageBatchCas(batch) => batch.request.len(),
        // A probe carries the two ids and nothing else.
        Command::OperationStatus(status) => status.request.len(),
        Command::Operation(request) => {
            use crate::bindings::solaris::plugin::domain_operations::DomainOperation;
            let largest = match &request.operation {
                DomainOperation::Settlement(value) => {
                    crate::domain_settlements::max_text_bytes(value)
                }
                DomainOperation::Resident(value) => crate::domain_residents::max_text_bytes(value),
                DomainOperation::ResidentOrder(value) => {
                    crate::domain_residents::max_order_text_bytes(value)
                }
                DomainOperation::Inventory(value) => {
                    crate::domain_inventories::max_text_bytes(value)
                }
            };
            request.request.len().max(largest)
        }
        // A zone command carries a box rather than data: its id, its dimension and
        // its protection uuid are all bounded by the server's own DTO (64, 128 and
        // 36 bytes) and the record has no list, so the whole answer is a few hundred
        // bytes by construction. The dimension is the longest string in it, and
        // counting that one against this bound is honest about the record's size
        // without refusing a zone the DTO admits.
        Command::UpsertZone(zone) => zone.dimension.len(),
        Command::UpsertProtectedZone(zone) => zone.dimension.len(),
        // A removal carries one id, the contract's 64-byte script id.
        Command::RemoveZone(remove) => remove.zone.len(),
        // A timer request carries its id and, for a schedule, a delay; the id is
        // the contract's own script id and the host's timer bounds are what refuse
        // one the contract does not admit, so the id is what counts here.
        Command::ScheduleTimer(timer) => timer.timer_id.len(),
        Command::CancelTimer(timer) => timer.timer_id.len(),
        Command::InventoryStorageTransaction(transaction) => transaction
            .inventory
            .iter()
            .map(|delta| delta.resource.len())
            .chain(transaction.storage.iter().map(|mutation| match mutation {
                StorageMutation::Cas(cas) => cas.key.len().max(cas.value.len()),
                StorageMutation::Delete(delete) => delete.key.len(),
            }))
            .fold(transaction.request.len(), usize::max),
        // A menu carries many strings rather than one, and the DTO already bounds
        // every one of them: a 64-byte id, a 128-byte title and at most 54 slots of
        // a 128-byte resource and a 128-byte label, which is ~14 KiB for the whole
        // record. This per-string bound is below that, so charging every slot here
        // would refuse a menu the server's own DTO admits - the same reason the
        // transaction above is charged its longest string rather than its whole
        // content. The whole answer stays bounded by the transfer budget the store
        // was built with, which refuses an answer past `PluginLimits::hostcall_bytes`
        // before Wasmtime lifts any of it.
        Command::OpenInventoryMenu(open) => open
            .menu
            .slots
            .iter()
            .map(|slot| {
                slot.resource
                    .len()
                    .max(slot.label.as_ref().map_or(0, String::len))
            })
            .fold(open.menu.id.len().max(open.menu.title.len()), usize::max),
        // A close carries one menu id, the contract's own 64-byte script id.
        Command::CloseInventoryMenu(close) => close.menu.len(),
        // A Loader presentation carries its data nested inside a view model, as a
        // menu does, so the whole request is charged its longest string rather than
        // its sum: the DTO already bounds every nested string (a 256-byte cell and
        // text value, 128-byte ids, labels and tokens, a 256-byte reason) and caps
        // the lists (64 rows of 16 cells, 16 of every other list), so charging the
        // sum would refuse a model the server's own DTO admits - the same reason the
        // menu and the transaction above are charged their longest string. What
        // matters is that no nested string of the record is forgotten, and none is:
        // every cell, field value, action label, deny reason, tab label, resource
        // id, marker token and the model's own reason counts here, while the
        // aggregate cost of lifting the record stays bounded by
        // `PluginLimits::hostcall_bytes` before Wasmtime copies any of it.
        Command::ClientPresentation(request) => client_presentation_text_bytes(request),
    }
}

/// The longest string one client-presentation request carries.
///
/// Every string inside a nested view model is counted, so no string the contract
/// bounds is one this check never looked at.
fn client_presentation_text_bytes(request: &ClientCommand) -> usize {
    match request {
        ClientCommand::OpenClientView(open) => open
            .request
            .len()
            .max(open.owned_view_id.len())
            .max(view_model_text_bytes(&open.model)),
        ClientCommand::PresentClientView(present) => present
            .view_instance_id
            .len()
            .max(view_model_text_bytes(&present.model)),
        ClientCommand::CloseClientView(close) => close.view_instance_id.len(),
        ClientCommand::BeginClientSelection(begin) => begin
            .request
            .len()
            .max(begin.view_instance_id.len())
            .max(begin.action_id.len())
            .max(begin.dimension.len()),
        ClientCommand::CancelClientSelection(cancel) => cancel.selection_context_id.len(),
        ClientCommand::PlayClientSound(play) => play.sound_id.len(),
        ClientCommand::StopClientSound(stop) => stop.sound_id.len(),
        ClientCommand::GrantLoaderBlockItem(grant) => grant.request.len().max(grant.block.len()),
    }
}

/// The longest string any part of one view model carries: its rows' cells, its
/// fields' ids and typed values, its actions' ids, labels and deny reasons, its
/// tabs' ids and labels, its resource entries' ids, its markers' ids, tokens and
/// action ids, and its own reason.
fn view_model_text_bytes(model: &ViewModel) -> usize {
    let mut longest = model.reason.as_ref().map_or(0, String::len);
    for row in &model.rows {
        for cell in &row.cells {
            longest = longest.max(cell.len());
        }
    }
    for field in &model.fields {
        longest = longest.max(field.id.len());
        longest = longest.max(match &field.value {
            ViewFieldValue::Number(_) => 0,
            ViewFieldValue::Text(text) => text.len(),
            ViewFieldValue::Selected(selected) => selected.len(),
        });
    }
    for action in &model.actions {
        longest = longest.max(action.action_id.len());
        longest = longest.max(action.label.as_ref().map_or(0, String::len));
        longest = longest.max(action.deny_reason.as_ref().map_or(0, String::len));
    }
    for tab in &model.tabs {
        longest = longest.max(tab.id.len());
        longest = longest.max(tab.label.len());
    }
    for entry in &model.resource_entries {
        longest = longest.max(entry.id.len());
    }
    for marker in &model.markers {
        longest = longest.max(marker.marker_id.len());
        longest = longest.max(marker.selection_token.as_ref().map_or(0, String::len));
        longest = longest.max(marker.action_id.as_ref().map_or(0, String::len));
    }
    longest
}
