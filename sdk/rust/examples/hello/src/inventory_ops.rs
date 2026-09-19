//! P3 owned-inventory acceptance fixture: one owned-inventory action at a time.
//!
//! `mode = "owned-inventory"` runs this module. A player runs the package's
//! `inventory` root with exactly one action as its argument, and the fixture
//! answers one owned-inventory operation of the contract - a query, a transfer or
//! a reservation - against an endpoint the server itself owns. Every endpoint of
//! the contract is exercised:
//!
//! - `query` reads the player's own canonical inventory and reports the snapshot
//!   the owner answered: how many slots it holds and the fence revision it is at.
//! - `move` reads the player inventory's fence and then moves the seeded
//!   component-bearing tool one hotbar slot up, `restore` moves it back. Both
//!   prove a transfer moves the item as the stack it is - components and damage
//!   included - and report the resulting fence revision the owner answered.
//! - `stale` submits the same transfer against a fence one revision past the one
//!   it read: the owner must refuse `stale-revision` and move nothing.
//! - `foreign` reads the actor's own fence and submits a transfer whose player
//!   endpoint is another runtime player id: the owner must refuse `forbidden`,
//!   because a transfer may only name the actor's own inventory.
//! - `absent-resident` and `absent-carry` query a handle the resident owner does
//!   not hold and must receive `not-found`. `absent-warehouse` runs without an
//!   authored settlement profile and must receive `runtime-unavailable`. Neither
//!   missing authority may be represented as an empty inventory.
//! - `reserve` reads the player inventory's fence and reserves one emerald
//!   against it, then reports the whole reservation state the owner answered.
//! - `replay` submits the last committed transfer's record again, byte for byte,
//!   under a new correlation id: the owner must replay the recorded outcome
//!   instead of applying the move a second time, and the fixture checks the
//!   replayed revision is the one the original commit answered with.
//! - `conflict` submits that same durable operation id with different content:
//!   the owner must refuse `operation-conflict` rather than apply the substituted
//!   request.
//!
//! The fixture checks every answer against what the operation implies - the
//! request it named, whether a durable operation id is present, the endpoint the
//! snapshot names, the reason a refusal carries - and only then reports
//! `P3_INV <action> ...` to the player. An answer that does not match is reported
//! as `P3_INV_UNEXPECTED <action> step <n>: <detail>` on the operator log, never
//! as a marker: the host drops the callback that answered a failure, so a
//! diagnostic here can only be a log line.
//!
//! One action is outstanding at a time, and nothing here waits: each step is the
//! answer to the request the previous one staged.

use solaris_plugin_sdk::events::{
    CommandInvoked, Event, OperationAnswered, OperationOutcome, OperationPayload,
};
use solaris_plugin_sdk::inventories::{
    InventoryEndpoint, InventoryExpectedRevision, InventoryFence, InventoryMaterial,
    InventoryReservationSnapshot, InventoryResourcePlan, InventoryResult, InventoryWorkPortion,
    OwnedInventorySnapshot, OwnedItemTransfer,
};
use solaris_plugin_sdk::operation_types::OperationFailure;
use solaris_plugin_sdk::{
    log, message_player, query_owned_inventory, reserve_inventory_items, transfer_owned_items,
    Command, Config, Failure, LogLevel,
};

/// The command root the package's manifest declares for this fixture.
pub(crate) const ROOT: &str = "inventory";

/// The configuration `mode` that selects this fixture.
const MODE: &str = "owned-inventory";

/// The operator-ready line this fixture logs from `init`, which is what a driver
/// waits for before it sends the first action.
const READY: &str = "P3_INV ready";

/// The line prefix a step that could not be concluded is reported under.
const UNEXPECTED: &str = "P3_INV_UNEXPECTED";

/// The marker prefix every concluded step is reported under.
const MARKER: &str = "P3_INV";

/// The hotbar slot the world's own durable player file puts the tool in, and the
/// free hotbar slot a transfer stows it in.
const TOOL_SLOT: u8 = 36;
const STOW_SLOT: u8 = 37;

/// The resource a reservation holds, and how much of it one reservation asks for.
const EMERALD: &str = "minecraft:emerald";
const EMERALD_RESERVATION: u64 = 1;

/// How much work one resource portion of that reservation declares.
const RESERVATION_WORK_UNITS: u64 = 1;

