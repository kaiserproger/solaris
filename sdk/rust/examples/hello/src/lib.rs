//! Example Solaris plugin.
//!
//! By default it is server-only and does two things a player can see: it greets a player who
//! joins, and it answers the `/hello` command. Both are staged commands the host
//! admits and applies; the plugin never touches the world itself.
//!
//! The two startup phases are separate here exactly as the host separates them:
//! `configure` answers the startup contribution and leaves nothing behind, and
//! `init` reads the same configuration again to build the runtime state every
//! later callback runs on. Nothing is carried from one phase to the other.
//!
//! The configuration is optional. With `greeting = "..."` in `config.toml` the
//! plugin uses that text; the fixture also reads `mode` to exercise the host's
//! budgets from the guest side (`spin` never returns, `grow` allocates without
//! bound, `oversized`/`wide`/`nested` answer more than a callback may hand the
//! host, `trap` builds a batch and then faults, `refuse-configure` answers a
//! plugin error from the startup phase), which is how the host's limit tests
//! drive a misbehaving guest with a real component instead of a hand-built
//! module. `mode = "configure-isolation"` is the probe of the host's own phase
//! separation: it writes a marker while answering the contribution and reports in
//! `init` whether that marker survived. `mode = "storage-compat"` is no
//! misbehaviour: it runs one phase of the compatibility fixture in
//! `storage_compat.rs`, named by `storage_phase`.
//! `mode = "inventory-storage"` is the P3 acceptance fixture in
//! `inventory_storage.rs`: a player's `trade` command answers one atomic
//! inventory-and-storage transaction per action and reports what the owners did.
//! `mode = "menu-observer"` is the second package the menu wire test deploys: it
//! subscribes to `inventory.menu.clicked` and owns no menu, so it answers a click
//! - if one ever reached it - with a line a test reads, and answers its own
//! `menu-fence` command with another, which is how the same test knows the
//! package is live while it proves a click reaches only the plugin that opened
//! the menu.
//! `mode = "zone-market"` is the P3 zone acceptance fixture in `zone_market.rs`:
//! it registers the fixed `trade-zone` box through the existing upsert command,
//! reports its readiness only from the owner's own applied answer, and lets the
//! owner's boundary transitions open and close the market the inventory-storage
//! fixture already serves. `mode = "zone-observer"` is the second package that
//! test deploys: it subscribes to `player.zone_entered`/`player.zone_exited` and
//! owns no zone, so it reports any transition that ever reached it and answers
//! its own `zone-fence` command, which is what makes its silence about the
//! owner's transitions a result.

use solaris_plugin_sdk::events::{
    OperationOutcome, OperationPayload, PlayerTeleportFailure, PlayerTeleportOutcome,
    ZoneCommandOutcome,
};
use solaris_plugin_sdk::operation_types::OperationFailure;
use solaris_plugin_sdk::{
    broadcast, commands, disconnect_player, export_plugin, lifecycle, list_online_players, log,
    message_player, storage, storage_cas, storage_get, types, BuildContext, BuildDecision, Command,
    Config, DamageContext, DamageDecision, Event, EventContext, Failure, InitContext,
    InventoryClick, LogLevel, Plugin, StartupContribution, StorageCasOutcome, StorageFailure,
    StorageGetOutcome,
};

mod audit_events;
mod inventory_ops;
mod inventory_storage;
mod loader_live;
mod precommit;
mod resident_ops;
mod settlement_ops;
mod storage_compat;
mod timers;
mod zone_market;

/// The command root the observer package's manifest declares: the one command a
/// test runs to hear from it, so that its silence on another package's menu click
/// is a result instead of a missing deployment.
const MENU_FENCE: &str = "menu-fence";

/// The zone fixture's second package, the same way: its own command root, the line
/// it answers that command with, and the prefix of the line it would answer a zone
/// transition with if a transition ever reached a package that owns no zone.
const ZONE_FENCE: &str = "zone-fence";
const ZONE_FENCE_LINE: &str = "P3_ZONE_FENCE";
const ZONE_LEAK_PREFIX: &str = "P3_ZONE_LEAK";

/// How many times the blocking mode calls the slow import. More than one, so the
/// guest returns to its own code - and to the host's epoch check - after the call.
const BLOCKING_LOGS: usize = 8;

/// Two messages per join, so a host whose command queue holds one refuses the
/// second without the guest misbehaving.
const BURST: usize = 2;

/// The one message a test-side host service treats as slow.
const BLOCK_SENTINEL: &str = "block";

/// Bytes one element of the misbehaving modes carries when `config.toml` does
/// not name a `size`.
const DEFAULT_PAYLOAD: usize = 1024 * 1024;

/// Elements the misbehaving modes build when `config.toml` does not name a
/// `count`.
const DEFAULT_COUNT: usize = 1024;

/// Biome names one nested rule declaration carries. The contract bounds the
/// declarations a plan may list, not the names inside one.
const NESTED_NAMES: usize = 64;

/// The identity the `configure-isolation` probe's one line is addressed to.
///
/// The fixture names it because `init` runs with no event of its own: the probe
/// line is that mode's whole answer, and it has to go somewhere a deployment can
/// deliver it to.
const CONFIGURATION_PROBE_PLAYER: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// The prefix of the probe's line, so a test reads the marker it asked about
/// instead of any line another mode could have produced.
const CONFIGURATION_PROBE_LINE: &str = "configure-state";

/// The request id the storage-batch mode gives its batch. A test reads it back
/// from the answer, so it is a constant of the fixture and not of the server.
const BATCH_REQUEST: &str = "batch-1";

/// The durable operation id that batch commits under. The answer echoes it, and
/// the mode's follow-up probe addresses the id the answer named.
const BATCH_OPERATION: &str = "op-1";

/// The three zones the zone mode names: the box it defines for everyone, the one
/// it defines for a single actor, and the one it removes. All three are the
/// plugin's own ids, so a test reads them back from what the owner was asked.
const ZONE_UNPROTECTED: &str = "market-stall";
const ZONE_PROTECTED: &str = "claim-1";
const ZONE_REMOVED: &str = "claim-2";

