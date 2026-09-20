//! Guest SDK for Solaris plugins.
//!
//! One source of truth: the WIT package in the core repository
//! (`crates/mc-script/wit`). This crate generates the guest bindings from it, so
//! a plugin cannot drift from the contract the server was built against, and the
//! host cannot generate a second, hand-written copy of the same types.
//!
//! A plugin implements [`Plugin`] and exports it with [`export_plugin!`]. The
//! SDK owns everything else: the generated ABI glue, config parsing, and the
//! conversion between the guest-facing Rust types and the contract's own types,
//! which are the same types.

/// The generated guest side of the `solaris:plugin` contract.
///
/// The bindings live in a named module so the export macro the guest invokes can
/// name them by an absolute path: a plugin crate expands its own `#[no_mangle]`
/// exports here, which is why this crate cannot simply re-export the generated
/// macro under its own root.
pub mod bindings {
    wit_bindgen::generate!({
        path: "../../../crates/mc-script/wit",
        world: "plugin",
        pub_export_macro: true,
        default_bindings_module: "solaris_plugin_sdk::bindings",
    });
}

pub use bindings::export as __export;
pub use bindings::exports::solaris::plugin::{events, lifecycle, precommit};
pub use bindings::solaris::plugin::{
    client_presentation, commands, domain_operations, host, inventories, operation_types,
    residents, settlements, storage, types, world_events,
};

mod glue;
mod inventory_ops;
mod resident_ops;
mod settlement_ops;

pub use inventory_ops::*;
pub use resident_ops::*;
pub use settlement_ops::*;

pub use client_presentation::{
    ClientCommand, CloseClientView, GrantLoaderBlockItem, OpenClientView, PlayClientSound,
    PresentClientView, StopClientSound, ViewAction, ViewField, ViewFieldValue, ViewFormation,
    ViewMarker, ViewModel, ViewResourceEntry, ViewRow, ViewTab,
};
pub use commands::{
    CancelTimer, Command, DisconnectPlayer, InventoryMenu, InventoryMenuSlot,
    InventoryResourceDelta, InventoryStorageTransaction, ListOnlinePlayers, MessageTarget,
    ScheduleTimer, SendMessage, StorageCas, StorageGet, StorageMutation,
};
pub use events::{
    ClientViewOpened, ClientViewOutcome, Event, EventContext, InventoryClick, InventoryMenuClicked,
    InventoryStorageOutcome, InventoryStorageTransactionAnswered, LoaderItemGrantFailure,
    LoaderItemGrantOutcome, LoaderItemGrantResult, LoaderViewAction, LoaderViewRequest,
    PlayerZoneTransition, TimerFired, ViewFailure, ViewOpened, ViewRequestKind,
};
pub use lifecycle::{InitContext, StartupContribution};
pub use precommit::{
    BuildContext, BuildDecision, BuildEdit, DamageContext, DamageDecision, DamageTarget, HookActor,
    HookPlayer,
};
pub use storage::{StorageCasOutcome, StorageFailure, StorageGetOutcome, StorageRecord};
pub use types::{LogLevel, PluginError};
pub use world_events::{
    BlockPosition, PlayerBlockBroken, PlayerBlockPlaced, PlayerDied, PlayerEntityInteracted,
    PlayerEntityKilled, PlayerItemCrafted, PlayerItemPickedUp,
};

/// Failures a plugin reports back to the host.
///
/// The host maps these to the same variants the operator sees, so a plugin's own
/// refusals stay distinguishable from a host refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The plugin asked for something its manifest does not grant.
    Denied,
    /// The plugin's own input was invalid.
    Invalid,
    /// The plugin reached a budget it owns.
    Budget,
    /// The named object is gone.
    NotFound,
    /// The call arrived in a phase that does not allow it.
    Unexpected,
    /// The plugin itself failed.
    Failed,
}

impl From<Failure> for PluginError {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Denied => PluginError::Denied,
            Failure::Invalid => PluginError::Invalid,
            Failure::Budget => PluginError::Budget,
            Failure::NotFound => PluginError::NotFound,
            Failure::Unexpected => PluginError::Unexpected,
            Failure::Failed => PluginError::Failed,
        }
    }
}

/// The package's `config.toml`, parsed by the guest.
///
/// The host does not interpret package configuration: it hands over the text and
/// the plugin decides what it means. The same text reaches both [`Plugin::configure`]
/// and [`Plugin::init`], each in its own store. A plugin that expects no
/// configuration ignores the argument entirely.
#[derive(Debug, Clone, Default)]
pub struct Config {
    text: String,
}