/// Opaque absent handles. The resident ledger exists in this fixture; the
/// settlement runtime does not, so the warehouse query has no available owner.
const ABSENT_RESIDENT: &str = "resident-absent";
const ABSENT_WAREHOUSE: &str = "warehouse-absent";

/// The request ids and durable operation ids of the actions. They are the
/// fixture's own, so a test reads back which operation this guest decided to send
/// rather than a constant the host could have written.
const QUERY_REQUEST: &str = "inv-query";
const MOVE_FENCE: &str = "inv-move-fence";
const MOVE_REQUEST: &str = "inv-move";
const MOVE_OPERATION: &str = "inv-op-move";
const RESTORE_FENCE: &str = "inv-restore-fence";
const RESTORE_REQUEST: &str = "inv-restore";
const RESTORE_OPERATION: &str = "inv-op-restore";
const STALE_FENCE: &str = "inv-stale-fence";
const STALE_REQUEST: &str = "inv-stale";
const STALE_OPERATION: &str = "inv-op-stale";
const RESERVE_FENCE: &str = "inv-reserve-fence";
const RESERVE_REQUEST: &str = "inv-reserve";
const RESERVE_OPERATION: &str = "inv-op-reserve";
const FOREIGN_FENCE: &str = "inv-foreign-fence";
const FOREIGN_REQUEST: &str = "inv-foreign";
const FOREIGN_OPERATION: &str = "inv-op-foreign";
const REPLAY_REQUEST: &str = "inv-replay";
const CONFLICT_REQUEST: &str = "inv-conflict";

/// One owned-inventory action a player can ask the fixture for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Read the player's own canonical inventory.
    Query,
    /// Move the seeded tool from `TOOL_SLOT` to `STOW_SLOT`.
    Move,
    /// Move it back.
    Restore,
    /// Submit `Move` against a fence one revision past the one read.
    Stale,
    /// Submit a transfer naming another runtime player id.
    Foreign,
    /// Read the resident equipment endpoint of a handle no owner holds.
    AbsentResident,
    /// Read the resident carry endpoint of a handle no owner holds.
    AbsentCarry,
    /// Read the warehouse endpoint of a handle no owner holds.
    AbsentWarehouse,
    /// Reserve one emerald against the player inventory.
    Reserve,
    /// Submit the last committed transfer's record again.
    Replay,
    /// Submit that record under the same durable id with different content.
    Conflict,
}

impl Action {
    /// The action one argument names, or nothing.
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "query" => Self::Query,
            "move" => Self::Move,
            "restore" => Self::Restore,
            "stale" => Self::Stale,
            "foreign" => Self::Foreign,
            "absent-resident" => Self::AbsentResident,
            "absent-carry" => Self::AbsentCarry,
            "absent-warehouse" => Self::AbsentWarehouse,
            "reserve" => Self::Reserve,
            "replay" => Self::Replay,
            "conflict" => Self::Conflict,
            _ => return None,
        })
    }

    /// The word this action is named by in a marker.
    fn name(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Move => "move",
            Self::Restore => "restore",
            Self::Stale => "stale",
            Self::Foreign => "foreign",
            Self::AbsentResident => "absent-resident",
            Self::AbsentCarry => "absent-carry",
            Self::AbsentWarehouse => "absent-warehouse",
            Self::Reserve => "reserve",
            Self::Replay => "replay",
            Self::Conflict => "conflict",
        }
    }

    /// The fence read this action stages before its final request, when it stages
    /// one.
    fn fence_request(self) -> Option<&'static str> {
        match self {
            Self::Move => Some(MOVE_FENCE),
            Self::Restore => Some(RESTORE_FENCE),
            Self::Stale => Some(STALE_FENCE),
            Self::Foreign => Some(FOREIGN_FENCE),
            Self::Reserve => Some(RESERVE_FENCE),
            _ => None,
        }
    }
}

/// Where one action stands: which answer the fixture is waiting for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Nothing outstanding: the fixture is between actions.
    Idle,
    /// The fence read a mutation stages first.
    Fence,
    /// The final answer of the action.
    Answer,
}

impl Step {
    /// The number a diagnostic line names this step by.
    fn number(self) -> usize {
        match self {
            Self::Idle => 0,
            Self::Fence => 1,
            Self::Answer => 2,
        }
    }
}

