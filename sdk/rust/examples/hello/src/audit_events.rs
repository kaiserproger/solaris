//! P3 audit fixture: the world observations `solaris-audit` records, delivered to a
//! component instead of Luau.
//!
//! `mode = "audit-events"` runs this module. It is the audit consumer's own shape,
//! reduced to what a test can read off the wire: every world observation the
//! package subscribed to is appended to a bounded history with the tick of the
//! batch that carried it - the audit's own monotonic stamp - and reported twice,
//! once as the observation arrives and once when a player asks for the history with
//! `audit report`.
//!
//! The tick is the batch's `EventContext`, not an event of its own. The server's
//! pushed simulation tick is the clock the host stamps every batch with (and the
//! clock the host's timers are scheduled against), and no contract event carries
//! it: an audit record's stamp is therefore the tick of the callback that recorded
//! it, which is what this fixture stores and reports. A package that wanted a
//! callback *per tick* would be asking for an event the contract deliberately does
//! not have - the timers each instance owns are how a plugin acts on the clock.
//!
//! Two roles, and the difference between them is the point of the second one:
//!
//! - `audit_role = "recorder"` (the default) is the audit package. It answers
//!   `audit report` and reports every observation it received.
//! - `audit_role = "stranger"` is the same component deployed under another id,
//!   subscribing to none of these observations. It answers its own command root as
//!   a liveness fence and reports any world observation it is handed as a `LEAK`
//!   line: a subscriber that declared no subscription must be handed none, so a
//!   line here is what a test reads if delivery reaches more than the packages that
//!   asked for it.

use std::collections::VecDeque;

use solaris_plugin_sdk::events::{CommandInvoked, EventContext};
use solaris_plugin_sdk::{log, message_player, Command, Config, Event, Failure, LogLevel};

/// The prefix of every line this fixture publishes, so a driver can wait on its
/// own reports while the server interleaves frames of its own.
const PREFIX: &str = "P3_AUDIT";

/// How many records the fixture retains when the configuration names no bound. It
/// mirrors the audit package's own `maximum_records`: the history is a ring, and a
/// record that falls out of it is gone rather than silently merged.
const DEFAULT_RECORDS: usize = 64;

/// The two roles this fixture runs, named by `audit_role`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    /// The audit package: it records what it subscribed to and reports on demand.
    Recorder,
    /// A package that subscribed to nothing: it must stay silent about the world.
    Stranger,
}

impl Role {
    /// The name this role reports at startup, exactly as the configuration spells
    /// it.
    const fn name(self) -> &'static str {
        match self {
            Self::Recorder => "recorder",
            Self::Stranger => "stranger",
        }
    }
}

/// One recorded observation, in the audit package's own shape: what happened, who
/// did it, on which connection, where, and the tick the host stamped the batch
/// with.
struct Record {
    /// Index of this observation in everything the fixture has seen, counted from
    /// one and never reused. It is the fixture's own ordering evidence, not a value
    /// the contract carries.
    index: u64,
    /// The audit kind this observation was recorded as: `break`, `place`, `craft`,
    /// `pickup`, `kill`, `interact` or `death`.
    kind: &'static str,
    /// The stable identity the event named.
    actor: String,
    /// The connection the event named.
    session: u64,
    /// The dimension the event named.
    dimension: String,
    /// The kind's own detail: the block and its coordinate, the item and count, the
    /// entity type, or the pose.
    detail: String,
    /// The `EventContext.tick` of the batch that delivered the observation.
    tick: u64,
}

/// The fixture: its role, the history it keeps and the stamp it last observed.
pub struct Audit {
    role: Role,
    records: VecDeque<Record>,
    capacity: usize,
    seen: u64,
    latest_tick: u64,
}

impl Audit {
    /// Read this instance's role and history bound. An unknown role is refused
    /// rather than defaulted: the two roles answer different things, and a run that
    /// gets the wrong one would be a deployment mistake reported as a result.
    pub fn configure(config: &Config) -> Result<Self, Failure> {
        let value = config.toml();
        let role = match value
            .as_ref()
            .and_then(|value| value.get("audit_role"))
            .and_then(|role| role.as_str())
        {
            None | Some("recorder") => Role::Recorder,
            Some("stranger") => Role::Stranger,
            Some(_) => return Err(Failure::Invalid),
        };
        let capacity = value
            .as_ref()
            .and_then(|value| value.get("audit_records"))
            .and_then(|capacity| capacity.as_integer())
            .and_then(|capacity| usize::try_from(capacity).ok())
            .filter(|capacity| *capacity > 0)
            .unwrap_or(DEFAULT_RECORDS);
        Ok(Self {
            role,
            records: VecDeque::with_capacity(capacity),
            capacity,
            seen: 0,
            latest_tick: 0,
        })
    }

    /// Report the role this instance bound, so a driver knows the deployment it is
    /// about to drive is the one it wrote.
    pub fn init(&mut self) -> Result<Vec<Command>, Failure> {
        log(
            LogLevel::Info,
            &format!(
                "{PREFIX} ready role={} records={}",
                self.role.name(),
                self.capacity
            ),
        );
        Ok(Vec::new())
    }