impl Config {
    /// Wrap the raw configuration text the host delivered.
    #[must_use]
    pub fn new(text: &str) -> Self {
        Self {
            text: text.to_owned(),
        }
    }

    /// The raw text, for a plugin with its own format.
    #[must_use]
    pub fn raw(&self) -> &str {
        &self.text
    }

    /// Parse the text as a TOML value, or `None` when it is empty or invalid.
    #[must_use]
    pub fn toml(&self) -> Option<toml::Value> {
        if self.text.trim().is_empty() {
            return None;
        }
        self.text.parse().ok()
    }
}

/// One plugin, as its author writes it.
///
/// Every method has a default, so a plugin states only the phases it uses. Each
/// answer's commands are staged by the host and applied only after the whole
/// batch is admissible; nothing here changes the world by itself.
///
/// Implementing [`Plugin::before_build`] or [`Plugin::before_damage`] publishes a
/// capability of this component and registers nothing: the host asks a hook only
/// for the packages an operator explicitly registered for that kind, in the
/// roster's order, and a component that is registered for neither is asked
/// neither.
pub trait Plugin: Sized + 'static {
    /// Startup phase: read the configuration and answer a startup contribution.
    ///
    /// It runs in a short-lived store with no runtime capability, and the
    /// contribution it returns is fingerprinted into the world contract, so the
    /// same configuration must always produce the same contribution.
    ///
    /// The store this phase runs in is dropped before the store
    /// [`Plugin::init`] runs in exists, so nothing mutated here is runtime
    /// state: initialize the fields the instance runs with in [`Plugin::init`]
    /// from the same configuration instead of retaining them from this phase.
    fn configure(&mut self, _config: &Config) -> Result<Option<StartupContribution>, Failure> {
        Ok(None)
    }

    /// Runtime phase, once per instance. Its commands are applied before the
    /// plugin receives events.
    ///
    /// This is the only phase with runtime state: build what the instance needs
    /// here from `config` and `context`, rather than from fields kept during
    /// [`Plugin::configure`], whose store no longer exists.
    fn init(&mut self, _config: &Config, _context: &InitContext) -> Result<Vec<Command>, Failure> {
        Ok(Vec::new())
    }

    /// One bounded batch of events, in delivery order for this plugin.
    fn on_events(
        &mut self,
        _context: &EventContext,
        _events: &[Event],
    ) -> Result<Vec<Command>, Failure> {
        Ok(Vec::new())
    }

    /// One before-build question, asked only for a registration an operator made
    /// for this kind.
    ///
    /// The context carries the dimension and the edits an actor is about to
    /// commit; the answer is `Keep` to let them through or `Cancel` to refuse the
    /// whole batch. Nothing here can stage a command: the host is asking about an
    /// effect it has not committed yet, and the contract has no shape in this
    /// question that carries one. The default keeps every build, so a plugin that
    /// states nothing about builds never changes one.
    ///
    /// The host runs this inside the deadline that covers the whole chain of
    /// registered handlers, in a phase where the logging import and every command
    /// and asynchronous import are refused: keep whatever a later callback should
    /// report in this instance's own state and answer from it in
    /// [`Plugin::on_events`] instead. Answering from state the callback itself
    /// maintains is also what makes the decision reproducible, because the same
    /// context must keep deciding the same way.
    fn before_build(&mut self, _context: &BuildContext) -> Result<BuildDecision, Failure> {
        Ok(BuildDecision::Keep)
    }

    /// One before-damage question, asked only for a registration an operator made
    /// for this kind.
    ///
    /// `context.amount` is the raw amount about to be applied, before armor,
    /// shields or resistance; `Keep` applies it, `Cancel` refuses the damage
    /// entirely, and `Replace(amount)` names the finite, non-negative amount to
    /// apply instead - zero being a deliberately admitted no-damage outcome. The
    /// chain is a pipeline while it keeps: a `Replace` an earlier handler decided
    /// is the amount a later handler is asked about. A `Cancel` settles the chain
    /// instead: no later handler is asked and the damage is refused whatever the
    /// handlers before it decided, so an earlier `Replace` is discarded. The
    /// default keeps the amount it was asked about.
    fn before_damage(&mut self, _context: &DamageContext) -> Result<DamageDecision, Failure> {
        Ok(DamageDecision::Keep)
    }

    /// Best-effort cleanup. The host reclaims everything it owns whether or not
    /// this runs, and a crash skips it, so no durable decision may depend on it.
    fn shutdown(&mut self) -> Result<(), Failure> {
        Ok(())
    }
}

