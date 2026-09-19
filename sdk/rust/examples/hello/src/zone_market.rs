//! P3 zone-market fixture: the zone owner's own boundary transitions drive the
//! existing market.
//!
//! `mode = "zone-market"` runs this module. A player runs the package's `trade`
//! root with `zone-setup`, and the fixture registers the fixed `trade-zone` box
//! through the existing `upsert-zone` command. It reports readiness only from the
//! owner's own answer to that command: `P3_ZONE ready zone=trade-zone` once the
//! owner says it applied the box, nothing at all when it refuses it.
//!
//! From then on the owner decides. The boundary transition it publishes for that
//! box opens the fixture's existing market on the connection the transition names,
//! and the opposite transition closes the menu this fixture is holding on that
//! menu's own connection. Every marker of this module but the readiness one is a
//! request fence: `P3_ZONE entered` and `P3_ZONE exited` say what the fixture
//! staged when the owner told it the player crossed, and the server's own frames
//! and click routing are what establish the effect. Nothing here fabricates a
//! transition - a run that never moved a player across the boundary opens no
//! market and publishes no transition marker.
//!
//! Every other `trade` action stays the inventory/storage fixture's own. This
//! module owns one `Trade`, answers `zone-setup` itself, and hands the fixture
//! every other event, so the ledger state machine and the menu it tracks have one
//! implementation and the zone path only decides when to ask for that market.

use crate::inventory_storage::{Trade, ROOT};
use crate::{corner, upsert_zone, ZONE_DIMENSION};
use solaris_plugin_sdk::events::{CommandInvoked, ZoneCommandOutcome};
use solaris_plugin_sdk::{log, message_player, Command, Event, Failure, LogLevel};

/// The one `trade` action this fixture answers itself. Every other action of the
/// root belongs to the inventory/storage fixture, which is told this name so it
/// neither answers it nor reports it as unknown.
pub(crate) const SETUP: &str = "zone-setup";

/// The one zone this fixture registers: the fixed box the P3 contract names. It is
/// the plugin's own id, so the owner keys it by this package.
const ZONE: &str = "trade-zone";
/// The box's corners, exactly as the shared contract states them: a small room in
/// the overworld, above the sea-level boxes the `zones` mode already uses.
const MINIMUM: (f64, f64, f64) = (10.0, 80.0, 10.0);
const MAXIMUM: (f64, f64, f64) = (16.0, 84.0, 16.0);

/// The three lines this fixture publishes, each naming the zone it is about. The
/// readiness line is the only one that waits for the owner's own answer; the two
/// transition lines are fences over what the fixture staged for the transition the
/// owner produced.
const READY: &str = "P3_ZONE ready zone=trade-zone";
const ENTERED: &str = "P3_ZONE entered zone=trade-zone";
const EXITED: &str = "P3_ZONE exited zone=trade-zone";

/// What the fixture logs when a connection leaves, with the session it was. A
/// driver reads this line to know the server has finished with the connection it
/// traded on, rather than guessing how long that takes.
const LEAVE_PREFIX: &str = "P3_ZONE_LEFT";

/// The fixture itself: the trade state machine it drives, and the registration a
/// driver waits on.
pub struct ZoneMarket {
    /// The inventory/storage fixture: every other `trade` action is its own.
    trade: Trade,
    /// The player the registration's answer is reported to, from the command that
    /// asked for it.
    reporter: Option<String>,
    /// Whether a registration this instance sent is still unanswered.
    registering: bool,
}

impl ZoneMarket {
    /// The fixture before any command: no zone registered and no action in flight.
    #[must_use]
    pub fn new() -> Self {
        Self {
            trade: Trade::new(),
            reporter: None,
            registering: false,
        }
    }