    /// One delivered batch. The batch's context is the audit stamp: it is the tick
    /// the observations in it were produced at, taken from the server's own pushed
    /// clock and never from a counter of deliveries.
    pub fn on_events(
        &mut self,
        context: &EventContext,
        events: &[Event],
    ) -> Result<Vec<Command>, Failure> {
        self.latest_tick = self.latest_tick.max(context.tick);
        let mut commands = Vec::new();
        for event in events {
            let record = match event {
                Event::CommandInvoked(invoked) => {
                    commands.extend(self.command(invoked));
                    continue;
                }
                Event::PlayerBlockBroken(change) => self.record(
                    "break",
                    &change.player,
                    change.session,
                    &change.dimension,
                    format!(
                        "block={} at={},{},{}",
                        change.block, change.at.x, change.at.y, change.at.z
                    ),
                    context.tick,
                ),
                Event::PlayerBlockPlaced(change) => self.record(
                    "place",
                    &change.player,
                    change.session,
                    &change.dimension,
                    format!(
                        "block={} at={},{},{}",
                        change.block, change.at.x, change.at.y, change.at.z
                    ),
                    context.tick,
                ),
                Event::PlayerItemCrafted(crafted) => self.record(
                    "craft",
                    &crafted.player,
                    crafted.session,
                    &crafted.dimension,
                    format!("item={} count={}", crafted.item, crafted.count),
                    context.tick,
                ),
                Event::PlayerItemPickedUp(picked) => self.record(
                    "pickup",
                    &picked.player,
                    picked.session,
                    &picked.dimension,
                    format!("item={} count={}", picked.item, picked.count),
                    context.tick,
                ),
                Event::PlayerEntityKilled(killed) => self.record(
                    "kill",
                    &killed.player,
                    killed.session,
                    &killed.dimension,
                    format!("entity={}", killed.entity_type),
                    context.tick,
                ),
                Event::PlayerEntityInteracted(interacted) => self.record(
                    "interact",
                    &interacted.player,
                    interacted.session,
                    &interacted.dimension,
                    format!("entity={}", interacted.entity_type),
                    context.tick,
                ),
                Event::PlayerDied(died) => self.record(
                    "death",
                    &died.player,
                    died.session,
                    &died.dimension,
                    format!(
                        "position={:.3},{:.3},{:.3}",
                        died.position.x, died.position.y, died.position.z
                    ),
                    context.tick,
                ),
                _ => continue,
            };
            commands.push(self.observe(record));
        }
        Ok(commands)
    }

    /// The record fields every observation shares, so the arms above name only what
    /// differs between them. The index counts every observation this instance has
    /// seen, retained or not.
    fn record(
        &mut self,
        kind: &'static str,
        actor: &str,
        session: u64,
        dimension: &str,
        detail: String,
        tick: u64,
    ) -> Record {
        self.seen += 1;
        Record {
            index: self.seen,
            kind,
            actor: actor.to_owned(),
            session,
            dimension: dimension.to_owned(),
            detail,
            tick,
        }
    }

    /// Append one observation and answer the line that reports it.
    fn observe(&mut self, record: Record) -> Command {
        let text = match self.role {
            // The audit package's own report: what it recorded, where and when.
            Role::Recorder => render("observe", &record),
            // A package that subscribed to nothing was handed a world observation.
            // Nothing about this is quotable behaviour, so the line is loud.
            Role::Stranger => format!("{PREFIX} LEAK {}", render("observe", &record)),
        };
        let actor = record.actor.clone();
        if self.role == Role::Recorder {
            if self.records.len() == self.capacity {
                self.records.pop_front();
            }
            self.records.push_back(record);
        }
        message_player(&actor, text)
    }

    /// Answer the one command this fixture owns: the recorder's history request, or
    /// the stranger's liveness fence.
    fn command(&mut self, invoked: &CommandInvoked) -> Vec<Command> {
        match (self.role, invoked.arguments.first().map(String::as_str)) {
            (Role::Recorder, Some("report")) => self.report(invoked),
            (Role::Stranger, Some("ready")) => vec![message_player(
                &invoked.player,
                format!("{PREFIX} ready role={}", self.role.name()),
            )],
            _ => Vec::new(),
        }
    }

    /// The retained history, oldest first, and the totals behind it.
    ///
    /// The history is what the audit package would answer with: the records it kept
    /// in order, each with its own stamp, and a summary naming how many it retained,
    /// how many it has ever seen and the latest tick it recorded. A driver reads
    /// this after its last action, so an observation that never arrived or arrived
    /// twice is visible in one place instead of inferred from a wait that ended.
    fn report(&self, invoked: &CommandInvoked) -> Vec<Command> {
        let mut lines = Vec::with_capacity(self.records.len() + 1);
        for record in &self.records {
            lines.push(message_player(&invoked.player, render("record", record)));
        }
        lines.push(message_player(
            &invoked.player,
            format!(
                "{PREFIX} report records={} seen={} latest_tick={}",
                self.records.len(),
                self.seen,
                self.latest_tick
            ),
        ));
        lines
    }
}

/// One report line. Every line carries the whole record, so a driver compares
/// exact content rather than a count.
fn render(verb: &str, record: &Record) -> String {
    format!(
        "{PREFIX} {verb} {} kind={} tick={} actor={} session={} dimension={} {}",
        record.index,
        record.kind,
        record.tick,
        record.actor,
        record.session,
        record.dimension,
        record.detail
    )
}