/// The record of one transfer the fixture staged, which is what a replay repeats
/// byte for byte and a conflict reuses with different content.
struct TransferRecord {
    /// The durable operation id the transfer committed under.
    operation_id: String,
    /// The correlation id the transfer was staged with. A replay deliberately
    /// does not reuse it: the durable identity is the operation id, not this one.
    request: String,
    /// The actor the transfer named.
    actor_id: u64,
    /// The transfers as they were staged.
    transfers: Vec<OwnedItemTransfer>,
    /// The fences as they were read.
    expected_revisions: Vec<InventoryExpectedRevision>,
    /// The revision the owner answered for the actor's own endpoint.
    revision: u64,
}

/// The fixture itself: the action in flight and everything it read.
pub struct Fixture {
    /// The session of the connection the action in flight runs on.
    session: u64,
    /// The player the marker goes to, from the command that began the action.
    reporter: String,
    /// The action in flight, if any.
    action: Option<Action>,
    /// What the fixture is waiting for.
    step: Step,
    /// The fence the action read, for the mutation it stages next.
    fence: Option<InventoryFence>,
    /// The last committed transfer, for `replay` and `conflict`.
    last_transfer: Option<TransferRecord>,
}

impl Fixture {
    /// Read the mode out of the package's own configuration.
    ///
    /// The fixture is only the `owned-inventory` one, so a configuration that
    /// says something else is this package being wired to the wrong mode, which
    /// is refused here rather than answered by running the wrong fixture.
    pub fn configure(config: &Config) -> Result<Self, Failure> {
        let mode = config.toml().and_then(|value| {
            value
                .get("mode")
                .and_then(|mode| mode.as_str().map(str::to_owned))
        });
        if mode.as_deref() != Some(MODE) {
            return Err(Failure::Invalid);
        }
        Ok(Self {
            session: 0,
            reporter: String::new(),
            action: None,
            step: Step::Idle,
            fence: None,
            last_transfer: None,
        })
    }

    /// The startup phase: nothing to stage, one readiness line for a driver.
    pub fn init(&mut self) -> Result<Vec<Command>, Failure> {
        log(LogLevel::Info, READY);
        Ok(Vec::new())
    }

    /// One delivered batch: the command root a player ran, and at most one answer
    /// to this fixture's outstanding request.
    pub fn on_events(&mut self, events: &[Event]) -> Result<Vec<Command>, Failure> {
        let mut answers = events.iter().filter(|event| is_answer(event));
        let first = answers.next();
        if answers.next().is_some() {
            return Err(self.unexpected("two answers arrived for one outstanding request"));
        }
        let mut commands = Vec::new();
        if let Some(event) = first {
            commands.extend(self.answer(event)?);
        }
        for event in events {
            if let Event::CommandInvoked(invoked) = event {
                if invoked.name == ROOT {
                    commands.extend(self.begin(invoked)?);
                }
            }
        }
        Ok(commands)
    }

    /// One `inventory` command: what it names, and the request that starts it.
    fn begin(&mut self, invoked: &CommandInvoked) -> Result<Vec<Command>, Failure> {
        if self.action.is_some() {
            return Err(self.unexpected("an action was asked for while another was in flight"));
        }
        let Some(action) = invoked
            .arguments
            .first()
            .and_then(|name| Action::from_name(name))
        else {
            return Err(self.unexpected(&format!(
                "the inventory root was asked for without one of its actions: {:?}",
                invoked.arguments
            )));
        };
        self.session = invoked.session;
        self.reporter = invoked.player.clone();
        self.action = Some(action);
        self.fence = None;
        // A fence read is the first step of every mutation action, and the final
        // step of the others is the request itself.
        if let Some(request) = action.fence_request() {
            self.step = Step::Fence;
            return Ok(vec![query_owned_inventory(
                request,
                player_endpoint(self.session),
                None,
            )]);
        }
        self.step = Step::Answer;
        Ok(vec![self.mutation(action)?])
    }