/// One line of chat to one player, addressed by their stable identity.
#[must_use]
pub fn message_player(player: &str, text: impl Into<String>) -> Command {
    Command::SendMessage(SendMessage {
        target: MessageTarget::Player(player.to_owned()),
        text: text.into(),
    })
}

/// One line of chat to the session that caused an operation.
///
/// Use this for command replies and deferred result notifications: a reconnect
/// must not redirect an old operation's reply to the player's new connection.
#[must_use]
pub fn message_session(session: u64, text: impl Into<String>) -> Command {
    Command::SendMessage(SendMessage {
        target: MessageTarget::Session(session),
        text: text.into(),
    })
}

/// One chat line to every online player, admitted with the rest of this batch.
#[must_use]
pub fn broadcast(text: impl Into<String>) -> Command {
    Command::Broadcast(text.into())
}

/// Disconnect exactly this session; never resolve a replacement connection.
#[must_use]
pub fn disconnect_player(session: types::SessionId, reason: impl Into<String>) -> Command {
    Command::DisconnectPlayer(DisconnectPlayer {
        session,
        reason: reason.into(),
    })
}

/// Read one key of this plugin's own durable storage.
///
/// The answer arrives later as [`Event::StorageGetAnswered`] with the same
/// `request`; nothing about this call waits for storage.
#[must_use]
pub fn storage_get(request: &str, key: &str) -> Command {
    Command::StorageGet(StorageGet {
        request: request.to_owned(),
        key: key.to_owned(),
    })
}

/// Compare-and-swap one key of this plugin's own durable storage.
///
/// `expected_version` is the revision the swap expects; `None` means "only if
/// the key holds nothing". A swap that does not commit answers `refused` without
/// saying why, so a plugin that must know re-reads the key with a second
/// `storage_get` and decides again. Versions come from the server; a plugin never
/// picks one.
#[must_use]
pub fn storage_cas(
    request: &str,
    key: &str,
    expected_version: Option<u64>,
    value: impl Into<String>,
) -> Command {
    Command::StorageCas(StorageCas {
        request: request.to_owned(),
        key: key.to_owned(),
        expected_version,
        value: value.into(),
    })
}

/// Read who is connected right now, at most `limit` players.
///
/// The answer arrives later as [`Event::OnlinePlayersAnswered`] with the same
/// `request`. The limit is the plugin's own bound: the server never answers more
/// than it, and sets `truncated` when more players were connected.
#[must_use]
pub fn list_online_players(request: &str, limit: u32) -> Command {
    Command::ListOnlinePlayers(ListOnlinePlayers {
        request: request.to_owned(),
        limit,
    })
}

/// One atomic transaction of one session's inventory resources and this plugin's
/// own durable storage.
///
/// Both sides commit together or neither does: the server's own owners re-check
/// the session, the item counts and every storage revision, and the answer
/// arrives later as [`Event::InventoryStorageTransactionAnswered`] with the same
/// `request`. `request` is correlation only - the command has no durable
/// operation id - so a plugin reuses it for a later transaction rather than
/// reading it as a receipt.
///
/// The transaction names the session, not the stable identity: it changes one
/// live connection's inventory, so a transaction for a connection that has ended
/// is refused instead of reaching whoever holds the identity now. It is admitted
/// only for a package whose manifest declares `inventory_storage_transactions`;
/// a key, a value or an amount the contract does not admit fails the whole batch.
#[must_use]
pub fn inventory_storage_transaction(
    request: &str,
    session: types::SessionId,
    inventory: Vec<InventoryResourceDelta>,
    storage: Vec<StorageMutation>,
) -> Command {
    Command::InventoryStorageTransaction(InventoryStorageTransaction {
        request: request.to_owned(),
        session,
        inventory,
        storage,
    })
}