    /// One delivered batch: the `zone-setup` command, the owner's own boundary
    /// transitions, the answer to this fixture's registration, and everything the
    /// trade fixture answers.
    pub fn on_events(&mut self, events: &[Event]) -> Result<Vec<Command>, Failure> {
        let mut commands = Vec::new();
        for event in events {
            match event {
                Event::CommandInvoked(invoked) if names_setup(invoked) => {
                    commands.extend(self.setup(invoked)?);
                }
                Event::ZoneCommandAnswered(answered) if answered.zone == ZONE => {
                    commands.extend(self.registered(&answered.outcome)?);
                }
                // One boundary transition this fixture's own zone produced. The
                // market is asked for on the connection the transition names - the
                // original session of the join - and the marker reports that
                // request, not that a menu opened.
                Event::PlayerZoneEntered(transition) if transition.zone == ZONE => {
                    commands.extend(self.trade.open_market(transition.session));
                    commands.push(message_player(&transition.player, ENTERED));
                }
                // The opposite transition closes the menu this fixture is holding,
                // on the connection that menu was opened on. The zone's own
                // connection is not consulted: a menu belongs to the session that
                // opened it, which is the one a close has to name.
                Event::PlayerZoneExited(transition) if transition.zone == ZONE => {
                    commands.extend(self.trade.close_market());
                    commands.push(message_player(&transition.player, EXITED));
                }
                // A connection leaving. The server deregisters a session before it
                // announces the leave, so this line is also the point after which
                // the same player can join again - which is what a driver that has
                // to reconnect waits for.
                Event::PlayerLeft(left) => {
                    log(LogLevel::Debug, &format!("{LEAVE_PREFIX} {}", left.session));
                }
                _ => {}
            }
        }
        // Everything else is the inventory/storage fixture's, including the
        // actions of the `trade` root this module does not own.
        commands.extend(self.trade.on_events(events, &[SETUP])?);
        Ok(commands)
    }

    /// One `zone-setup` command: it registers the fixed box and says nothing else.
    /// The registration is a command to the existing zone owner and carries no
    /// promise of its own, so no marker is published here.
    fn setup(&mut self, invoked: &CommandInvoked) -> Result<Vec<Command>, Failure> {
        if self.registering {
            return Err(self.reject("a registration was asked for while another was unanswered"));
        }
        self.reporter = Some(invoked.player.clone());
        self.registering = true;
        Ok(vec![upsert_zone(
            ZONE,
            ZONE_DIMENSION,
            corner(MINIMUM.0, MINIMUM.1, MINIMUM.2),
            corner(MAXIMUM.0, MAXIMUM.1, MAXIMUM.2),
        )])
    }

    /// What the owner did with the registration this instance sent.
    fn registered(&mut self, outcome: &ZoneCommandOutcome) -> Result<Vec<Command>, Failure> {
        if !self.registering {
            return Err(self.reject("the owner answered a registration this fixture never sent"));
        }
        self.registering = false;
        match outcome {
            ZoneCommandOutcome::Applied => {
                let Some(player) = self.reporter.clone() else {
                    return Err(self.reject("no player is waiting for the registration's answer"));
                };
                Ok(vec![message_player(&player, READY)])
            }
            // The owner holds no such box, so no transition can ever be published
            // for it. A marker here would announce a market no boundary can open,
            // so the refusal is a diagnostic and nothing else.
            ZoneCommandOutcome::Refused => {
                Err(self.reject("the zone owner refused to register trade-zone"))
            }
        }
    }

    /// The one failure this fixture reports: a line an operator can read, and the
    /// plugin's own failure, so the deployment sees a callback that did not answer
    /// and no marker is published for it.
    fn reject(&self, detail: &str) -> Failure {
        log(LogLevel::Error, &format!("P3_ZONE_UNEXPECTED {detail}"));
        Failure::Failed
    }
}

/// Whether one command is this fixture's own `zone-setup` action.
fn names_setup(invoked: &CommandInvoked) -> bool {
    invoked.name == ROOT && invoked.arguments.first().map(String::as_str) == Some(SETUP)
}