    /// The final request one action stages, from the state it read.
    fn mutation(&mut self, action: Action) -> Result<Command, Failure> {
        let endpoint = player_endpoint(self.session);
        Ok(match action {
            Action::Query => query_owned_inventory(QUERY_REQUEST, endpoint, None),
            Action::AbsentResident => query_owned_inventory(
                QUERY_REQUEST,
                InventoryEndpoint::ResidentEquipment(ABSENT_RESIDENT.to_owned()),
                None,
            ),
            Action::AbsentCarry => query_owned_inventory(
                QUERY_REQUEST,
                InventoryEndpoint::ResidentCarry(ABSENT_RESIDENT.to_owned()),
                None,
            ),
            Action::AbsentWarehouse => query_owned_inventory(
                QUERY_REQUEST,
                InventoryEndpoint::Warehouse(ABSENT_WAREHOUSE.to_owned()),
                None,
            ),
            Action::Move => {
                let fence = self.read_fence()?;
                let record = transfer_record(
                    MOVE_OPERATION,
                    MOVE_REQUEST,
                    self.session,
                    vec![tool_transfer(endpoint, TOOL_SLOT, STOW_SLOT)],
                    &fence,
                );
                let command = record.command();
                self.last_transfer = Some(record);
                command
            }
            Action::Restore => {
                let fence = self.read_fence()?;
                let record = transfer_record(
                    RESTORE_OPERATION,
                    RESTORE_REQUEST,
                    self.session,
                    vec![tool_transfer(endpoint, STOW_SLOT, TOOL_SLOT)],
                    &fence,
                );
                let command = record.command();
                self.last_transfer = Some(record);
                command
            }
            Action::Stale => {
                let fence = self.read_fence()?;
                // One revision past the one read, with the hash the owner
                // answered: the endpoint has moved past this fence, which is the
                // whole case.
                let stale = InventoryFence {
                    revision: fence.revision + 1,
                    snapshot_hash: fence.snapshot_hash,
                };
                transfer_owned_items(
                    STALE_REQUEST,
                    STALE_OPERATION,
                    self.session,
                    vec![tool_transfer(endpoint.clone(), TOOL_SLOT, STOW_SLOT)],
                    vec![InventoryExpectedRevision {
                        endpoint,
                        fence: stale,
                    }],
                )
            }
            Action::Foreign => {
                // Another runtime player id, which is a player whose inventory
                // this actor may not move anything out of. The actor's own fence
                // is what this fixture read; the owner refuses on the endpoint
                // before it ever compares one.
                let fence = self.read_fence()?;
                let foreign = player_endpoint(self.session + 1);
                transfer_owned_items(
                    FOREIGN_REQUEST,
                    FOREIGN_OPERATION,
                    self.session,
                    vec![tool_transfer(foreign.clone(), TOOL_SLOT, STOW_SLOT)],
                    vec![InventoryExpectedRevision {
                        endpoint: foreign,
                        fence,
                    }],
                )
            }
            Action::Reserve => {
                let fence = self.read_fence()?;
                let plan = InventoryResourcePlan {
                    portions: vec![InventoryWorkPortion {
                        work_units: RESERVATION_WORK_UNITS,
                        materials: vec![InventoryMaterial {
                            resource_id: EMERALD.to_owned(),
                            quantity: EMERALD_RESERVATION,
                        }],
                    }],
                };
                reserve_inventory_items(RESERVE_REQUEST, RESERVE_OPERATION, endpoint, plan, fence)
            }
            Action::Replay => {
                let Some(record) = &self.last_transfer else {
                    return Err(self.unexpected("no transfer was committed to replay"));
                };
                // The same durable record under a new correlation id: the
                // operation id is the durable identity, and the content is
                // byte-identical.
                transfer_owned_items(
                    REPLAY_REQUEST,
                    &record.operation_id,
                    record.actor_id,
                    record.transfers.clone(),
                    record.expected_revisions.clone(),
                )
            }
            Action::Conflict => {
                let Some(record) = &self.last_transfer else {
                    return Err(self.unexpected("no transfer was committed to reuse"));
                };
                let mut transfers = record.transfers.clone();
                let Some(first) = transfers.first_mut() else {
                    return Err(self.unexpected("the recorded transfer lost its members"));
                };
                // Different content under the same durable id.
                first.count += 1;
                transfer_owned_items(
                    CONFLICT_REQUEST,
                    &record.operation_id,
                    record.actor_id,
                    transfers,
                    record.expected_revisions.clone(),
                )
            }
        })
    }