/// Ask the host to show one menu to the client of one live session.
///
/// The menu is built by the plugin and owned by the server once admitted: it
/// renders the slots, keeps the menu for that connection, and routes a click on
/// one of its slots back as [`Event::InventoryMenuClicked`] carrying this `id`.
/// The session is named rather than the stable identity, because a menu exists on
/// one connection: a request for a session that has ended is refused by the
/// server's own menu owner instead of appearing for whoever holds the identity
/// now.
///
/// Nothing here validates the definition and nothing waits: the host converts the
/// record under the package's `inventory_menus` grant, the server's own
/// constructors refuse a title, an index, a resource, a count or a label the
/// contract does not admit - which fails the whole batch as this plugin's own
/// malformed answer - and a menu the owner refuses produces no answer at all.
/// Opening is fire-and-forget, so a plugin must not read the absence of an answer
/// as "the client saw the menu": its own clicks are the only confirmation it gets.
#[must_use]
pub fn open_inventory_menu(
    session: types::SessionId,
    id: &str,
    title: &str,
    slots: Vec<InventoryMenuSlot>,
) -> Command {
    Command::OpenInventoryMenu(commands::OpenInventoryMenu {
        session,
        menu: InventoryMenu {
            id: id.to_owned(),
            title: title.to_owned(),
            slots,
        },
    })
}

/// Ask the host to close one menu this plugin opened for one live session.
///
/// `menu` is the id the matching [`open_inventory_menu`] named, and the session is
/// the connection whose client is showing it: a close for a session that has ended
/// is refused rather than closing the menu of whoever holds the identity now, and
/// closing a menu that connection is not showing is nothing to close rather than a
/// refusal the plugin could act on. Like opening, it is fire-and-forget: no event
/// of the contract answers it, and no answer is invented for it here.
#[must_use]
pub fn close_inventory_menu(session: types::SessionId, menu: &str) -> Command {
    Command::CloseInventoryMenu(commands::CloseInventoryMenu {
        session,
        menu: menu.to_owned(),
    })
}

/// Ask the host to open a view this plugin owns on the client of one live
/// session.
///
/// The view is built by the plugin and owned by the server once admitted: the
/// server assigns the instance id and the first revision, keeps them for that
/// connection, and routes a later action on the instance back as
/// [`Event::LoaderViewAction`]. The session is named rather than the stable
/// identity, because a view exists on one acknowledged Loader connection: a
/// request for a session that has ended is refused by the server's own Loader
/// owner instead of appearing for whoever holds the identity now.
///
/// Nothing here validates the model and nothing waits: the host converts the
/// record under the package's Loader content and permission pair, the server's
/// own constructors refuse a page, a list, an id, a cell or a value the contract
/// does not admit - which fails the whole batch as this plugin's own malformed
/// answer - and the outcome arrives later as [`Event::ClientViewOpened`] with the
/// same `request`, carrying the instance and revision to present against or the
/// owner's refusal.
#[must_use]
pub fn open_client_view(
    request: &str,
    session: types::SessionId,
    owned_view_id: &str,
    model: ViewModel,
) -> Command {
    Command::ClientPresentation(ClientCommand::OpenClientView(OpenClientView {
        request: request.to_owned(),
        session,
        owned_view_id: owned_view_id.to_owned(),
        model,
    }))
}

/// Replace the model of a view this plugin already opened.
///
/// `expected_revision` is the revision the matching [`Event::ClientViewOpened`]
/// reported: the server presents the new model only while the live instance still
/// holds it, so a request after the instance moved on is refused rather than
/// overwriting a model the plugin never saw. A replacement drops every selection
/// context the earlier model armed. Like closing, it is fire-and-forget: no event
/// of the contract answers it, and a plugin that must know whether the client sees
/// the new model hears about it through the next action instead.
#[must_use]
pub fn present_client_view(
    session: types::SessionId,
    view_instance_id: &str,
    expected_revision: u64,
    model: ViewModel,
) -> Command {
    Command::ClientPresentation(ClientCommand::PresentClientView(PresentClientView {
        session,
        view_instance_id: view_instance_id.to_owned(),
        expected_revision,
        model,
    }))
}

/// Close a view this plugin opened on one live session.
///
/// A close for a session that has ended is refused rather than closing the view
/// of whoever holds the identity now, and closing an instance that connection is
/// not showing is nothing to close rather than a refusal the plugin could act on.
/// No event of the contract answers it.
#[must_use]
pub fn close_client_view(session: types::SessionId, view_instance_id: &str) -> Command {
    Command::ClientPresentation(ClientCommand::CloseClientView(CloseClientView {
        session,
        view_instance_id: view_instance_id.to_owned(),
    }))
}