/// The dimension all three zones live in.
const ZONE_DIMENSION: &str = "minecraft:overworld";

/// The one actor the protected box admits inside it, as a uuid with its dashes:
/// the server normalizes it, so what a test compares against is the normalized
/// actor rather than this spelling.
const ZONE_ACTOR: &str = "12345678-1234-1234-1234-123456781234";

/// Recurse `depth` times, keeping a buffer alive in every frame.
fn deep(depth: usize) -> u64 {
    if depth == 0 {
        return 0;
    }
    let live = [depth as u64; 8];
    std::hint::black_box(&live);
    live[0].wrapping_add(deep(depth - 1))
}

/// How a plugin names a storage failure to a player. The generated enum carries
/// no `Display`, and a plugin deciding its own player-facing wording is the point
/// of handing it a typed outcome instead of a string.
fn failure_name(failure: &StorageFailure) -> &'static str {
    match failure {
        StorageFailure::Unavailable => "unavailable",
        StorageFailure::DurabilityFailed => "durability-failed",
    }
}

/// One authoritative teleport of the connection an event named.
///
/// The one command the SDK does not wrap yet, so the fixture builds the
/// contract's own record itself: the guest still sends exactly what
/// `commands.wit` declares, and nothing here can drift from it.
#[must_use]
fn teleport_player(request: &str, session: u64, x: f64, y: f64, z: f64) -> Command {
    commands::Command::TeleportPlayer(commands::TeleportPlayer {
        request: request.to_owned(),
        session,
        position: types::Position { x, y, z },
    })
}

/// The same for the reason the server refused a teleport: a plugin that reports
/// it to a player picks the wording itself.
fn teleport_failure_name(failure: &PlayerTeleportFailure) -> &'static str {
    match failure {
        PlayerTeleportFailure::PlayerUnavailable => "player-unavailable",
        PlayerTeleportFailure::TeleportPending => "teleport-pending",
        PlayerTeleportFailure::RuntimeUnavailable => "runtime-unavailable",
    }
}

/// One corner of a zone box.
///
/// The three zone commands are ones the SDK does not wrap yet, so the fixture
/// builds the contract's own records itself, exactly as it does for the teleport
/// above: what a test reads back is what `commands.wit` declares.
#[must_use]
fn corner(x: f64, y: f64, z: f64) -> types::Position {
    types::Position { x, y, z }
}

/// One unprotected zone: a box the owner admits every edit inside.
#[must_use]
fn upsert_zone(
    zone: &str,
    dimension: &str,
    minimum: types::Position,
    maximum: types::Position,
) -> Command {
    commands::Command::UpsertZone(commands::UpsertZone {
        zone: zone.to_owned(),
        dimension: dimension.to_owned(),
        minimum,
        maximum,
    })
}

/// One protected zone: the same box, with the one actor the owner admits in it.
#[must_use]
fn upsert_protected_zone(
    zone: &str,
    dimension: &str,
    allowed_actor_uuid: &str,
    minimum: types::Position,
    maximum: types::Position,
) -> Command {
    commands::Command::UpsertProtectedZone(commands::UpsertProtectedZone {
        zone: zone.to_owned(),
        dimension: dimension.to_owned(),
        allowed_actor_uuid: allowed_actor_uuid.to_owned(),
        minimum,
        maximum,
    })
}

/// One removal of a zone this plugin owns.
#[must_use]
fn remove_zone(zone: &str) -> Command {
    commands::Command::RemoveZone(commands::RemoveZone {
        zone: zone.to_owned(),
    })
}

/// How a plugin names the owner's own answer to a zone command. The contract
/// carries one bit, and a plugin names that bit rather than guessing a reason
/// behind it.
fn zone_outcome_name(outcome: &ZoneCommandOutcome) -> &'static str {
    match outcome {
        ZoneCommandOutcome::Applied => "applied",
        ZoneCommandOutcome::Refused => "refused",
    }
}

/// One atomic batch of `count` storage mutations.
///
/// Like the teleport above, this is a command the SDK does not wrap yet, so the
/// fixture builds the contract's own record itself. The mutations alternate a
/// compare-and-swap with a deletion over zero-padded keys, so the batch the server
/// canonicalizes by key is also the batch as it was built, and a test can assert
/// every mutation instead of a count.
#[must_use]
fn storage_batch(count: usize) -> Command {
    let mutations = (0..count)
        .map(|index| {
            let key = format!("key-{index:03}");
            if index % 2 == 0 {
                commands::StorageMutation::Cas(storage::StorageCasMutation {
                    key,
                    expected_version: None,
                    value: format!("value-{index:03}"),
                })
            } else {
                commands::StorageMutation::Delete(storage::StorageDeleteMutation {
                    key,
                    expected_version: Some(u64::try_from(index).expect("a small index")),
                })
            }
        })
        .collect();
    commands::Command::StorageBatchCas(commands::StorageBatchCas {
        request: BATCH_REQUEST.to_owned(),
        operation_id: BATCH_OPERATION.to_owned(),
        mutations,
    })
}

/// A probe of one durable operation id. The request id is derived from the id
/// being probed, so a test reads back which operation this guest decided to ask
/// about rather than a constant it could have sent on its own.
#[must_use]
fn operation_status(operation_id: &str) -> Command {
    commands::Command::OperationStatus(commands::OperationStatus {
        request: format!("status-{operation_id}"),
        operation_id: operation_id.to_owned(),
    })
}

/// How a plugin names an operation refusal to a player: the contract's own
/// vocabulary, so a test reads the reason the server gave rather than a wording
/// the plugin chose.
fn operation_failure_name(failure: &OperationFailure) -> &'static str {
    match failure {
        OperationFailure::InvalidRequest => "invalid-request",
        OperationFailure::Forbidden => "forbidden",
        OperationFailure::StaleRevision => "stale-revision",
        OperationFailure::NotFound => "not-found",
        OperationFailure::Unloaded => "unloaded",
        OperationFailure::Blocked => "blocked",
        OperationFailure::InsufficientItems => "insufficient-items",
        OperationFailure::Capacity => "capacity",
        OperationFailure::Busy => "busy",
        OperationFailure::RuntimeUnavailable => "runtime-unavailable",
        OperationFailure::OperationConflict => "operation-conflict",
        OperationFailure::CursorExpired => "cursor-expired",
        OperationFailure::Unknown => "unknown",
    }
}