    /// The fence the fence read answered, which the mutation that follows stages
    /// against.
    fn read_fence(&self) -> Result<InventoryFence, Failure> {
        self.fence
            .clone()
            .ok_or_else(|| self.reject("a mutation was staged without its fence read"))
    }

    /// One answer to this fixture's outstanding request.
    fn answer(&mut self, event: &Event) -> Result<Vec<Command>, Failure> {
        let Some(action) = self.action else {
            return Err(self.unexpected("an answer arrived with no request outstanding"));
        };
        let Event::OperationAnswered(answered) = event else {
            return Err(self.unexpected("an event that is not an operation answer"));
        };
        match self.step {
            Step::Fence => {
                let Some(request) = action.fence_request() else {
                    return Err(self.unexpected("a fence answer arrived for an action without one"));
                };
                if answered.request != request {
                    return Err(self.unexpected(&format!(
                        "the fence read was answered under {}, not {request}",
                        answered.request
                    )));
                }
                if answered.operation_id.is_some() {
                    return Err(self.unexpected("a query answer carried a durable operation id"));
                }
                let fence = self.snapshot_fence(answered, &player_endpoint(self.session))?;
                self.fence = Some(fence);
                self.step = Step::Answer;
                let command = self.mutation(action)?;
                Ok(vec![command])
            }
            Step::Answer => {
                self.action = None;
                self.step = Step::Idle;
                self.fence = None;
                self.conclude(action, answered)
            }
            Step::Idle => Err(self.unexpected("an answer arrived while nothing was outstanding")),
        }
    }

    /// Check one final answer against what the action's operation implies, and
    /// report the marker only when it matches.
    fn conclude(
        &mut self,
        action: Action,
        answered: &OperationAnswered,
    ) -> Result<Vec<Command>, Failure> {
        match action {
            Action::Query => {
                if answered.operation_id.is_some() {
                    return Err(self.unexpected("a query answer carried a durable operation id"));
                }
                let endpoint = player_endpoint(self.session);
                let snapshot = self.snapshot(answered)?;
                if !same_endpoint(&snapshot.endpoint, &endpoint) {
                    return Err(
                        self.unexpected("the snapshot names another endpoint than the one asked")
                    );
                }
                let revision = snapshot.fence.revision;
                let slots = snapshot.slots.len();
                Ok(vec![self.report(&format!(
                    "query committed slots={slots} revision={revision}"
                ))])
            }
            Action::AbsentResident | Action::AbsentCarry => {
                self.expect_refusal(action, answered, OperationFailure::NotFound)
            }
            Action::AbsentWarehouse => {
                self.expect_refusal(action, answered, OperationFailure::RuntimeUnavailable)
            }
            Action::Move | Action::Restore => {
                let revision = self.expect_transfer(action, answered)?;
                self.record_revision(revision);
                Ok(vec![self.report(&format!(
                    "{} committed revision={revision}",
                    action.name()
                ))])
            }
            Action::Stale => self.expect_refusal(action, answered, OperationFailure::StaleRevision),
            Action::Foreign => self.expect_refusal(action, answered, OperationFailure::Forbidden),
            Action::Conflict => {
                self.expect_refusal(action, answered, OperationFailure::OperationConflict)
            }
            Action::Replay => {
                let revision = self.expect_transfer(action, answered)?;
                let Some(record) = &self.last_transfer else {
                    return Err(self.unexpected("a replay concluded without a recorded transfer"));
                };
                if revision != record.revision {
                    return Err(self.unexpected(&format!(
                        "the replay answered revision {revision}, not the recorded {}",
                        record.revision
                    )));
                }
                Ok(vec![self.report(&format!(
                    "replay committed revision={revision} replayed=true"
                ))])
            }
            Action::Reserve => {
                let reservation = self.reservation(answered)?;
                let reserved: u64 = reservation.quantities.iter().map(|q| q.reserved).sum();
                if reservation.released {
                    return Err(self.unexpected("a fresh reservation answered released"));
                }
                Ok(vec![self.report(&format!(
                    "reserve committed resources={} reserved={reserved} released=false",
                    reservation.quantities.len()
                ))])
            }
        }
    }

    /// Remember the revision one committed transfer answered with, so a later
    /// replay can be checked against it.
    fn record_revision(&mut self, revision: u64) {
        if let Some(record) = &mut self.last_transfer {
            record.revision = revision;
        }
    }

