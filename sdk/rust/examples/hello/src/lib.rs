//! Example Solaris plugin.
//!
//! It is server-only and does two things a player can see: it greets a player who
//! joins, and it answers the `/hello` command. Both are staged commands the host
//! admits and applies; the plugin never touches the world itself.
//!
//! The configuration is optional. With `greeting = "..."` in `config.toml` the
//! plugin uses that text; the fixture also reads `mode` to exercise the host's
//! budgets from the guest side (`spin` never returns, `grow` allocates without
//! bound, `oversized`/`wide`/`nested` answer more than a callback may hand the
//! host, `trap` builds a batch and then faults), which is how the host's limit
//! tests drive a misbehaving guest with a real component instead of a hand-built
//! module.

use solaris_plugin_sdk::events::{
    OperationFailure, OperationOutcome, OperationPayload, PlayerTeleportFailure,
    PlayerTeleportOutcome, ZoneCommandOutcome,
};
use solaris_plugin_sdk::{
    Command, Config, Event, EventContext, Failure, InitContext, LogLevel, Plugin, RulePlan,
    StorageCasOutcome, StorageFailure, StorageGetOutcome, commands, export_plugin, lifecycle,
    list_online_players, log, message_player, storage, storage_cas, storage_get, types,
};

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
    /// Answers one command carrying `size` bytes of text: the largest single
    /// string a guest can ask the host to copy out of its memory.
    Oversized,
    /// Answers `count` commands, to drive the per-callback element count.
    Wide,
    /// Answers a rule plan of `count` declarations, each naming `NESTED_NAMES`
    /// biomes of `size` bytes: many small strings inside two levels of lists.
    Nested,
    /// Builds a batch and then faults, so a test can pin that a callback which
    /// never returns publishes nothing.
    Trap,
    /// Recurses past any stack a guest could want: the host's stack bound has to
    /// end the call instead of the process.
    Recurse,
    /// Answers a rule plan the host accepts: one placement category inside every
    /// documented bound and no names to repeat. The nested mode above is a claim
    /// rather than a plan - its biome names are all the same, so the validator
    /// refuses it as a duplicate - which is why a test that needs a *valid* plan
    /// asks for this mode.
    Placement,
}

impl Plugin for Hello {
    fn configure(&mut self, config: &Config) -> Result<Option<RulePlan>, Failure> {
        let Some(value) = config.toml() else {
            self.greeting = "Welcome!".to_owned();
            return Ok(None);
        };
        self.greeting = value
            .get("greeting")
            .and_then(|text| text.as_str())
            .unwrap_or("Welcome!")
            .to_owned();
        self.mode = match value.get("mode").and_then(|mode| mode.as_str()) {
            Some("spin") => Mode::Spin,
            Some("grow") => Mode::Grow,
            Some("refuse") => Mode::Refuse,
            Some("block") => Mode::Block,
            Some("burst") => Mode::Burst,
            Some("storage") => Mode::Storage,
            Some("players") => Mode::Players,
            Some("teleport") => Mode::Teleport,
            Some("storage-batch") => Mode::StorageBatch,
            Some("zones") => Mode::Zones,
            Some("oversized") => Mode::Oversized,
            Some("wide") => Mode::Wide,
            Some("nested") => Mode::Nested,
            Some("trap") => Mode::Trap,
            Some("recurse") => Mode::Recurse,
            Some("placement") => Mode::Placement,
            _ => Mode::Normal,
        };
        // `size`/`count` are the misbehaving modes' payload: a test states how
        // much the guest answers with, and a negative or unreadable value falls
        // back to the fixture's own default rather than failing the package.
        self.size = value
            .get("size")
            .and_then(|size| size.as_integer())
            .and_then(|size| usize::try_from(size).ok())
            .unwrap_or(DEFAULT_PAYLOAD);
        self.count = value
            .get("count")
            .and_then(|count| count.as_integer())
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(DEFAULT_COUNT);
        if self.mode == Mode::Placement {
            return Ok(Some(RulePlan {
                placement: Some(lifecycle::PlacementRules {
                    land_spacing: 2,
                    water_attempts: 8,
                    water_depth: 4,
                }),
                trees: None,
                clay: None,
                spawning: None,
            }));
        }
        if self.mode == Mode::Nested {
            // A nested plan is the startup answer, so it is built here rather than
            // from an event.
            return Ok(Some(RulePlan {
                placement: None,
                trees: Some(
                    (0..self.count)
                        .map(|_| lifecycle::TreeRules {
                            biomes: vec!["z".repeat(self.size); NESTED_NAMES],
                            spacing: 1,
                            density_threshold: 0.0,
                        })
                        .collect(),
                ),
                clay: None,
                spawning: None,
            }));
        }
        Ok(None)
    }

    fn init(&mut self, _config: &Config, context: &InitContext) -> Result<Vec<Command>, Failure> {
        match self.mode {
            Mode::Normal
            | Mode::Refuse
            | Mode::Burst
            | Mode::Storage
            | Mode::Players
            | Mode::Teleport
            | Mode::StorageBatch
            | Mode::Zones
            | Mode::Oversized
            | Mode::Wide
            | Mode::Nested
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
        Ok(Vec::new())
    }

    fn on_events(
        &mut self,
        _context: &EventContext,
        events: &[Event],
    ) -> Result<Vec<Command>, Failure> {
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
                            };
                            format!("committed at {} {changes}", committed.revision)
                        }
                        OperationOutcome::Refused(failure) => {
                            format!("refused {}", operation_failure_name(failure))
                        }
                    };
                    if let Some(player) = self.reporter.as_deref() {
                        commands.push(message_player(
                            player,
                            format!(
                                "{} {} {} {outcome}",
                                self.greeting, answered.request, answered.operation_id
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
                        commands.push(operation_status(&answered.operation_id));
                    }
                }
                _ => {}
            }
        }
        Ok(commands)
    }

    fn shutdown(&mut self) -> Result<(), Failure> {
        log(LogLevel::Info, "hello plugin stopping");
        Ok(())
    }
}

export_plugin!(Hello);