/// How one click of a server-owned menu is named in a line a player reads: the
/// contract's own vocabulary, so a test compares against the click it sent rather
/// than against a wording this guest chose.
fn click_name(click: InventoryClick) -> &'static str {
    match click {
        InventoryClick::Primary => "primary",
        InventoryClick::Secondary => "secondary",
        InventoryClick::ShiftPrimary => "shift-primary",
        InventoryClick::ShiftSecondary => "shift-secondary",
    }
}

#[derive(Default)]
struct Hello {
    greeting: String,
    mode: Mode,
    /// The player a storage answer is reported to.
    reporter: Option<String>,
    /// Bytes one element of the misbehaving modes carries.
    size: usize,
    /// Elements the misbehaving modes build.
    count: usize,
    /// What `configure` wrote down in the `configure-isolation` mode.
    ///
    /// Every other mode leaves this at `None`. It exists to be *lost*: the startup
    /// phase runs in a store the host drops before the runtime store `init` runs
    /// in, so the mode's `init` answers with `absent` for a host that separates the
    /// phases and with the marker for one that shares a store. It is a probe of
    /// the host, not runtime state.
    configure_probe: Option<usize>,
    /// The timer scenario, when `mode` is `timers`. Its own module: the timer
    /// batches are a second state machine, and the root already carries one.
    timers: Option<timers::Timers>,
    /// The storage-compatibility phase, when `mode` is `storage-compat`. Its own
    /// module for the timers' reason: the phase is a second state machine, with
    /// its own steps and its own outcome checks.
    storage_compat: Option<storage_compat::StorageCompat>,
    /// The inventory/storage acceptance fixture, when `mode` is
    /// `inventory-storage`. Its own module for the storage phases' reason: a trade
    /// action is a state machine over two reads and one transaction, and the root
    /// already carries one.
    inventory_storage: Option<inventory_storage::Trade>,
    /// The zone-market fixture, when `mode` is `zone-market`. It owns the
    /// inventory/storage fixture for its reason: the zone path decides when to ask
    /// for the market, and every trade action stays the state machine that module
    /// already is.
    zone_market: Option<zone_market::ZoneMarket>,
    settlement_ops: Option<settlement_ops::Fixture>,
    resident_ops: Option<resident_ops::Fixture>,
    inventory_ops: Option<inventory_ops::Fixture>,
    audit_events: Option<audit_events::Audit>,
    loader_live: Option<loader_live::Fixture>,
    /// The pre-commit fixture, when `mode` is `precommit`. Its own module: the two
    /// hooks answer decisions rather than batches, and the witness that reports
    /// what they saw is a second state machine beside the root's one.
    precommit: Option<precommit::Hooks>,
}

#[derive(Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Normal,
    Spin,
    Grow,
    /// Answers every event with a plugin error: a legitimate answer that must not
    /// retire the instance.
    Refuse,
    /// Logs the sentinel `block` a fixed number of times. A test-side host service
    /// slows that import down, which is the one place a guest spends no fuel and
    /// only the host's epoch deadline can end it.
    Block,
    /// Answers a burst of messages, to drive the host's command-queue bound.
    Burst,
    /// Exercises the durable-storage round trip: on a join it reads its own key,
    /// then reports what storage answered.
    Storage,
    /// Exercises the online-players query: on a join it asks who is connected,
    /// then reports the snapshot it was given.
    Players,
    /// Exercises the player teleport: on a join it asks the server to move that
    /// exact connection, then reports the typed answer it was given.
    Teleport,
    /// Exercises the durable storage batch: on a join it asks the server to apply
    /// `count` mutations under one durable operation id, reports the typed answer,
    /// and probes that same operation id with `operation_status`.
    StorageBatch,
    /// Exercises the three zone commands: on a join it defines one unprotected box,
    /// defines one protected box and removes a third zone, then reports what the
    /// owner answered about each of the three, by the zone it named.
    Zones,
    /// Schedules, replaces and cancels timers, driven only by the ticks the
    /// server pushes: the `timers` module owns the scenario `script` selects and
    /// reports every fire a player can read.
    Timers,
    /// Runs the one storage-compatibility phase `storage_phase` names: it reads
    /// the records a legacy package left, replays and migrates their durable
    /// operations, and reads what this owner, a reopened world and a foreign
    /// owner are each allowed to see. Driven from `init`, with no player and no
    /// subscription in it.
    StorageCompat,
    /// Runs the inventory/storage acceptance fixture: a player's `trade` command
    /// answers one atomic transaction of that player's inventory and this
    /// package's own storage, and reports what the owners did. One action is in
    /// flight at a time, and every action reuses the same correlation id because
    /// this command records no durable operation to address.
    InventoryStorage,
    /// Subscribes to `inventory.menu.clicked` and owns no menu: it reports any
    /// click that reaches it, which is what a test reads if a click is delivered
    /// to more than the plugin that opened the menu, and answers the `menu-fence`
    /// command that proves the package is live while it stays silent.
    MenuObserver,
    /// Runs the zone-market acceptance fixture: the fixed `trade-zone` box is
    /// registered through the existing upsert command, the owner's own applied
    /// answer publishes readiness, and the owner's boundary transitions open and
    /// close the market of the inventory-storage fixture.
    ZoneMarket,
    /// Subscribes to `player.zone_entered`/`player.zone_exited` and owns no zone:
    /// it reports any transition that reaches it, which is what a test reads if a
    /// transition is delivered to more than the plugin that owns the zone, and
    /// answers the `zone-fence` command that proves it is live while it stays
    /// silent.
    ZoneObserver,
    /// A mixed messaging batch, with `size` controlling the disconnect reason.
    Messaging,
    /// Answers one command carrying `size` bytes of text: the largest single
    /// string a guest can ask the host to copy out of its memory.
    Oversized,
    /// Answers `count` commands, to drive the per-callback element count.
    Wide,
    /// Answers a startup contribution of `count` declarations, each naming
    /// `NESTED_NAMES` biomes of `size` bytes: many small strings inside two levels
    /// of lists.
    Nested,
    /// Builds a batch and then faults, so a test can pin that a callback which
    /// never returns publishes nothing.
    Trap,
    /// Recurses past any stack a guest could want: the host's stack bound has to
    /// end the call instead of the process.
    Recurse,
    /// Answers a startup contribution the host accepts: one placement category
    /// inside every documented bound and no names to repeat. The nested mode above
    /// is a claim rather than a contribution - its biome names are all the same,
    /// so the validator refuses it as a duplicate - which is why a test that needs
    /// a *valid* contribution asks for this mode.
    Placement,
    /// Writes a state marker from `configure` and answers with whether `init`
    /// found it: the probe of the two stores the host runs the phases in. A
    /// deployment that shares one store answers with the marker.
    ConfigureIsolation,
    /// Answers a plugin error from `configure`: a package whose own startup phase
    /// fails, which has to refuse the deployment before a runtime store exists.
    RefuseConfigure,
    SettlementOperations,
    ResidentOperations,
    OwnedInventory,
    AuditEvents,
    LoaderLive,
    /// Runs the pre-commit fixture in `precommit.rs`: one real guest whose two
    /// hooks answer the `build`/`damage` keys' decisions, record what they were
    /// asked, and are driven and reported at runtime by the command root the
    /// `root` key names. The hooks themselves answer a decision and nothing else:
    /// `status`, `reset`, `build`, `damage` and `fault` are ordinary commands,
    /// and a chain test gives two deployments of this fixture two roots.
    Precommit,
}