    /// The snapshot fence one committed query answered with, checking it names the
    /// endpoint the query asked about.
    fn snapshot_fence(
        &self,
        answered: &OperationAnswered,
        endpoint: &InventoryEndpoint,
    ) -> Result<InventoryFence, Failure> {
        let snapshot = self.snapshot(answered)?;
        if !same_endpoint(&snapshot.endpoint, endpoint) {
            return Err(self.unexpected("the snapshot names another endpoint than the one asked"));
        }
        Ok(snapshot.fence)
    }

    /// The snapshot one committed query answered with.
    fn snapshot(&self, answered: &OperationAnswered) -> Result<OwnedInventorySnapshot, Failure> {
        let OperationOutcome::Committed(committed) = &answered.outcome else {
            return Err(self.unexpected("a query answered refused"));
        };
        let OperationPayload::OwnedInventory(InventoryResult::Snapshot(snapshot)) =
            &committed.payload
        else {
            return Err(self.unexpected("a query answered a payload that is not a snapshot"));
        };
        Ok(snapshot.clone())
    }

    /// The revision one committed transfer answered for the actor's own endpoint,
    /// checking the answer is the fence list of the endpoints it touched.
    fn expect_transfer(
        &self,
        action: Action,
        answered: &OperationAnswered,
    ) -> Result<u64, Failure> {
        let Some(operation_id) = answered.operation_id.as_deref() else {
            return Err(self.unexpected("a mutation answered without a durable operation id"));
        };
        let expected_operation = match action {
            Action::Move => MOVE_OPERATION,
            Action::Restore => RESTORE_OPERATION,
            Action::Replay => self
                .last_transfer
                .as_ref()
                .map_or(REPLAY_REQUEST, |record| record.operation_id.as_str()),
            _ => return Err(self.unexpected("a non-transfer action answered as a transfer")),
        };
        if operation_id != expected_operation {
            return Err(self.unexpected(&format!(
                "the transfer was answered under {operation_id}, not {expected_operation}"
            )));
        }
        let OperationOutcome::Committed(committed) = &answered.outcome else {
            return Err(self.unexpected("a transfer answered refused"));
        };
        let OperationPayload::OwnedInventory(InventoryResult::Transfer(inventories)) =
            &committed.payload
        else {
            return Err(self.unexpected("a transfer answered a payload that is not a fence list"));
        };
        let endpoint = player_endpoint(self.session);
        let Some(entry) = inventories
            .iter()
            .find(|expected| same_endpoint(&expected.endpoint, &endpoint))
        else {
            return Err(self.unexpected("the transfer answered no fence for the actor inventory"));
        };
        if entry.fence.snapshot_hash.len() != 64 {
            return Err(self.unexpected("the transfer answered a fence the server did not mint"));
        }
        Ok(entry.fence.revision)
    }

    /// The whole reservation state one committed reservation answered with.
    fn reservation(
        &self,
        answered: &OperationAnswered,
    ) -> Result<InventoryReservationSnapshot, Failure> {
        let Some(operation_id) = answered.operation_id.as_deref() else {
            return Err(self.unexpected("a reservation answered without a durable operation id"));
        };
        if operation_id != RESERVE_OPERATION {
            return Err(self.unexpected(&format!(
                "the reservation was answered under {operation_id}, not {RESERVE_OPERATION}"
            )));
        }
        let OperationOutcome::Committed(committed) = &answered.outcome else {
            return Err(self.unexpected("a reservation answered refused"));
        };
        let OperationPayload::OwnedInventory(InventoryResult::Reservation(reservation)) =
            &committed.payload
        else {
            return Err(
                self.unexpected("a reservation answered a payload that is not a reservation")
            );
        };
        if reservation.reservation_ref.is_empty() || reservation.quantities.is_empty() {
            return Err(
                self.unexpected("the reservation answered without a reference or quantities")
            );
        }
        let reserved: u64 = reservation.quantities.iter().map(|q| q.reserved).sum();
        let accounted: u64 = reservation
            .quantities
            .iter()
            .map(|q| q.consumed + q.returned + q.remaining)
            .sum();
        if reserved != accounted {
            return Err(self.unexpected("the reservation answered quantities that do not add up"));
        }
        Ok(reservation.clone())
    }