/// Play one sound this plugin owns on the client of one live session.
///
/// `volume` is 0.0..=1.0 and `pitch` is 0.5..=2.0, as the audio channel defines
/// them; a `position` places the sound in the world, and `None` plays it
/// listener-relative and unattenuated. The sound id must be namespaced to this
/// plugin, and the Loader manifest must carry the sound content and its play
/// permission: the server's own owners refuse a sound this plugin does not own or
/// a session that is gone, and no event of the contract answers a play.
#[must_use]
pub fn play_client_sound(
    session: types::SessionId,
    sound_id: &str,
    volume: f32,
    pitch: f32,
    position: Option<types::Position>,
) -> Command {
    Command::ClientPresentation(ClientCommand::PlayClientSound(PlayClientSound {
        session,
        sound_id: sound_id.to_owned(),
        volume,
        pitch,
        position,
    }))
}

/// Stop a sound this plugin is playing on one live session's client.
///
/// The session and the id are the ones the matching [`play_client_sound`] named:
/// a stop for a session that has ended is refused rather than silencing a sound
/// on whoever holds the identity now, and stopping a sound that connection is not
/// playing is nothing to stop. No event of the contract answers it.
#[must_use]
pub fn stop_client_sound(session: types::SessionId, sound_id: &str) -> Command {
    Command::ClientPresentation(ClientCommand::StopClientSound(StopClientSound {
        session,
        sound_id: sound_id.to_owned(),
    }))
}

/// Grant one custom block item this plugin owns into one live session's inventory.
///
/// The grant goes through the server's own player-inventory authority, which
/// re-checks that the block belongs to this plugin's Loader bundle, that the
/// connection is still there and that the inventory can take the items; `count`
/// is 1..=64. The session is named rather than the stable identity, so a grant
/// for a connection that has ended is refused instead of reaching whoever holds
/// the identity now. The result arrives later as
/// [`Event::LoaderItemGrantResult`] with the same `request`, and it is the only
/// thing that tells a plugin whether the item reached the player.
#[must_use]
pub fn grant_loader_block_item(
    request: &str,
    session: types::SessionId,
    block: &str,
    count: u8,
) -> Command {
    Command::ClientPresentation(ClientCommand::GrantLoaderBlockItem(GrantLoaderBlockItem {
        request: request.to_owned(),
        session,
        block: block.to_owned(),
        count,
    }))
}

/// Ask the host to fire [`Event::TimerFired`] for this plugin `delay_ticks`
/// simulation ticks from now.
///
/// The deadline is the tick of the event the call answers (`init` uses the tick
/// the host observed for it) plus `delay_ticks`, added with an overflow check:
/// the host refuses a request outside 1..=630720000 ticks, and never moves a
/// timer backwards. A plugin may hold at most 256 pending timers; scheduling an
/// id it already holds moves that timer to the new deadline instead of adding
/// one, which is the one scheduling request a full plugin may still make.
/// Nothing here waits: the fire arrives as its own event batch, and the event
/// names the id, the tick it was scheduled for and the tick it fired at, so a
/// plugin that rescheduled from a callback reads the tick the host actually
/// observed rather than the deadline it first named.
///
/// Timers are the plugin's own and live only in its instance: a restart forgets
/// them, and no other package can name one of them.
#[must_use]
pub fn schedule_timer(timer_id: &str, delay_ticks: u64) -> Command {
    Command::ScheduleTimer(ScheduleTimer {
        timer_id: timer_id.to_owned(),
        delay_ticks,
    })
}

/// Drop one timer this plugin scheduled, so it will not fire.
///
/// Only the plugin's own ids are named, so a plugin can cancel nothing but what
/// it scheduled itself. An id the plugin is not holding is nothing to cancel
/// rather than a refusal, and cancelling a timer that is already due - including
/// one due later in the same batch of fires - is how an earlier callback drops a
/// later one.
#[must_use]
pub fn cancel_timer(timer_id: &str) -> Command {
    Command::CancelTimer(CancelTimer {
        timer_id: timer_id.to_owned(),
    })
}

/// One diagnostic line.
pub fn log(level: LogLevel, message: &str) {
    #[cfg(target_arch = "wasm32")]
    host::log(level, message);

    #[cfg(not(target_arch = "wasm32"))]
    let _ = (level, message);
}

/// The plugin id the host bound to this instance.
#[must_use]
pub fn plugin_id() -> String {
    host::plugin_id()
}