impl Mode {
    /// The mode one configuration names, or [`Mode::Normal`] for an absent or
    /// unknown name.
    fn named(name: Option<&str>) -> Self {
        match name {
            Some("spin") => Mode::Spin,
            Some("grow") => Mode::Grow,
            Some("refuse") => Mode::Refuse,
            Some("block") => Mode::Block,
            Some("burst") => Mode::Burst,
            Some("storage") => Mode::Storage,
            Some("players") => Mode::Players,
            Some("teleport") => Mode::Teleport,
            Some("storage-batch") => Mode::StorageBatch,
            Some("storage-compat") => Mode::StorageCompat,
            Some("inventory-storage") => Mode::InventoryStorage,
            Some("menu-observer") => Mode::MenuObserver,
            Some("zone-market") => Mode::ZoneMarket,
            Some("zone-observer") => Mode::ZoneObserver,
            Some("zones") => Mode::Zones,
            Some("timers") => Mode::Timers,
            Some("messaging") => Mode::Messaging,
            Some("oversized") => Mode::Oversized,
            Some("wide") => Mode::Wide,
            Some("nested") => Mode::Nested,
            Some("trap") => Mode::Trap,
            Some("recurse") => Mode::Recurse,
            Some("placement") => Mode::Placement,
            Some("configure-isolation") => Mode::ConfigureIsolation,
            Some("refuse-configure") => Mode::RefuseConfigure,
            Some("settlement-operations") => Mode::SettlementOperations,
            Some("resident-operations") => Mode::ResidentOperations,
            Some("owned-inventory") => Mode::OwnedInventory,
            Some("audit-events") => Mode::AuditEvents,
            Some("loader-live") => Mode::LoaderLive,
            Some("precommit") => Mode::Precommit,
            _ => Mode::Normal,
        }
    }
}

/// The configuration fields the two startup phases each read.
///
/// Both phases parse the package's own text, and neither hands the result to the
/// other: they run in different stores, so state left behind by `configure` is
/// gone from `init`. `configure` reads this to answer the contribution, `init`
/// reads it to build the runtime state.
struct Settings {
    greeting: String,
    mode: Mode,
    size: usize,
    count: usize,
    /// The timer scenario `script` names, or the fixture's own probe.
    script: String,
    /// The storage-compatibility phase `storage_phase` names, or the empty name
    /// the fixture refuses.
    storage_phase: String,
}

/// Parse one package configuration the way both startup phases read it.
fn settings_of(config: &Config) -> Settings {
    let Some(value) = config.toml() else {
        return Settings {
            greeting: "Welcome!".to_owned(),
            mode: Mode::Normal,
            size: DEFAULT_PAYLOAD,
            count: DEFAULT_COUNT,
            script: "probe".to_owned(),
            storage_phase: String::new(),
        };
    };
    Settings {
        greeting: value
            .get("greeting")
            .and_then(|text| text.as_str())
            .unwrap_or("Welcome!")
            .to_owned(),
        mode: Mode::named(value.get("mode").and_then(|mode| mode.as_str())),
        // `size`/`count` are the misbehaving modes' payload: a test states how
        // much the guest answers with, and a negative or unreadable value falls
        // back to the fixture's own default rather than failing the package.
        size: value
            .get("size")
            .and_then(|size| size.as_integer())
            .and_then(|size| usize::try_from(size).ok())
            .unwrap_or(DEFAULT_PAYLOAD),
        count: value
            .get("count")
            .and_then(|count| count.as_integer())
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(DEFAULT_COUNT),
        script: value
            .get("script")
            .and_then(|script| script.as_str())
            .unwrap_or("probe")
            .to_owned(),
        storage_phase: value
            .get("storage_phase")
            .and_then(|phase| phase.as_str())
            .unwrap_or("")
            .to_owned(),
    }
}

