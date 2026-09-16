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
pub use bindings::exports::solaris::plugin::{events, lifecycle};
pub use bindings::solaris::plugin::{commands, host, storage, types};

mod glue;

pub use events::{Event, EventContext};
pub use lifecycle::{InitContext, RulePlan};
pub use commands::{
    Command, ListOnlinePlayers, MessageTarget, SendMessage, StorageCas, StorageGet,
};
pub use storage::{StorageCasOutcome, StorageFailure, StorageGetOutcome, StorageRecord};
pub use types::{LogLevel, PluginError};

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
/// the plugin decides what it means. A plugin that expects no configuration
/// ignores the argument entirely.
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
pub trait Plugin: Sized + 'static {
    /// Startup phase: read the configuration and answer a rule plan.
    ///
    /// It runs in a short-lived store with no runtime capability, and the plan it
    /// returns is fingerprinted into the world contract, so the same
    /// configuration must always produce the same plan.
    fn configure(&mut self, _config: &Config) -> Result<Option<RulePlan>, Failure> {
        Ok(None)
    }

    /// Runtime phase, once per instance. Its commands are applied before the
    /// plugin receives events.
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

/// One diagnostic line.
pub fn log(level: LogLevel, message: &str) {
    host::log(level, message);
}

/// The plugin id the host bound to this instance.
#[must_use]
pub fn plugin_id() -> String {
    host::plugin_id()
}