    /// One answer that must be the action's own refusal, with the reason the
    /// contract names.
    fn expect_refusal(
        &self,
        action: Action,
        answered: &OperationAnswered,
        reason: OperationFailure,
    ) -> Result<Vec<Command>, Failure> {
        let OperationOutcome::Refused(refused) = &answered.outcome else {
            return Err(
                self.unexpected(&format!("{} committed instead of refusing", action.name()))
            );
        };
        if refused.reason != reason {
            return Err(self.unexpected(&format!(
                "{} was refused {} instead of {}",
                action.name(),
                failure_name(&refused.reason),
                failure_name(&reason)
            )));
        }
        Ok(vec![self.report(&format!(
            "{} refused {}",
            action.name(),
            failure_name(&reason)
        ))])
    }

    /// One marker line to the player who asked.
    fn report(&self, body: &str) -> Command {
        message_player(&self.reporter, format!("{MARKER} {body}"))
    }

    /// Log one step this fixture could not conclude, and fail the callback with it
    /// so the batch it staged is dropped.
    fn unexpected(&self, detail: &str) -> Failure {
        let action = self.action.map_or("none", Action::name);
        log(
            LogLevel::Error,
            &format!(
                "{UNEXPECTED} {action} step {}: {detail}",
                self.step.number()
            ),
        );
        Failure::Failed
    }

    /// The same diagnostic as an invalid-input failure, for a request this fixture
    /// never made.
    fn reject(&self, detail: &str) -> Failure {
        log(
            LogLevel::Error,
            &format!("{UNEXPECTED} fixture step {}: {detail}", self.step.number()),
        );
        Failure::Invalid
    }
}

impl TransferRecord {
    /// The transfer command this record describes.
    fn command(&self) -> Command {
        transfer_owned_items(
            &self.request,
            &self.operation_id,
            self.actor_id,
            self.transfers.clone(),
            self.expected_revisions.clone(),
        )
    }
}

/// One runtime player id as its own owned inventory.
fn player_endpoint(player_id: u64) -> InventoryEndpoint {
    InventoryEndpoint::PlayerInventory(player_id)
}

/// One move of a single tool between two slots of the endpoint it names.
fn tool_transfer(
    endpoint: InventoryEndpoint,
    source_slot: u8,
    destination_slot: u8,
) -> OwnedItemTransfer {
    OwnedItemTransfer {
        destination: endpoint.clone(),
        source: endpoint,
        source_slot,
        destination_slot,
        count: 1,
    }
}

/// One transfer record, fenced by the endpoint's own fence.
fn transfer_record(
    operation_id: &str,
    request: &str,
    actor_id: u64,
    transfers: Vec<OwnedItemTransfer>,
    fence: &InventoryFence,
) -> TransferRecord {
    TransferRecord {
        operation_id: operation_id.to_owned(),
        request: request.to_owned(),
        actor_id,
        transfers,
        expected_revisions: vec![InventoryExpectedRevision {
            endpoint: player_endpoint(actor_id),
            fence: fence.clone(),
        }],
        revision: 0,
    }
}

/// Whether two endpoints name the same container.
///
/// The generated types carry no `PartialEq`, and this contract compares an
/// endpoint's whole identity - its case and its value - so the comparison is
/// written out rather than derived.
fn same_endpoint(left: &InventoryEndpoint, right: &InventoryEndpoint) -> bool {
    match (left, right) {
        (InventoryEndpoint::PlayerInventory(left), InventoryEndpoint::PlayerInventory(right)) => {
            left == right
        }
        (InventoryEndpoint::Warehouse(left), InventoryEndpoint::Warehouse(right))
        | (
            InventoryEndpoint::ResidentEquipment(left),
            InventoryEndpoint::ResidentEquipment(right),
        )
        | (InventoryEndpoint::ResidentCarry(left), InventoryEndpoint::ResidentCarry(right)) => {
            left == right
        }
        _ => false,
    }
}

/// Whether one event is the host answering a request this fixture made. Every
/// other event - a join, a chat line, another plugin's - is none of this
/// fixture's business.
fn is_answer(event: &Event) -> bool {
    matches!(event, Event::OperationAnswered(_))
}

/// How one refusal reason is named in a marker: the contract's own vocabulary, so
/// a test reads the reason the server gave rather than a wording this guest chose.
fn failure_name(failure: &OperationFailure) -> &'static str {
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