impl Hello {
    /// Read one package configuration into this instance's runtime state.
    ///
    /// `configure` answers the startup contribution from the same configuration
    /// and keeps none of it: it runs in a store the host drops before the runtime
    /// store exists, so the state every later callback runs on is built here.
    fn initialize(&mut self, config: &Config, context: &InitContext) -> Result<(), Failure> {
        let settings = settings_of(config);
        self.greeting = settings.greeting;
        self.mode = settings.mode;
        self.size = settings.size;
        self.count = settings.count;
        match self.mode {
            Mode::SettlementOperations => {
                self.settlement_ops = Some(settlement_ops::Fixture::configure(config)?);
            }
            Mode::ResidentOperations => {
                self.resident_ops = Some(resident_ops::Fixture::configure(config)?);
            }
            Mode::OwnedInventory => {
                self.inventory_ops = Some(inventory_ops::Fixture::configure(config)?);
            }
            Mode::AuditEvents => {
                self.audit_events = Some(audit_events::Audit::configure(config)?);
            }
            Mode::LoaderLive => {
                self.loader_live = Some(loader_live::Fixture::new(&context.plugin_id)?);
            }
            // The fixture's own keys are its whole configuration, and a key it
            // does not implement refuses the package rather than deciding for the
            // operator. It also records the command root and plugin id it answers
            // with.
            Mode::Precommit => {
                self.precommit = Some(precommit::Hooks::configure(config, context)?);
            }
            _ => {}
        }
        if self.mode == Mode::Timers {
            // `script` is the timer fixture's own vocabulary, so a test states the
            // one scenario it drives; an unknown name is the probe, which only
            // schedules and reports.
            self.timers = Some(timers::Timers::new(&settings.script, self.count));
        }
        if self.mode == Mode::StorageCompat {
            // The compatibility phases answer their own phase by name: an unknown
            // phase refuses init rather than operating on the wrong stored data.
            self.storage_compat = Some(storage_compat::StorageCompat::new(&settings.storage_phase));
        }
        if self.mode == Mode::InventoryStorage {
            // The fixture has no configuration of its own; `trade` selects an action.
            self.inventory_storage = Some(inventory_storage::Trade::new());
        }
        if self.mode == Mode::ZoneMarket {
            // The zone fixture shares the inventory/storage trade state machine.
            self.zone_market = Some(zone_market::ZoneMarket::new());
        }
        Ok(())
    }
}

/// The placement contribution a test asks for when it needs one the host accepts:
/// one category inside every documented bound and no names to repeat.
///
/// `placement` answers it and nothing else; `configure-isolation` answers it as
/// well, so that a test can tell "the startup phase ran" apart from "what the
/// startup phase wrote down survived".
fn placement_contribution() -> StartupContribution {
    StartupContribution {
        placement: Some(lifecycle::PlacementRules {
            land_spacing: 2,
            water_attempts: 8,
            water_depth: 4,
        }),
        trees: None,
        clay: None,
        spawning: None,
        items: None,
    }
}

impl Plugin for Hello {
    fn configure(&mut self, config: &Config) -> Result<Option<StartupContribution>, Failure> {
        // The startup phase answers the contribution and nothing else, because the
        // host drops this store before the runtime store exists: state left here
        // would be gone by `init`, so `init` reads the same configuration again
        // instead of relying on it. Nothing below touches the runtime state.
        let settings = settings_of(config);
        match settings.mode {
            Mode::Placement => Ok(Some(placement_contribution())),
            Mode::LoaderLive => {
                let item = config.toml().and_then(|config| {
                    match config.get("item_owner").and_then(|owner| owner.as_str()) {
                        Some("ruby-live") => Some(("ruby-live:ruby", "Ruby Fixture Item")),
                        Some("sapphire-live") => {
                            Some(("sapphire-live:sapphire", "Sapphire Fixture Item"))
                        }
                        _ => None,
                    }
                });
                Ok(item.map(|(id, name)| {
                    let mut items = vec![lifecycle::ItemDefinition {
                        id: id.to_owned(),
                        carrier: "minecraft:paper".to_owned(),
                        name: name.to_owned(),
                        max_stack_size: 16,
                        max_damage: None,
                        weapon: false,
                        attack_damage_modifier: None,
                        attack_speed_modifier: None,
                        equippable_slot: None,
                        crafting_ingredient: Some("minecraft:paper".to_owned()),
                    }];
                    if id == "ruby-live:ruby" {
                        items.push(lifecycle::ItemDefinition {
                            id: "ruby-live:blade".to_owned(),
                            carrier: "minecraft:paper".to_owned(),
                            name: "Ruby Blade".to_owned(),
                            max_stack_size: 1,
                            max_damage: Some(3),
                            weapon: true,
                            attack_damage_modifier: Some(5.0),
                            attack_speed_modifier: Some(-2.0),
                            equippable_slot: None,
                            crafting_ingredient: Some(id.to_owned()),
                        });
                    }
                    StartupContribution {
                        placement: None,
                        trees: None,
                        clay: None,
                        spawning: None,
                        items: Some(items),
                    }
                }))
            }
            // A nested contribution is the startup answer, so it is built here
            // rather than from an event.
            Mode::Nested => Ok(Some(StartupContribution {
                placement: None,
                trees: Some(
                    (0..settings.count)
                        .map(|_| lifecycle::TreeRules {
                            biomes: vec!["z".repeat(settings.size); NESTED_NAMES],
                            spacing: 1,
                            density_threshold: 0.0,
                        })
                        .collect(),
                ),
                clay: None,
                spawning: None,
                items: None,
            })),
            // The one probe that the two phases do not share a store: this mode
            // answers a valid contribution like `placement` and writes a marker
            // down, so a deployment reports that the phase ran *and* reports in
            // `init` whether what it wrote survived. Every other mode leaves the
            // marker at its default.
            Mode::ConfigureIsolation => {
                self.configure_probe = Some(settings.count);
                Ok(Some(placement_contribution()))
            }
            // A package whose own startup phase fails, which the deployment reads
            // as a refusal and never gets as far as a runtime store.
            Mode::RefuseConfigure => Err(Failure::Invalid),
            _ => Ok(None),
        }
    }

    fn init(&mut self, config: &Config, context: &InitContext) -> Result<Vec<Command>, Failure> {
        // The runtime state comes from the same configuration the startup phase
        // read, built here because that phase's store is already gone.
        self.initialize(config, context)?;
        match self.mode {
            Mode::Normal
            | Mode::Refuse
            | Mode::Burst
            | Mode::Storage
            | Mode::Players
            | Mode::Teleport
            | Mode::StorageBatch
            | Mode::StorageCompat
            | Mode::InventoryStorage
            | Mode::MenuObserver
            | Mode::ZoneMarket
            | Mode::ZoneObserver
            | Mode::Zones
            | Mode::Timers
            | Mode::Messaging
            | Mode::Oversized
            | Mode::Wide
            | Mode::Nested
            | Mode::SettlementOperations
            | Mode::ResidentOperations
            | Mode::OwnedInventory
            | Mode::AuditEvents
            | Mode::LoaderLive
            | Mode::Precommit
            | Mode::ConfigureIsolation
            | Mode::RefuseConfigure
            | Mode::Placement => log(LogLevel::Info, "hello plugin ready"),
            // The misbehaving modes that fault do so from a callback, so the
            // deployment itself always starts.
            Mode::Trap | Mode::Recurse => {}
            // Nothing slow happens during startup: the slow import is only reached
            // from a callback, so the deployment itself always starts.
            Mode::Block => {}
            // A guest that never returns must be stopped by the host's budget.
            Mode::Spin => loop {
                let _ = std::hint::black_box(1_u64).wrapping_mul(3);
            },
            // A guest that allocates without bound must be stopped by the store's
            // memory limit, not by the machine running out.
            Mode::Grow => {
                let mut held: Vec<Vec<u8>> = Vec::new();
                loop {
                    held.push(vec![0_u8; 1024 * 1024]);
                }
            }
        }
        log(LogLevel::Debug, &context.world_fingerprint);
        if self.mode == Mode::ConfigureIsolation {
            // The probe's one line, and this mode's whole answer: what `configure`
            // wrote in the store the host dropped. A host that ran both phases in
            // one store answers with the marker instead of `absent`.
            let marker = self
                .configure_probe
                .map_or_else(|| "absent".to_owned(), |value| value.to_string());
            return Ok(vec![message_player(
                CONFIGURATION_PROBE_PLAYER,
                format!("{CONFIGURATION_PROBE_LINE} {marker}"),
            )]);
        }
        if let Some(fixture) = &mut self.settlement_ops {
            return fixture.init();
        }
        if let Some(fixture) = &mut self.resident_ops {
            return fixture.init();
        }
        if let Some(fixture) = &mut self.inventory_ops {
            return fixture.init();
        }
        if let Some(fixture) = &mut self.audit_events {
            return fixture.init();
        }
        if let Some(compat) = &mut self.storage_compat {
            // The compatibility phases answer their own requests: the first read
            // is issued here, so a phase needs no player to start it.
            return compat.init();
        }
        match &mut self.timers {
            // The timer scenarios that schedule from startup answer the batch
            // here: `init` runs with no tick pushed yet, so a fixture that
            // measures a deadline schedules from a callback instead.
            Some(timers) => Ok(timers.init()),
            None => Ok(Vec::new()),
        }
    }

    fn on_events(
        &mut self,
        _context: &EventContext,
        events: &[Event],
    ) -> Result<Vec<Command>, Failure> {
        if let Some(fixture) = &mut self.settlement_ops {
            return fixture.on_events(events);
        }
        if let Some(fixture) = &mut self.resident_ops {
            return fixture.on_events(events);
        }
        if let Some(fixture) = &mut self.inventory_ops {
            return fixture.on_events(events);
        }
        if let Some(fixture) = &mut self.audit_events {
            return fixture.on_events(_context, events);
        }
        if let Some(fixture) = &mut self.loader_live {
            return Ok(fixture.on_events(_context, events));
        }
        if let Some(hooks) = &mut self.precommit {
            // The fixture answers its own witness command. A hook question never
            // arrives here: it is a call, not an event, and its answer is a
            // decision rather than a batch.
            return hooks.on_events(events);
        }
        if let Some(timers) = &mut self.timers {
            // The timer scenarios answer their own events, fires included: a
            // fire arrives as its own delivered batch, and everything it answers
            // is staged by the host like any other callback's batch.
            return Ok(timers.on_events(events));
        }
        if let Some(compat) = &mut self.storage_compat {
            // The compatibility phases answer only their own requests, so every
            // event here is either the answer they wait for or nothing they act
            // on.
            return compat.on_events(events);
        }
        if let Some(zone) = &mut self.zone_market {
            // The zone fixture answers the owner's own transition events and hands
            // every other event to the inventory/storage fixture it owns. The join
            // greeting stays here for the trade mode's reason: it is the one line a
            // driver reads as readiness before it does anything.
            let mut commands = Vec::new();
            for event in events {
                if let Event::PlayerJoined(joined) = event {
                    commands.push(message_player(
                        &joined.player,
                        format!("{} {}", self.greeting, joined.name),
                    ));
                }
            }
            commands.extend(zone.on_events(events)?);
            return Ok(commands);
        }
        if let Some(trade) = &mut self.inventory_storage {
            // The trade fixture answers its own requests and dispatches the trade
            // commands. The join greeting stays here because it is the one line a
            // driver reads as readiness before it sends the first action; nothing
            // else in this mode answers an event.
            let mut commands = Vec::new();
            for event in events {
                if let Event::PlayerJoined(joined) = event {
                    commands.push(message_player(
                        &joined.player,
                        format!("{} {}", self.greeting, joined.name),
                    ));
                }
            }
            commands.extend(trade.on_events(events, &[])?);
            return Ok(commands);
        }
        if self.mode == Mode::Refuse {
            return Err(Failure::NotFound);
        }
        if self.mode == Mode::Block {
            for _ in 0..BLOCKING_LOGS {
                log(LogLevel::Info, BLOCK_SENTINEL);
            }
            return Ok(Vec::new());
        }
        if self.mode == Mode::Recurse {
            // One frame per call with a live buffer in each, so neither the
            // optimiser nor a tail call can flatten this into a loop.
            deep(self.count);
            return Ok(Vec::new());
        }
        if self.mode == Mode::Oversized || self.mode == Mode::Wide || self.mode == Mode::Trap {
            // One answer, three misbehaviours: a single string far past what a
            // callback may hand the host, a list far longer than the batch bound,
            // and a batch that is built and then never returned.
            let player = events.iter().find_map(|event| match event {
                Event::PlayerJoined(joined) => Some(joined.player.clone()),
                _ => None,
            });
            let Some(player) = player else {
                return Ok(Vec::new());
            };
            let mut commands = Vec::new();
            match self.mode {
                Mode::Oversized => {
                    commands.push(message_player(&player, "x".repeat(self.size)));
                }
                Mode::Wide | Mode::Trap => {
                    for _ in 0..self.count {
                        commands.push(message_player(&player, "y".repeat(self.size.max(1))));
                    }
                }
                _ => {}
            }
            if self.mode == Mode::Trap {
                // The batch above is already built when the guest faults, which is
                // what makes it an unpublished batch rather than a missing one.
                std::hint::black_box(&commands);
                core::arch::wasm32::unreachable();
            }
            return Ok(commands);
        }
        let mut commands = Vec::new();
        for event in events {
            match event {
                Event::PlayerJoined(joined) => {
                    // A storage read has to name the player it will report to
                    // later: the answer arrives as its own event, which carries no
                    // player of its own.
                    self.reporter = Some(joined.player.clone());
                    if self.mode == Mode::Storage {
                        // The storage mode answers only what storage answered, so
                        // the test reads one thing per phase.
                        commands.push(storage_get("coins", "coins:player"));
                        commands.push(storage_cas("bump", "coins:player", None, "3"));
                        continue;
                    }
                    if self.mode == Mode::Players {
                        commands.push(list_online_players("who", 8));
                        continue;
                    }
                    if self.mode == Mode::Teleport {
                        // The request names the session the join reported, not the
                        // stable identity: a teleport is an effect on one live
                        // connection. The coordinates are the plugin's own, so a
                        // test reads back exactly what it sent.
                        commands.push(teleport_player(
                            "warp-home",
                            joined.session,
                            12.5,
                            70.0,
                            -4.5,
                        ));
                        continue;
                    }
                    if self.mode == Mode::StorageBatch {
                        // `count` is the plugin's own input here rather than a
                        // misbehaving payload: how many mutations fit in one batch
                        // is the server's bound, and a test states the number it
                        // means to be inside or outside it.
                        commands.push(storage_batch(self.count));
                        continue;
                    }
                    if self.mode == Mode::Zones {
                        // All three zone commands in one callback, each naming its
                        // own box: what a test reads back is the owner's answer
                        // about each of the three zones, in the contract's own
                        // records. The coordinates and the actor uuid are the
                        // plugin's own, and the smallest protected box is the
                        // second one - a zone whose upper corner is past its lower
                        // one is refused by the server, so this fixture only ever
                        // sends boxes it means.
                        commands.push(upsert_zone(
                            ZONE_UNPROTECTED,
                            ZONE_DIMENSION,
                            corner(10.0, 60.0, -20.0),
                            corner(20.0, 70.0, -10.0),
                        ));
                        commands.push(upsert_protected_zone(
                            ZONE_PROTECTED,
                            ZONE_DIMENSION,
                            ZONE_ACTOR,
                            corner(-48.0, 0.0, -48.0),
                            corner(48.0, 255.0, 48.0),
                        ));
                        commands.push(remove_zone(ZONE_REMOVED));
                        continue;
                    }
                    if self.mode == Mode::Messaging {
                        commands.push(message_player(&joined.player, "direct"));
                        commands.push(broadcast("broadcast"));
                        commands.push(disconnect_player(joined.session, "x".repeat(self.size)));
                        continue;
                    }
                    let burst = if self.mode == Mode::Burst { BURST } else { 1 };
                    for index in 0..burst {
                        let text = if index == 0 {
                            format!("{} {}", self.greeting, joined.name)
                        } else {
                            format!("{} {} {index}", self.greeting, joined.name)
                        };
                        commands.push(message_player(&joined.player, text));
                    }
                }
                Event::CommandInvoked(invoked) if invoked.name == "hello" => {
                    commands.push(message_player(&invoked.player, "Hello from a WASM plugin."));
                }
                // The observer package's one command: it answers a line a test
                // reads to know the package is live and that its own messages
                // arrive, which is what makes its silence about another package's
                // menu click a result rather than a missing deployment.
                Event::CommandInvoked(invoked)
                    if self.mode == Mode::MenuObserver && invoked.name == MENU_FENCE =>
                {
                    commands.push(message_player(&invoked.player, "P3_MENU_FENCE"));
                }
                // One click of a server-owned menu this instance was told about.
                // Only the plugin that opened a menu is told about its clicks, so
                // the observer package has nothing to answer here: the line below
                // is exactly what a test would read if a click reached a
                // subscriber that owns no menu, and the line names the menu, the
                // slot and the click the server reported.
                Event::InventoryMenuClicked(clicked) if self.mode == Mode::MenuObserver => {
                    commands.push(message_player(
                        &clicked.player,
                        format!(
                            "P3_MENU_LEAK {} {} {}",
                            clicked.menu,
                            clicked.slot,
                            click_name(clicked.click)
                        ),
                    ));
                }
                // The zone observer's one command, its line of the same kind as the
                // menu fence: the package answers it while a zone transition it
                // subscribes to stays unanswered.
                Event::CommandInvoked(invoked)
                    if self.mode == Mode::ZoneObserver && invoked.name == ZONE_FENCE =>
                {
                    commands.push(message_player(&invoked.player, ZONE_FENCE_LINE));
                }
                // One boundary transition that reached a package owning no zone.
                // The zone owner addresses a transition to the plugin that
                // registered the box, so a subscriber without that box has nothing
                // to answer; the line below is exactly what a test reads if a
                // transition is delivered to somebody else's subscriber.
                Event::PlayerZoneEntered(transition) if self.mode == Mode::ZoneObserver => {
                    commands.push(message_player(
                        &transition.player,
                        format!("{ZONE_LEAK_PREFIX} entered {}", transition.zone),
                    ));
                }
                Event::PlayerZoneExited(transition) if self.mode == Mode::ZoneObserver => {
                    commands.push(message_player(
                        &transition.player,
                        format!("{ZONE_LEAK_PREFIX} exited {}", transition.zone),
                    ));
                }
                // The answer to the read this plugin issued on join: the text a
                // player sees carries exactly what storage reported.
                Event::StorageGetAnswered(answered) => {
                    let text = match &answered.outcome {
                        StorageGetOutcome::Read(record) => match (&record.value, record.version) {
                            (Some(value), Some(version)) => {
                                format!("coins {value} at {version}")
                            }
                            _ => "coins absent".to_owned(),
                        },
                        StorageGetOutcome::Failed(failure) => {
                            format!("storage {}", failure_name(failure))
                        }
                    };
                    if let Some(player) = self.reporter.as_deref() {
                        commands.push(message_player(player, format!("{} {text}", self.greeting)));
                    }
                }
                // The answer to the query this plugin issued on join: one line
                // per connected player the server reported, plus the truncation.
                Event::OnlinePlayersAnswered(answered) => {
                    let text = if answered.players.is_empty() {
                        "nobody online".to_owned()
                    } else {
                        answered
                            .players
                            .iter()
                            .map(|player| format!("{}@{}", player.name, player.dimension))
                            .collect::<Vec<_>>()
                            .join(",")
                    };
                    let suffix = if answered.truncated { " more" } else { "" };
                    if let Some(player) = self.reporter.as_deref() {
                        commands.push(message_player(
                            player,
                            format!("{} {text}{suffix}", self.greeting),
                        ));
                    }
                }
                Event::StorageCasAnswered(answered) => {
                    let text = match &answered.outcome {
                        StorageCasOutcome::Committed(version) => format!("cas at {version}"),
                        StorageCasOutcome::Refused => "cas refused".to_owned(),
                        StorageCasOutcome::Failed(failure) => {
                            format!("cas {}", failure_name(failure))
                        }
                    };
                    if let Some(player) = self.reporter.as_deref() {
                        commands.push(message_player(player, format!("{} {text}", self.greeting)));
                    }
                }
                // The answer to the teleport this plugin issued on join. The line
                // names the plugin's own request id and the exact coordinates, so
                // a test reads back what the contract carried instead of taking
                // delivery order on trust.
                Event::PlayerTeleportAnswered(answered) => {
                    let outcome = match &answered.outcome {
                        PlayerTeleportOutcome::Committed => "committed".to_owned(),
                        PlayerTeleportOutcome::Refused(failure) => {
                            format!("refused {}", teleport_failure_name(failure))
                        }
                    };
                    let position = &answered.position;
                    if let Some(player) = self.reporter.as_deref() {
                        commands.push(message_player(
                            player,
                            format!(
                                "{} {} {} {outcome} at {}/{}/{}",
                                self.greeting,
                                answered.request,
                                answered.session,
                                position.x,
                                position.y,
                                position.z,
                            ),
                        ));
                    }
                }
                // The answer to one zone command this plugin issued. The owner names
                // the zone and whether it took the command, so the line carries both:
                // a test reads back the zone the answer was about and the owner's own
                // verdict, not an order it assumed.
                Event::ZoneCommandAnswered(answered) => {
                    if let Some(player) = self.reporter.as_deref() {
                        commands.push(message_player(
                            player,
                            format!(
                                "{} {} {}",
                                self.greeting,
                                answered.zone,
                                zone_outcome_name(&answered.outcome)
                            ),
                        ));
                    }
                }
                // The answer to the operation this plugin issued. The line names
                // both ids and the outcome, so a test reads back what the contract
                // carried instead of taking delivery order on trust. A committed
                // batch is then probed by the durable id the *answer* named.
                Event::OperationAnswered(answered) => {
                    let operation_id = answered
                        .operation_id
                        .as_deref()
                        .ok_or(Failure::Unexpected)?;
                    let outcome = match &answered.outcome {
                        OperationOutcome::Committed(committed) => {
                            let changes = match &committed.payload {
                                OperationPayload::StorageBatch(changes) => changes
                                    .iter()
                                    .map(|change| {
                                        format!(
                                            "{}{}",
                                            change.key,
                                            if change.deleted {
                                                ":deleted"
                                            } else {
                                                ":written"
                                            }
                                        )
                                    })
                                    .collect::<Vec<_>>()
                                    .join(","),
                                _ => return Err(Failure::Unexpected),
                            };
                            format!("committed at {} {changes}", committed.revision)
                        }
                        OperationOutcome::Refused(failure) => {
                            format!("refused {}", operation_failure_name(&failure.reason))
                        }
                    };
                    if let Some(player) = self.reporter.as_deref() {
                        commands.push(message_player(
                            player,
                            format!(
                                "{} {} {} {outcome}",
                                self.greeting, answered.request, operation_id
                            ),
                        ));
                    }
                    // Only a commit recorded a durable operation to probe: a
                    // refusal stored nothing under the id, so the mode's answer is
                    // what a test reads next.
                    if self.mode == Mode::StorageBatch
                        && answered.request == BATCH_REQUEST
                        && matches!(answered.outcome, OperationOutcome::Committed(_))
                    {
                        commands.push(operation_status(operation_id));
                    }
                }
                _ => {}
            }
        }
        Ok(commands)
    }

    /// Every mode but `precommit` keeps every build, and says so by answering the
    /// trait's own default: this example registers no hook anywhere, and the host
    /// asks a hook only for a registration an operator made.
    fn before_build(&mut self, context: &BuildContext) -> Result<BuildDecision, Failure> {
        match &mut self.precommit {
            Some(hooks) => hooks.before_build(context),
            None => Ok(BuildDecision::Keep),
        }
    }

    /// The damage half of the same default: `precommit` answers what its `damage`
    /// key says, and every other mode keeps the amount it was asked about.
    fn before_damage(&mut self, context: &DamageContext) -> Result<DamageDecision, Failure> {
        match &mut self.precommit {
            Some(hooks) => hooks.before_damage(context),
            None => Ok(DamageDecision::Keep),
        }
    }

    fn shutdown(&mut self) -> Result<(), Failure> {
        log(LogLevel::Info, "hello plugin stopping");
        Ok(())
    }
}

export_plugin!(Hello);
