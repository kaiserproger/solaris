//! P3 resident and resident-order acceptance fixture: one owner package that
//! walks a real resident through the whole used surface, and one observer package
//! that is refused on another plugin's resident.
//!
//! `mode = "resident-operations"` runs this module and `role` selects which of the
//! two packages a deployment is - `owner` by default, `observer` for the second
//! one. Every action is one player command, one operation in flight and one
//! answer: nothing here waits, and each step is the answer to the request the
//! previous step staged.
//!
//! The owner walks the sequence a settlement really runs:
//!
//! - `site` lists the settlement sites the owner holds - the contract's own
//!   largest page, so one listing names every authored candidate the scanned
//!   region carries - and reports the next free home point of interest of an
//!   authored site, with the cells that POI stands at and the revision the owner
//!   answered for that site. The site id, the POI id, the revision and the cells
//!   are the server's own; the fixture derives none of them.
//! - `goto` teleports this connection to that POI cell. The reservation below is
//!   grounded on real terrain and a resident materialises only where the owner can
//!   already read the chunk, so the teleport is what loads it.
//! - `spawn` re-reads the site the driver walks to, reserves that POI at the
//!   revision the re-read answered, and then materialises the resident the
//!   reservation is for: a reservation the owner commits moves the site's own
//!   revision, so the fence a reserve names is never an earlier listing's. A
//!   refusal that touched nothing hands the reservation back with
//!   `release_resident_site`, exactly as the shipped settlement plugin does, so a
//!   later `spawn` reserves again instead of finding a reservation it cannot
//!   consume. `unloaded` is the one refusal a driver retries: it means the chunk
//!   the previous teleport asked for is not readable yet.
//! - `pois` binds home, work and meeting at the revision this fixture read, and
//!   `stale-pois` sends the revision it read *before* that binding: the owner
//!   refuses the second and leaves the binding it refused exactly as it was, which
//!   the following `pois` proves by committing at the revision the refusal did not
//!   touch.
//! - `claim` claims the very villager the spawn materialised, by the stable entity
//!   uuid the spawn answered and at the resident revision the committed binding
//!   named - the only revision the owner accepts for an entity this plugin already
//!   holds - so the owner answers this plugin's own resident instead of adopting a
//!   second one. `stale-claim` sends the revision that preceded that binding and is
//!   refused `stale_revision` with the resident untouched.
//! - `withdraw` moves one emerald out of this player's own inventory and into the
//!   resident's carry through the owned-inventory owner: both endpoints are read
//!   for their fences first, and the transfer names exactly those fences.
//! - `missing-work` assigns a haul of that emerald before the withdrawal, so the
//!   owner reports `missing_input` with nothing moved; `work` assigns the same
//!   haul afterwards and commits the real move; `stale-work` sends the revision
//!   that preceded the last committed assignment and is refused with nothing
//!   applied.
//! - `cancel-work` cancels the assignment at the revision the last answer named.
//! - `order` issues one follow order for this connection; `stale-order` issues it
//!   again at the revision that preceded the committed one, which the owner
//!   refuses as a batch whose every member is `stale_revision`; `cancel-order`
//!   cancels the order at the current revision.
//! - `gear` reads the resident's equipment back through the owned-inventory owner,
//!   so a test reads the real slots rather than a count this fixture remembered.
//! - `demobilize` turns the resident back into a civilian, and a following `gear`
//!   shows whether the gear stayed.
//!
//! The observer package asks for another plugin's resident, one shape per action:
//! `resident-fence <handle> pois` binds points of interest on it, `<handle> work`
//! gives it work, `<handle> order` cancels an order it holds and `<uuid> claim 0`
//! attempts to adopt that entity as a new resident. The owner must refuse the
//! foreign binding and claim as `forbidden`, work as `not_found`, and the order
//! as a `blocked` batch with a `stale_revision` member. The following owner
//! mutation proves that the refusals did not change its resident.
//!
//! Every action has its own durable operation id, and the actions that must be
//! *refused* have their own ids too: the owner records a committed operation id
//! and replays or refuses a repeat, so two actions that mean different things must
//! never share one. The fixture checks every answer against what the operation
//! implies - the request it named, whether a durable operation id is present, the
//! handle the snapshot names, the reason a refusal carries, the member state an
//! order batch reports - and only then reports `P3_RES <body>` to the player. An
//! answer that does not match is reported as
//! `P3_RES_UNEXPECTED <action> step <n>: <detail>` on the operator log and never
//! as a marker: the host drops the batch of a callback that answered with a
//! failure, so a diagnostic here can only be a log line.

use solaris_plugin_sdk::events::{
    CommandInvoked, Event, OperationAnswered, OperationOutcome, OperationPayload,
    PlayerTeleportAnswered, PlayerTeleportOutcome,
};
use solaris_plugin_sdk::inventories::{
    InventoryEndpoint, InventoryExpectedRevision, InventoryFence, InventoryResult,
    OwnedInventorySnapshot, OwnedItemTransfer,
};
use solaris_plugin_sdk::operation_types::OperationFailure;
use solaris_plugin_sdk::residents::{
    DemobilizeState, FollowOrder, Formation, FormationKind, HaulWork, Order, OrderMemberOutcome,
    OrderMemberState, ResidentKind, ResidentLifecycle, ResidentOrderResult, ResidentResult,
    ResidentSnapshot, WorkOrder, WorkPauseReason, WorkState,
};
use solaris_plugin_sdk::settlements::{
    SettlementResult, SitePoiKind, SitePoiState, SiteProvenance,
};
use solaris_plugin_sdk::{
    assign_resident_work, cancel_resident_order, cancel_resident_work, claim_resident,
    demobilize_resident, issue_resident_order, list_settlement_sites, log, message_player,
    query_owned_inventory, query_settlement_site, release_resident_site, reserve_resident_site,
    set_resident_pois, spawn_resident, transfer_owned_items, Command, Config, Failure, LogLevel,
};

/// The command root the owner package's manifest declares.
pub(crate) const ROOT: &str = "resident";

/// The command root the observer package's manifest declares. It is a different
/// root on purpose: two packages that declared the same player command would be
/// two owners of one name.
pub(crate) const FENCE_ROOT: &str = "resident-fence";

/// The configuration `mode` that selects this fixture.
const MODE: &str = "resident-operations";

/// The operator-ready line this fixture logs from `init`.
const READY: &str = "P3_RES ready";

/// The marker prefix every concluded step is reported under, and the prefix a step
/// that could not be concluded is reported under.
const MARKER: &str = "P3_RES";
const UNEXPECTED: &str = "P3_RES_UNEXPECTED";

/// The correlation ids this fixture names. Each step has its own, so an answer
/// that arrives for another step is a diagnostic rather than a step concluding
/// twice.
const SITE_REQUEST: &str = "res-site";
const REFRESH_REQUEST: &str = "res-refresh";
const RESERVE_REQUEST: &str = "res-reserve";
const SPAWN_REQUEST: &str = "res-spawn";
const RELEASE_REQUEST: &str = "res-release";
const GOTO_REQUEST: &str = "res-goto";
const POIS_REQUEST: &str = "res-pois";
const CLAIM_REQUEST: &str = "res-claim";
const PLAYER_REQUEST: &str = "res-player";
const CARRY_REQUEST: &str = "res-carry";
const TRANSFER_REQUEST: &str = "res-withdraw";
const WORK_REQUEST: &str = "res-work";
const CANCEL_WORK_REQUEST: &str = "res-cancel-work";
const ORDER_REQUEST: &str = "res-order";
const CANCEL_ORDER_REQUEST: &str = "res-cancel-order";
const GEAR_REQUEST: &str = "res-gear";
const DEMOBILIZE_REQUEST: &str = "res-demob";

/// The request-id prefix core's own scheduled work resumption answers under.
/// It is not one of this fixture's correlation ids: the storage actor resumes
/// paused work when a causal event removes its pause reason, and the receipt
/// proves the durable record revision the following steps must fence on.
const NATIVE_RESUME_PREFIX: &str = "native-resume-";
const FENCE_POIS_REQUEST: &str = "res-fence-pois";
const FENCE_CLAIM_REQUEST: &str = "res-fence-claim";
const FENCE_WORK_REQUEST: &str = "res-fence-work";
const FENCE_ORDER_REQUEST: &str = "res-fence-order";

/// The durable operation ids this fixture names: one per action, so an action that
/// must be refused cannot replay another action's committed receipt.
const RESERVE_OPERATION: &str = "res-op-reserve";
const SPAWN_OPERATION: &str = "res-op-spawn";
const RELEASE_OPERATION: &str = "res-op-release";
const POIS_OPERATION: &str = "res-op-pois";
const STALE_POIS_OPERATION: &str = "res-op-pois-stale";
const CLAIM_OPERATION: &str = "res-op-claim";
const STALE_CLAIM_OPERATION: &str = "res-op-claim-stale";
const TRANSFER_OPERATION: &str = "res-op-withdraw";
const MISSING_WORK_OPERATION: &str = "res-op-work-missing";
const WORK_OPERATION: &str = "res-op-work";
const STALE_WORK_OPERATION: &str = "res-op-work-stale";
const CANCEL_WORK_OPERATION: &str = "res-op-cancel-work";
const ORDER_OPERATION: &str = "res-op-order";
const STALE_ORDER_OPERATION: &str = "res-op-order-stale";
const CANCEL_ORDER_OPERATION: &str = "res-op-cancel-order";
const DEMOBILIZE_OPERATION: &str = "res-op-demob";
const FENCE_POIS_OPERATION: &str = "res-op-fence-pois";
const FENCE_CLAIM_OPERATION: &str = "res-op-fence-claim";
const FENCE_WORK_OPERATION: &str = "res-op-fence-work";
const FENCE_ORDER_OPERATION: &str = "res-op-fence-order";

/// The item the withdrawal moves, how many of it, and the work units one haul may
/// commit. One of each, so the assignment commits exactly one move and the answer's
/// real changes are one item either way.
const EMERALD: &str = "minecraft:emerald";
const ONE_ITEM: u32 = 1;
const ONE_UNIT: u64 = 1;

/// The formation a follow order names: the server's own closed vocabulary with a
/// spacing of two blocks (four half-blocks).
const FORMATION_SPACING: u8 = 4;

/// The points of interest the owner binds by default. They are the fixture's own
/// opaque handles - the resident owner records the strings it is given - and the
/// second binding the test drives replaces two of them so a test reads a real
/// change rather than the same values twice.
const HOME_POI: &str = "res-home";
const WORK_POI: &str = "res-work";
const MEETING_POI: &str = "res-meeting";

/// The value an argument takes to mean "no POI": the contract carries an option,
/// and this fixture's own argument vocabulary spells the absent one out.
const ABSENT: &str = "none";

/// How many sites one listing asks for: the largest page the contract allows, the
/// same bound the sibling settlement fixture asks for. Which cell of the profile's
/// grid carries the nearest authored candidate is a pure function of the profile
/// revision, so how far a listing has to walk before it names one is not something
/// this fixture may assume.
const SITE_PAGE: u8 = 64;

/// The role one deployment of this component runs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Owner,
    Observer,
}

/// One action the owner package answers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Site,
    Goto,
    Spawn,
    Pois,
    StalePois,
    Claim,
    StaleClaim,
    Withdraw,
    MissingWork,
    Work,
    StaleWork,
    CancelWork,
    Order,
    StaleOrder,
    CancelOrder,
    Gear,
    Demobilize,
}

impl Action {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "site" => Self::Site,
            "goto" => Self::Goto,
            "spawn" => Self::Spawn,
            "pois" => Self::Pois,
            "stale-pois" => Self::StalePois,
            "claim" => Self::Claim,
            "stale-claim" => Self::StaleClaim,
            "withdraw" => Self::Withdraw,
            "missing-work" => Self::MissingWork,
            "work" => Self::Work,
            "stale-work" => Self::StaleWork,
            "cancel-work" => Self::CancelWork,
            "order" => Self::Order,
            "stale-order" => Self::StaleOrder,
            "cancel-order" => Self::CancelOrder,
            "gear" => Self::Gear,
            "demobilize" => Self::Demobilize,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::Site => "site",
            Self::Goto => "goto",
            Self::Spawn => "spawn",
            Self::Pois => "pois",
            Self::StalePois => "stale-pois",
            Self::Claim => "claim",
            Self::StaleClaim => "stale-claim",
            Self::Withdraw => "withdraw",
            Self::MissingWork => "missing-work",
            Self::Work => "work",
            Self::StaleWork => "stale-work",
            Self::CancelWork => "cancel-work",
            Self::Order => "order",
            Self::StaleOrder => "stale-order",
            Self::CancelOrder => "cancel-order",
            Self::Gear => "gear",
            Self::Demobilize => "demobilize",
        }
    }

    /// The durable operation prefix for this action's distinct explicit intents.
    fn operation_prefix(self) -> &'static str {
        match self {
            Self::Site | Self::Goto | Self::Gear | Self::Spawn => "",
            Self::Pois => POIS_OPERATION,
            Self::StalePois => STALE_POIS_OPERATION,
            Self::Claim => CLAIM_OPERATION,
            Self::StaleClaim => STALE_CLAIM_OPERATION,
            Self::Withdraw => TRANSFER_OPERATION,
            Self::MissingWork => MISSING_WORK_OPERATION,
            Self::Work => WORK_OPERATION,
            Self::StaleWork => STALE_WORK_OPERATION,
            Self::CancelWork => CANCEL_WORK_OPERATION,
            Self::Order => ORDER_OPERATION,
            Self::StaleOrder => STALE_ORDER_OPERATION,
            Self::CancelOrder => CANCEL_ORDER_OPERATION,
            Self::Demobilize => DEMOBILIZE_OPERATION,
        }
    }
}

/// One shape the observer package asks for another plugin's resident in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fence {
    Pois,
    Claim,
    Work,
    Order,
}

impl Fence {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "pois" => Self::Pois,
            "claim" => Self::Claim,
            "work" => Self::Work,
            "order" => Self::Order,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::Pois => "pois",
            Self::Claim => "claim",
            Self::Work => "work",
            Self::Order => "order",
        }
    }

    fn request(self) -> &'static str {
        match self {
            Self::Pois => FENCE_POIS_REQUEST,
            Self::Claim => FENCE_CLAIM_REQUEST,
            Self::Work => FENCE_WORK_REQUEST,
            Self::Order => FENCE_ORDER_REQUEST,
        }
    }

    fn operation(self) -> &'static str {
        match self {
            Self::Pois => FENCE_POIS_OPERATION,
            Self::Claim => FENCE_CLAIM_OPERATION,
            Self::Work => FENCE_WORK_OPERATION,
            Self::Order => FENCE_ORDER_OPERATION,
        }
    }
}

/// One site the fixture found a free home point of interest in: the server's own
/// ids, revision and cells, kept exactly as the answer carried them.
struct SiteState {
    site_id: String,
    poi_id: String,
    x: i32,
    y: i32,
    z: i32,
    revision: u64,
}

/// Where one action stands.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Idle,
    /// A site listing is outstanding.
    Site,
    /// A re-read of the site a reserve is about to name is outstanding.
    Refresh,
    /// A site reservation is outstanding.
    Reserve,
    /// A resident spawn is outstanding.
    Spawn,
    /// A reservation hand-back is outstanding, after a refused spawn.
    Release,
    /// A teleport is outstanding. It is answered by its own event, not by an
    /// operation answer.
    Teleport,
    /// A point-of-interest binding is outstanding.
    Pois,
    /// A resident claim is outstanding.
    Claim,
    /// The actor's own inventory is being read, for the withdrawal's source.
    Player,
    /// The worker's carry is being read, for the withdrawal's destination.
    Carry,
    /// The withdrawal itself is outstanding.
    Transfer,
    /// A work assignment is outstanding.
    Work,
    /// A work cancellation is outstanding.
    CancelWork,
    /// A squad order is outstanding.
    Order,
    /// A squad order cancellation is outstanding.
    CancelOrder,
    /// The worker's equipment is being read.
    Gear,
    /// A demobilisation is outstanding.
    Demobilize,
    /// The observer's own one-shot request is outstanding.
    Foreign(Fence),
}

impl Step {
    fn number(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Site => 1,
            Self::Refresh => 2,
            Self::Reserve => 3,
            Self::Spawn => 4,
            Self::Release => 5,
            Self::Teleport => 6,
            Self::Pois => 7,
            Self::Claim => 8,
            Self::Player => 9,
            Self::Carry => 10,
            Self::Transfer => 11,
            Self::Work => 12,
            Self::CancelWork => 13,
            Self::Order => 14,
            Self::CancelOrder => 15,
            Self::Gear => 16,
            Self::Demobilize => 17,
            Self::Foreign(_) => 18,
        }
    }

    /// The correlation id this step's answer must name, or nothing when the step
    /// is a teleport, whose answer is its own event.
    fn request(self) -> Option<&'static str> {
        Some(match self {
            Self::Idle | Self::Teleport => return None,
            Self::Site => SITE_REQUEST,
            Self::Refresh => REFRESH_REQUEST,
            Self::Reserve => RESERVE_REQUEST,
            Self::Spawn => SPAWN_REQUEST,
            Self::Release => RELEASE_REQUEST,
            Self::Pois => POIS_REQUEST,
            Self::Claim => CLAIM_REQUEST,
            Self::Player => PLAYER_REQUEST,
            Self::Carry => CARRY_REQUEST,
            Self::Transfer => TRANSFER_REQUEST,
            Self::Work => WORK_REQUEST,
            Self::CancelWork => CANCEL_WORK_REQUEST,
            Self::Order => ORDER_REQUEST,
            Self::CancelOrder => CANCEL_ORDER_REQUEST,
            Self::Gear => GEAR_REQUEST,
            Self::Demobilize => DEMOBILIZE_REQUEST,
            Self::Foreign(shape) => shape.request(),
        })
    }
}

/// The fixture itself: the action in flight and everything the answers named.
pub struct Fixture {
    role: Role,
    /// The connection the action in flight runs on, from the command that began it.
    session: u64,
    /// The player the marker goes to.
    reporter: String,
    action: Option<Action>,
    /// The observer's one-shot shape, when this deployment is the observer.
    fence: Option<Fence>,
    step: Step,
    /// The candidate the current action walks to and reserves: a free home point
    /// of interest of one authored site, kept exactly as the listing carried it,
    /// with the revision the reserve re-reads from the site before it names it.
    site: Option<SiteState>,
    /// Every candidate the last listing named, and which of them the fixture is
    /// on. A cell that cannot hold a standing body is that candidate's own
    /// refusal, so the next attempt moves down this list instead of asking for the
    /// same cell again. The revision it carries is the one that listing answered;
    /// the reserve re-reads the site rather than trusting it.
    candidates: Vec<SiteState>,
    index: usize,
    /// The resident handle the server minted, once a spawn committed.
    handle: Option<String>,
    /// The server's own canonical uuid of the villager that spawn materialised,
    /// kept exactly as the snapshot carried it: the identity a claim names.
    entity: Option<String>,
    /// The reservation waiting to be consumed or handed back.
    reservation: Option<String>,
    /// Explicit commands are distinct durable intents, even when they use the
    /// same operation family with a newer fence or different points of interest.
    operation_sequence: u64,
    operation_id: String,
    /// The revision of the last resident snapshot this fixture read: the fence a
    /// point-of-interest binding sends.
    resident_revision: u64,
    /// The revision that preceded the first committed binding, which is what
    /// `stale-pois` sends.
    stale_resident_revision: Option<u64>,
    /// The revision of the resident's own work and order record, from the last
    /// answer that carried one, and the one that preceded it.
    record_revision: u64,
    stale_record_revision: Option<u64>,
    /// Whether core's own resumption receipt arrived. A haul it already
    /// performed replays as a finished watermark without a new change, so the
    /// explicit assignment that follows must accept that.
    resumed: bool,
    order_revision: u64,
    stale_order_revision: Option<u64>,
    /// The actor's own inventory read for the withdrawal: the emerald's slot and
    /// the fence that read answered.
    player_slot: Option<u8>,
    player_fence: Option<InventoryFence>,
    /// The worker's carry read for the withdrawal: the free slot it will take and
    /// the fence that read answered.
    carry_slot: Option<u8>,
    carry_fence: Option<InventoryFence>,
    /// The refusal a step concluded with, reported once the hand-back it needs has
    /// answered too.
    pending: Option<String>,
}

impl Fixture {
    /// Read this deployment's role and check the mode.
    ///
    /// The fixture is only the `resident-operations` one, so a configuration that
    /// says something else is this package being wired to the wrong mode, which is
    /// refused here rather than answered by running the wrong fixture. The observer
    /// is the same mode with `role = "observer"`: the same contract and the same
    /// fixture, with the one shape of its request differing.
    pub fn configure(config: &Config) -> Result<Self, Failure> {
        let toml = config.toml();
        let mode = toml
            .as_ref()
            .and_then(|value| value.get("mode").and_then(|mode| mode.as_str()));
        if mode != Some(MODE) {
            return Err(Failure::Invalid);
        }
        let role = match toml
            .as_ref()
            .and_then(|value| value.get("role").and_then(|role| role.as_str()))
        {
            None | Some("owner") => Role::Owner,
            Some("observer") => Role::Observer,
            Some(_) => return Err(Failure::Invalid),
        };
        Ok(Self {
            role,
            session: 0,
            reporter: String::new(),
            action: None,
            fence: None,
            step: Step::Idle,
            site: None,
            candidates: Vec::new(),
            index: 0,
            handle: None,
            entity: None,
            reservation: None,
            operation_sequence: 0,
            operation_id: String::new(),
            resident_revision: 0,
            stale_resident_revision: None,
            record_revision: 0,
            stale_record_revision: None,
            resumed: false,
            order_revision: 0,
            stale_order_revision: None,
            player_slot: None,
            player_fence: None,
            carry_slot: None,
            carry_fence: None,
            pending: None,
        })
    }

    /// The startup phase: nothing to stage, one readiness line for a driver.
    pub fn init(&mut self) -> Result<Vec<Command>, Failure> {
        log(LogLevel::Info, READY);
        Ok(Vec::new())
    }

    /// One delivered batch: the command root a player ran, at most one answer to
    /// this fixture's outstanding request, and the teleport answer `goto` waits
    /// for.
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
            match event {
                Event::CommandInvoked(invoked) => {
                    if invoked.name == ROOT && self.role == Role::Owner {
                        commands.extend(self.begin(invoked)?);
                    } else if invoked.name == FENCE_ROOT && self.role == Role::Observer {
                        commands.extend(self.begin_fence(invoked)?);
                    }
                }
                // The answer to the teleport `goto` asked for. The owner may
                // refuse it, because a teleport is an effect on one live
                // connection and a plugin never learns that a connection has gone
                // from anything else.
                Event::PlayerTeleportAnswered(answered) => {
                    if self.step == Step::Teleport {
                        commands.extend(self.teleported(answered)?);
                    }
                }
                _ => {}
            }
        }
        Ok(commands)
    }

    /// The teleport answer `goto` waits for.
    fn teleported(&mut self, answered: &PlayerTeleportAnswered) -> Result<Vec<Command>, Failure> {
        if answered.request != GOTO_REQUEST {
            return Err(self.unexpected("a teleport was answered that this fixture never asked"));
        }
        self.step = Step::Idle;
        self.action = None;
        let outcome = match &answered.outcome {
            PlayerTeleportOutcome::Committed => "committed",
            PlayerTeleportOutcome::Refused(_) => "refused",
        };
        let position = &answered.position;
        Ok(vec![self.report(&format!(
            "goto {outcome} at {}/{}/{}",
            position.x, position.y, position.z
        ))])
    }

    /// One owner command: the action it names, and the request that starts it.
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
                "the resident root was asked for without one of its actions: {:?}",
                invoked.arguments
            )));
        };
        self.session = invoked.session;
        self.reporter = invoked.player.clone();
        self.action = Some(action);
        self.operation_sequence += 1;
        let prefix = action.operation_prefix();
        self.operation_id.clear();
        if !prefix.is_empty() {
            std::fmt::write(
                &mut self.operation_id,
                format_args!("{prefix}-{}", self.operation_sequence),
            )
            .expect("format the resident intent id");
        }
        match action {
            Action::Site => {
                self.step = Step::Site;
                Ok(vec![list_settlement_sites(SITE_REQUEST, None, SITE_PAGE)])
            }
            Action::Goto => {
                let Some(site) = &self.site else {
                    return Err(self.reject("no site was listed to walk to"));
                };
                self.step = Step::Teleport;
                let (x, y, z) = (site.x, site.y, site.z);
                Ok(vec![teleport_player(
                    GOTO_REQUEST,
                    self.session,
                    f64::from(x) + 0.5,
                    f64::from(y),
                    f64::from(z) + 0.5,
                )])
            }
            Action::Spawn => self.spawn(),
            Action::Pois | Action::StalePois => {
                let (home, work, meeting) = binding_arguments(invoked.arguments.get(1..))?;
                self.pois(action, home.as_deref(), work.as_deref(), meeting.as_deref())
            }
            Action::Claim | Action::StaleClaim => self.claim(action),
            Action::Withdraw => {
                self.step = Step::Player;
                Ok(vec![query_owned_inventory(
                    PLAYER_REQUEST,
                    InventoryEndpoint::PlayerInventory(self.session),
                    None,
                )])
            }
            Action::MissingWork | Action::Work | Action::StaleWork => self.work(action),
            Action::CancelWork => {
                let Some(handle) = self.handle.clone() else {
                    return Err(self.reject("no resident was spawned to cancel work for"));
                };
                self.step = Step::CancelWork;
                Ok(vec![cancel_resident_work(
                    CANCEL_WORK_REQUEST,
                    &self.operation_id,
                    &handle,
                    self.record_revision,
                )])
            }
            Action::Order | Action::StaleOrder => {
                let Some(handle) = self.handle.clone() else {
                    return Err(self.reject("no resident was spawned to order"));
                };
                let revision = if action == Action::StaleOrder {
                    let Some(stale) = self.stale_order_revision else {
                        return Err(self.reject("no committed order revision to send stale"));
                    };
                    stale
                } else {
                    self.order_revision
                };
                self.step = Step::Order;
                Ok(vec![issue_resident_order(
                    ORDER_REQUEST,
                    &self.operation_id,
                    vec![handle],
                    vec![revision],
                    Order::Follow(FollowOrder {
                        target_player: self.session,
                        formation: Formation {
                            kind: FormationKind::Line,
                            spacing: FORMATION_SPACING,
                        },
                    }),
                )])
            }
            Action::CancelOrder => {
                let Some(handle) = self.handle.clone() else {
                    return Err(self.reject("no resident was spawned to cancel an order for"));
                };
                self.step = Step::CancelOrder;
                Ok(vec![cancel_resident_order(
                    CANCEL_ORDER_REQUEST,
                    &self.operation_id,
                    vec![handle],
                    vec![self.order_revision],
                )])
            }
            Action::Gear => {
                self.step = Step::Gear;
                Ok(vec![query_owned_inventory(
                    GEAR_REQUEST,
                    self.gear_endpoint()?,
                    None,
                )])
            }
            Action::Demobilize => {
                let Some(handle) = self.handle.clone() else {
                    return Err(self.reject("no resident was spawned to demobilise"));
                };
                self.step = Step::Demobilize;
                Ok(vec![demobilize_resident(
                    DEMOBILIZE_REQUEST,
                    &self.operation_id,
                    &handle,
                    self.record_revision,
                )])
            }
        }
    }

    /// One observer command: the handle or entity uuid to ask about and the shape
    /// to ask in.
    fn begin_fence(&mut self, invoked: &CommandInvoked) -> Result<Vec<Command>, Failure> {
        if self.fence.is_some() {
            return Err(
                self.unexpected("an observer request was asked for while one was in flight")
            );
        }
        let Some(target) = invoked.arguments.first().cloned() else {
            return Err(self.reject("the observer root was asked for without a handle"));
        };
        let shape = match invoked.arguments.get(1) {
            None => Fence::Pois,
            Some(name) => Fence::from_name(name).ok_or_else(|| {
                self.reject("the observer root was asked for in an unknown shape")
            })?,
        };
        self.session = invoked.session;
        self.reporter = invoked.player.clone();
        self.fence = Some(shape);
        self.step = Step::Foreign(shape);
        Ok(vec![match shape {
            Fence::Pois => set_resident_pois(
                shape.request(),
                shape.operation(),
                &target,
                Some(HOME_POI),
                None,
                None,
                0,
            ),
            Fence::Claim => {
                // Use the requested entity revision without replacing it with a
                // deliberately stale fence: the driver probes actual ownership.
                let Some(revision) = invoked
                    .arguments
                    .get(2)
                    .and_then(|value| value.parse().ok())
                else {
                    return Err(
                        self.reject("the observer root was asked for a claim without a revision")
                    );
                };
                claim_resident(
                    shape.request(),
                    shape.operation(),
                    self.session,
                    &target,
                    revision,
                )
            }
            Fence::Work => assign_resident_work(
                shape.request(),
                shape.operation(),
                &target,
                WorkOrder::Haul(HaulWork {
                    source: InventoryEndpoint::ResidentCarry(target.clone()),
                    destination: InventoryEndpoint::ResidentEquipment(target.clone()),
                    item: Some(EMERALD.to_owned()),
                }),
                ONE_UNIT,
                0,
            ),
            Fence::Order => {
                cancel_resident_order(shape.request(), shape.operation(), vec![target], vec![0])
            }
        }])
    }

    /// The spawn action: re-read the site, reserve the candidate, or spawn against
    /// the reservation already held.
    ///
    /// A reserve names the revision this fixture re-reads from the site in the same
    /// action: the owner stamps every reservation decision onto the site's own
    /// revision - including the hand-back a refused spawn owes - so the revision an
    /// earlier listing answered is not the one a later or retried reserve must send.
    /// A spawn that consumes a reservation carries no revision and needs no read.
    fn spawn(&mut self) -> Result<Vec<Command>, Failure> {
        if let Some(token) = self.reservation.clone() {
            self.step = Step::Spawn;
            return Ok(vec![spawn_resident(
                SPAWN_REQUEST,
                &format!("{SPAWN_OPERATION}-{}", self.operation_sequence),
                &token,
                ResidentKind::Villager,
            )]);
        }
        let Some(site) = &self.site else {
            return Err(self.reject("no site was listed to reserve"));
        };
        let site_id = site.site_id.clone();
        self.step = Step::Refresh;
        Ok(vec![query_settlement_site(
            REFRESH_REQUEST,
            &site_id,
            None,
            SITE_PAGE,
        )])
    }

    /// One point-of-interest binding at the revision this action must send.
    fn pois(
        &mut self,
        action: Action,
        home: Option<&str>,
        work: Option<&str>,
        meeting: Option<&str>,
    ) -> Result<Vec<Command>, Failure> {
        let Some(handle) = self.handle.clone() else {
            return Err(self.reject("no resident was spawned to bind points of interest for"));
        };
        let revision = if action == Action::StalePois {
            let Some(stale) = self.stale_resident_revision else {
                return Err(self.reject("no committed binding revision to send stale"));
            };
            stale
        } else {
            self.resident_revision
        };
        self.step = Step::Pois;
        Ok(vec![set_resident_pois(
            POIS_REQUEST,
            &self.operation_id,
            &handle,
            home,
            work,
            meeting,
            revision,
        )])
    }

    /// One claim of the villager this plugin's own spawn materialised.
    ///
    /// The claim names the entity uuid the spawn answered - the server's stable
    /// identity for that villager, never one this fixture derived - and the actor
    /// is the session the command arrived on. `expected_entity_revision` is the
    /// resident revision the last committed binding answered, which is the only
    /// revision the owner accepts for an entity this plugin already holds;
    /// `stale-claim` sends the revision that preceded that binding instead.
    fn claim(&mut self, action: Action) -> Result<Vec<Command>, Failure> {
        let Some(entity) = self.entity.clone() else {
            return Err(self.reject("no resident was spawned to claim"));
        };
        if self.handle.is_none() {
            return Err(self.reject("no resident was spawned to claim"));
        }
        let revision = if action == Action::StaleClaim {
            let Some(stale) = self.stale_resident_revision else {
                return Err(self.reject("no committed resident revision to send stale"));
            };
            stale
        } else {
            self.resident_revision
        };
        self.step = Step::Claim;
        Ok(vec![claim_resident(
            CLAIM_REQUEST,
            &self.operation_id,
            self.session,
            &entity,
            revision,
        )])
    }

    /// A work assignment of one haul of the emerald out of the worker's carry and
    /// into its equipment.
    fn work(&mut self, action: Action) -> Result<Vec<Command>, Failure> {
        let Some(handle) = self.handle.clone() else {
            return Err(self.reject("no resident was spawned to assign work to"));
        };
        let revision = if action == Action::StaleWork {
            let Some(stale) = self.stale_record_revision else {
                return Err(self.reject("no committed work revision to send stale"));
            };
            stale
        } else {
            self.record_revision
        };
        self.step = Step::Work;
        Ok(vec![assign_resident_work(
            WORK_REQUEST,
            &self.operation_id,
            &handle,
            WorkOrder::Haul(HaulWork {
                source: InventoryEndpoint::ResidentCarry(handle.clone()),
                destination: InventoryEndpoint::ResidentEquipment(handle.clone()),
                item: Some(EMERALD.to_owned()),
            }),
            ONE_UNIT,
            revision,
        )])
    }

    /// One answer to this fixture's outstanding request.
    fn answer(&mut self, event: &Event) -> Result<Vec<Command>, Failure> {
        let Event::OperationAnswered(answered) = event else {
            return Err(self.unexpected("an event that is not an operation answer"));
        };
        if answered.request.starts_with(NATIVE_RESUME_PREFIX) {
            return self.native_resumed(answered);
        }
        let Some(expected) = self.step.request() else {
            return Err(self.unexpected("an answer arrived while nothing expected one"));
        };
        if answered.request != expected {
            return Err(self.unexpected(&format!(
                "the answer named {}, not {expected}",
                answered.request
            )));
        }
        match self.step {
            Step::Site => {
                let commands = self.conclude_site(answered)?;
                self.action = None;
                self.step = Step::Idle;
                Ok(commands)
            }
            Step::Refresh => {
                if answered.operation_id.is_some() {
                    return Err(self.unexpected("the site re-read carried a durable operation id"));
                }
                let SettlementResult::Site(site) = self.committed_settlement(answered)? else {
                    return Err(self.unexpected("the site re-read answered something else"));
                };
                let Some(held) = self.site.as_mut() else {
                    return Err(self.reject("no site was listed to reserve"));
                };
                if site.site_id != held.site_id {
                    return Err(self.unexpected("the site re-read answered another site"));
                }
                // The revision this read answered is the fence the reservation
                // must name: a query changes nothing, so it is the site's own.
                held.revision = site.revision;
                let (site_id, poi_id, revision) =
                    (held.site_id.clone(), held.poi_id.clone(), held.revision);
                self.step = Step::Reserve;
                Ok(vec![reserve_resident_site(
                    RESERVE_REQUEST,
                    &format!("{RESERVE_OPERATION}-{}", self.operation_sequence),
                    &site_id,
                    &poi_id,
                    revision,
                )])
            }
            Step::Reserve => {
                let token = self.reserved(answered)?;
                self.reservation = Some(token.clone());
                self.step = Step::Spawn;
                Ok(vec![spawn_resident(
                    SPAWN_REQUEST,
                    &format!("{SPAWN_OPERATION}-{}", self.operation_sequence),
                    &token,
                    ResidentKind::Villager,
                )])
            }
            Step::Spawn => {
                if let OperationOutcome::Refused(refused) = &answered.outcome {
                    // A refusal that touched nothing is one the reservation
                    // outlives, so the fixture hands it back exactly as the
                    // shipped settlement plugin does and reports the reason once
                    // the owner has answered the hand-back.
                    let Some(token) = self.reservation.clone() else {
                        return Err(
                            self.unexpected("a refused spawn left no reservation to release")
                        );
                    };
                    self.pending = Some(format!("spawn refused {}", failure_name(&refused.reason)));
                    if refused.reason == OperationFailure::Blocked {
                        // The world refused this cell, not the reservation: the
                        // next attempt reserves the next free home point of
                        // interest the listing named.
                        self.index += 1;
                    }
                    self.step = Step::Release;
                    return Ok(vec![release_resident_site(
                        RELEASE_REQUEST,
                        &format!("{RELEASE_OPERATION}-{}", self.operation_sequence),
                        &token,
                    )]);
                }
                let commands = self.conclude_spawn(answered)?;
                self.reservation = None;
                self.action = None;
                self.step = Step::Idle;
                Ok(commands)
            }
            Step::Release => {
                if matches!(answered.outcome, OperationOutcome::Refused(_)) {
                    return Err(self.unexpected("the reservation hand-back was refused"));
                }
                self.reservation = None;
                self.action = None;
                self.step = Step::Idle;
                let Some(body) = self.pending.take() else {
                    return Err(self.unexpected("a hand-back concluded without a refusal"));
                };
                Ok(vec![self.report(&body)])
            }
            Step::Player => {
                let endpoint = InventoryEndpoint::PlayerInventory(self.session);
                let snapshot = self.snapshot(answered, &endpoint)?;
                self.player_slot = first_slot_of(&snapshot, EMERALD);
                self.player_fence = Some(snapshot.fence.clone());
                let Some(handle) = self.handle.clone() else {
                    return Err(self.reject("no resident was spawned to withdraw into"));
                };
                self.step = Step::Carry;
                Ok(vec![query_owned_inventory(
                    CARRY_REQUEST,
                    InventoryEndpoint::ResidentCarry(handle),
                    None,
                )])
            }
            Step::Carry => {
                let Some(handle) = self.handle.clone() else {
                    return Err(self.reject("no resident was spawned to withdraw into"));
                };
                let carry = InventoryEndpoint::ResidentCarry(handle);
                let snapshot = self.snapshot(answered, &carry)?;
                self.carry_slot = first_empty_slot(&snapshot);
                self.carry_fence = Some(snapshot.fence.clone());
                let (Some(source_slot), Some(destination_slot)) =
                    (self.player_slot, self.carry_slot)
                else {
                    return Err(
                        self.unexpected("the withdrawal lost the slot one of the reads named")
                    );
                };
                let (Some(source), Some(destination)) =
                    (self.player_fence.clone(), self.carry_fence.clone())
                else {
                    return Err(self.unexpected("the withdrawal lost a fence it read"));
                };
                let source_endpoint = InventoryEndpoint::PlayerInventory(self.session);
                self.step = Step::Transfer;
                Ok(vec![transfer_owned_items(
                    TRANSFER_REQUEST,
                    &self.operation_id,
                    self.session,
                    vec![OwnedItemTransfer {
                        source: source_endpoint.clone(),
                        source_slot,
                        destination: carry.clone(),
                        destination_slot,
                        count: ONE_ITEM,
                    }],
                    // One fence per endpoint, in the server's own endpoint order:
                    // a player's inventory sorts before a resident's carry.
                    vec![
                        InventoryExpectedRevision {
                            endpoint: source_endpoint,
                            fence: source,
                        },
                        InventoryExpectedRevision {
                            endpoint: carry,
                            fence: destination,
                        },
                    ],
                )])
            }
            _ => {
                let commands = self.conclude(answered)?;
                self.action = None;
                self.fence = None;
                self.step = Step::Idle;
                Ok(commands)
            }
        }
    }

    /// One core-scheduled resumption receipt, outside this fixture's
    /// one-question protocol. Adopt the revision the assignment proves so the
    /// next step fences on the record core actually holds, and log the receipt
    /// without concluding a step.
    fn native_resumed(&mut self, answered: &OperationAnswered) -> Result<Vec<Command>, Failure> {
        let committed = match &answered.outcome {
            OperationOutcome::Committed(committed) => committed,
            OperationOutcome::Refused(_) => {
                // A refused resumption changed nothing this fixture fences on.
                log(LogLevel::Info, "native resume receipt was refused");
                return Ok(Vec::new());
            }
        };
        let OperationPayload::ResidentOrder(ResidentOrderResult::Work(assignment)) =
            &committed.payload
        else {
            return Err(self.unexpected("the resumption receipt answered something else"));
        };
        if Some(assignment.handle.as_str()) != self.handle.as_deref() {
            return Err(self.unexpected("the resumption receipt names another resident"));
        }
        self.record_revision = assignment.revision;
        self.resumed = true;
        log(
            LogLevel::Info,
            &format!(
                "native resume revision={} units={}/{}",
                assignment.revision, assignment.work_units_done, assignment.work_units_planned
            ),
        );
        Ok(Vec::new())
    }

    /// One site listing: the candidate this attempt walks to and reserves.
    ///
    /// Only an authored site can be reserved - the owner lays it out from the
    /// catalog it deployed - so a generated village's points of interest are not
    /// candidates here. The first listing keeps every free home point of interest
    /// it names, in the listing's own order; a later call answers the next one,
    /// which is how a driver moves on after a cell the world refuses.
    fn conclude_site(&mut self, answered: &OperationAnswered) -> Result<Vec<Command>, Failure> {
        if answered.operation_id.is_some() {
            return Err(self.unexpected("a site listing carried a durable operation id"));
        }
        let SettlementResult::Sites(page) = self.committed_settlement(answered)? else {
            return Err(self.unexpected("the listing answered something that is not a page"));
        };
        if self.candidates.is_empty() {
            for site in &page.sites {
                for poi in &site.pois {
                    if site.provenance == SiteProvenance::Authored
                        && poi.kind == SitePoiKind::Home
                        && poi.state == SitePoiState::Free
                    {
                        self.candidates.push(SiteState {
                            site_id: site.site_id.clone(),
                            poi_id: poi.poi_id.clone(),
                            x: poi.at.x,
                            y: poi.at.y,
                            z: poi.at.z,
                            revision: site.revision,
                        });
                    }
                }
            }
        }
        let Some(candidate) = self.candidates.get(self.index) else {
            let (index, candidates) = (self.index, self.candidates.len());
            self.site = None;
            return Ok(vec![self.report(&format!(
                "site exhausted index={index} candidates={candidates}"
            ))]);
        };
        let (site_id, poi_id, x, y, z, revision) = (
            candidate.site_id.clone(),
            candidate.poi_id.clone(),
            candidate.x,
            candidate.y,
            candidate.z,
            candidate.revision,
        );
        let body = format!(
            "site {site_id} {poi_id} {x}/{y}/{z} revision={revision} index={} candidates={}",
            self.index,
            self.candidates.len()
        );
        self.site = Some(SiteState {
            site_id,
            poi_id,
            x,
            y,
            z,
            revision,
        });
        Ok(vec![self.report(&body)])
    }

    /// The token a reservation answered with, checked against the site it was
    /// asked about.
    fn reserved(&mut self, answered: &OperationAnswered) -> Result<String, Failure> {
        let SettlementResult::ResidentSite(reservation) = self.committed_settlement(answered)?
        else {
            return Err(self.unexpected("the reservation answered something else"));
        };
        let Some(site) = &self.site else {
            return Err(self.reject("a reservation answered with no site asked about"));
        };
        if reservation.site_id != site.site_id || reservation.poi_id != site.poi_id {
            return Err(self.unexpected("the reservation names another site or point of interest"));
        }
        Ok(reservation.spawn_site_token)
    }

    /// One committed spawn: the resident the server minted, with the handle, the
    /// entity uuid and the revision every later call names.
    fn conclude_spawn(&mut self, answered: &OperationAnswered) -> Result<Vec<Command>, Failure> {
        self.expect_operation(
            answered,
            &format!("{SPAWN_OPERATION}-{}", self.operation_sequence),
        )?;
        let snapshot = self.committed_resident(answered)?;
        let handle = snapshot.handle.clone();
        let entity = snapshot.entity_uuid.clone();
        let lifecycle = lifecycle_name(snapshot.lifecycle);
        let revision = snapshot.revision;
        if entity.is_empty() {
            return Err(self.unexpected("the spawned resident carries no entity"));
        }
        self.handle = Some(handle.clone());
        self.entity = Some(entity.clone());
        self.resident_revision = revision;
        Ok(vec![self.report(&format!(
            "spawn {handle} {entity} {lifecycle} revision={revision}"
        ))])
    }

    /// Every other committed or refused answer.
    fn conclude(&mut self, answered: &OperationAnswered) -> Result<Vec<Command>, Failure> {
        if let Some(shape) = self.fence {
            return self.conclude_fence(shape, answered);
        }
        let Some(action) = self.action else {
            return Err(self.unexpected("an answer concluded without an action"));
        };
        match action {
            Action::Pois | Action::StalePois => self.conclude_pois(action, answered),
            Action::Claim | Action::StaleClaim => self.conclude_claim(action, answered),
            Action::Withdraw => {
                self.expect_operation(answered, &self.operation_id)?;
                let OperationOutcome::Committed(committed) = &answered.outcome else {
                    return Err(self.unexpected("the withdrawal was refused"));
                };
                let OperationPayload::OwnedInventory(InventoryResult::Transfer(inventories)) =
                    &committed.payload
                else {
                    return Err(self.unexpected("the withdrawal answered something else"));
                };
                if inventories.len() != 2 {
                    return Err(self.unexpected("the withdrawal named another set of endpoints"));
                }
                if self.player_slot.is_none() || self.carry_slot.is_none() {
                    return Err(self.unexpected("the withdrawal lost a slot it read"));
                }
                let carry = inventories
                    .iter()
                    .find(|inventory| {
                        matches!(
                            &inventory.endpoint,
                            InventoryEndpoint::ResidentCarry(handle)
                                if Some(handle.as_str()) == self.handle.as_deref()
                        )
                    })
                    .ok_or_else(|| {
                        self.unexpected("the withdrawal omitted the resident carry fence")
                    })?;
                // Carry and work share the resident's durable order record.
                // The transfer advanced that record, not just the player's
                // fence, and core's native resume may already have advanced
                // it further: adopt whichever revision is newest so the next
                // step never fences on a revision a receipt has superseded.
                self.record_revision = self.record_revision.max(carry.fence.revision);
                Ok(vec![self.report(&format!(
                    "withdraw moved={ONE_ITEM} endpoints={} revision={}",
                    inventories.len(),
                    committed.revision
                ))])
            }
            Action::MissingWork | Action::Work | Action::StaleWork => {
                self.conclude_work(action, answered)
            }
            Action::CancelWork => {
                self.expect_operation(answered, &self.operation_id)?;
                let OperationOutcome::Committed(committed) = &answered.outcome else {
                    return Err(self.unexpected("the work cancellation was refused"));
                };
                let OperationPayload::ResidentOrder(ResidentOrderResult::WorkCancelled(cancelled)) =
                    &committed.payload
                else {
                    return Err(self.unexpected("the work cancellation answered something else"));
                };
                if Some(cancelled.handle.as_str()) != self.handle.as_deref() {
                    return Err(self.unexpected("the cancellation names another resident"));
                }
                self.record_revision = cancelled.revision;
                Ok(vec![self.report(&format!(
                    "cancel-work {} revision={}",
                    cancelled.handle, cancelled.revision
                ))])
            }
            Action::Order => self.conclude_order(answered),
            Action::StaleOrder => self.conclude_stale_order(answered),
            Action::CancelOrder => {
                self.expect_operation(answered, &self.operation_id)?;
                let OperationOutcome::Committed(committed) = &answered.outcome else {
                    return Err(self.unexpected("the order cancellation was refused"));
                };
                let OperationPayload::ResidentOrder(ResidentOrderResult::OrderCancelled(cancelled)) =
                    &committed.payload
                else {
                    return Err(self.unexpected("the cancellation answered something else"));
                };
                let member = self.own_member(&cancelled.members)?.clone();
                self.record_revision = committed.revision;
                self.order_revision = 0;
                Ok(vec![self.report(&format!(
                    "cancel-order order-revision={} state={} slot={}",
                    cancelled.order_revision,
                    member_state_name(member.state),
                    member
                        .formation_slot
                        .map_or(ABSENT.to_owned(), |slot| slot.to_string())
                ))])
            }
            Action::Gear => {
                let endpoint = self.gear_endpoint()?;
                let snapshot = self.snapshot(answered, &endpoint)?;
                let mut carried = Vec::new();
                for slot in &snapshot.slots {
                    let Some(item) = &slot.item else { continue };
                    carried.push(format!("{}:{}", item.resource_id, item.count));
                }
                carried.sort_unstable();
                Ok(vec![self.report(&format!(
                    "gear {} revision={}",
                    if carried.is_empty() {
                        ABSENT.to_owned()
                    } else {
                        carried.join(",")
                    },
                    snapshot.fence.revision
                ))])
            }
            Action::Demobilize => {
                self.expect_operation(answered, &self.operation_id)?;
                let OperationOutcome::Committed(committed) = &answered.outcome else {
                    return Err(self.unexpected("the demobilisation was refused"));
                };
                let OperationPayload::ResidentOrder(ResidentOrderResult::Demobilized(resident)) =
                    &committed.payload
                else {
                    return Err(self.unexpected("the demobilisation answered something else"));
                };
                if Some(resident.handle.as_str()) != self.handle.as_deref() {
                    return Err(self.unexpected("the demobilisation names another resident"));
                }
                self.record_revision = resident.revision;
                Ok(vec![self.report(&format!(
                    "demobilize {} {} reason={} returned={} revision={}",
                    resident.handle,
                    demobilize_state_name(resident.state),
                    resident
                        .reason
                        .map_or(ABSENT.to_owned(), |reason| pause_reason_name(reason)),
                    resident.returned.len(),
                    resident.revision
                ))])
            }
            Action::Site | Action::Goto | Action::Spawn => {
                Err(self.unexpected("an action concluded in a step that answers earlier"))
            }
        }
    }

    /// One point-of-interest binding.
    fn conclude_pois(
        &mut self,
        action: Action,
        answered: &OperationAnswered,
    ) -> Result<Vec<Command>, Failure> {
        self.expect_operation(answered, &self.operation_id)?;
        if action == Action::StalePois {
            // The refusal leaves the revision this fixture read untouched, which is
            // why the next binding sends it again unchanged.
            return self.expect_refusal(action, answered, OperationFailure::StaleRevision);
        }
        let snapshot = self.committed_resident(answered)?;
        if self.stale_resident_revision.is_none() {
            self.stale_resident_revision = Some(self.resident_revision);
        }
        self.resident_revision = snapshot.revision;
        Ok(vec![self.report(&format!(
            "pois {}:{}:{} revision={}",
            snapshot.pois.home.as_deref().unwrap_or(ABSENT),
            snapshot.pois.work.as_deref().unwrap_or(ABSENT),
            snapshot.pois.meeting.as_deref().unwrap_or(ABSENT),
            snapshot.revision
        ))])
    }

    /// One claim answer: the resident this plugin's own spawn materialised, checked
    /// against the handle and the entity the fixture holds, or the refusal a
    /// revision that is not the record's own answers with.
    fn conclude_claim(
        &mut self,
        action: Action,
        answered: &OperationAnswered,
    ) -> Result<Vec<Command>, Failure> {
        self.expect_operation(answered, &self.operation_id)?;
        if action == Action::StaleClaim {
            return self.expect_refusal(action, answered, OperationFailure::StaleRevision);
        }
        let snapshot = self.committed_resident(answered)?;
        let (Some(handle), Some(entity)) = (self.handle.as_deref(), self.entity.as_deref()) else {
            return Err(self.reject("no resident was spawned to claim"));
        };
        if snapshot.handle.as_str() != handle {
            return Err(self.unexpected("the claim answered a resident this plugin does not hold"));
        }
        if snapshot.entity_uuid.as_str() != entity {
            return Err(self.unexpected("the claim answered another entity than the one claimed"));
        }
        // The answer is the resident as the server now holds it, and the revision it
        // names is the receipt this claim was recorded under. The fence this fixture
        // sends for a record mutation stays the revision the last committed binding
        // answered, which the binding it drives after the claim refusals proves by
        // committing at it.
        Ok(vec![self.report(&format!(
            "claim {handle} {entity} {} revision={}",
            lifecycle_name(snapshot.lifecycle),
            snapshot.revision
        ))])
    }

    /// One work assignment the owner executes.
    fn conclude_work(
        &mut self,
        action: Action,
        answered: &OperationAnswered,
    ) -> Result<Vec<Command>, Failure> {
        self.expect_operation(answered, &self.operation_id)?;
        if action == Action::StaleWork {
            return self.expect_refusal(action, answered, OperationFailure::StaleRevision);
        }
        let committed = match &answered.outcome {
            OperationOutcome::Committed(committed) => committed,
            OperationOutcome::Refused(refused) => {
                return Err(self.unexpected(&format!(
                    "the work assignment refused {}",
                    failure_name(&refused.reason)
                )));
            }
        };
        let OperationPayload::ResidentOrder(ResidentOrderResult::Work(assignment)) =
            &committed.payload
        else {
            return Err(self.unexpected("the work assignment answered something else"));
        };
        if Some(assignment.handle.as_str()) != self.handle.as_deref() {
            return Err(self.unexpected("the assignment names another resident"));
        }
        match action {
            Action::MissingWork => {
                if assignment.state != WorkState::Paused
                    || assignment.reason != Some(WorkPauseReason::MissingInput)
                {
                    return Err(self.unexpected(
                        "a worker with nothing to move did not pause with missing_input",
                    ));
                }
                if !assignment.changes.is_empty() {
                    return Err(self.unexpected("a worker with nothing to move reported changes"));
                }
            }
            _ => {
                if assignment.state != WorkState::Committed {
                    return Err(self.unexpected("the assignment did not commit"));
                }
                // Core's own resumption may already have performed this haul
                // between the pause and this explicit assignment: the replay
                // then reports the finished watermark, not a new change.
                if assignment.changes.is_empty() && !self.resumed {
                    return Err(self.unexpected("a committed haul reported no item change"));
                }
                if assignment.work_units_done != assignment.work_units_planned {
                    return Err(self.unexpected("a committed assignment did not finish its units"));
                }
            }
        }
        if self.stale_record_revision.is_none() {
            self.stale_record_revision = Some(self.record_revision);
        }
        self.record_revision = assignment.revision;
        let mut changes = Vec::new();
        for change in &assignment.changes {
            changes.push(format!(
                "{}:{}{}",
                change.item_id,
                if change.delta > 0 { "+" } else { "" },
                change.delta
            ));
        }
        changes.sort_unstable();
        Ok(vec![self.report(&format!(
            "{} {} reason={} units={}/{} changes={} revision={}",
            action.name(),
            state_name(assignment.state),
            assignment
                .reason
                .map_or(ABSENT.to_owned(), |reason| pause_reason_name(reason)),
            assignment.work_units_done,
            assignment.work_units_planned,
            if changes.is_empty() {
                ABSENT.to_owned()
            } else {
                changes.join(",")
            },
            assignment.revision
        ))])
    }

    /// One committed squad order.
    fn conclude_order(&mut self, answered: &OperationAnswered) -> Result<Vec<Command>, Failure> {
        self.expect_operation(answered, &self.operation_id)?;
        let OperationOutcome::Committed(committed) = &answered.outcome else {
            return Err(self.unexpected("the order was refused"));
        };
        let OperationPayload::ResidentOrder(ResidentOrderResult::Order(order)) = &committed.payload
        else {
            return Err(self.unexpected("the order answered something else"));
        };
        let member = self.own_member(&order.members)?.clone();
        // A follow order for a live actor is applied with a formation slot: the
        // engine gives the member a goal and a slot whether or not the actor
        // entity is currently readable.
        if member.state != OrderMemberState::Applied || member.formation_slot.is_none() {
            return Err(self.unexpected("the follow order was not applied to its member"));
        }
        if self.stale_order_revision.is_none() {
            self.stale_order_revision = Some(self.order_revision);
        }
        self.order_revision = order.order_revision;
        self.record_revision = committed.revision;
        Ok(vec![self.report(&format!(
            "order order-revision={} state={} slot={} targets={} combat={}",
            order.order_revision,
            member_state_name(member.state),
            member
                .formation_slot
                .map_or(ABSENT.to_owned(), |slot| slot.to_string()),
            member.targets.len(),
            order.combat.len()
        ))])
    }

    /// One order issued at a revision that already moved: the owner's own batch
    /// refusal, whose payload still names every member.
    fn conclude_stale_order(
        &mut self,
        answered: &OperationAnswered,
    ) -> Result<Vec<Command>, Failure> {
        self.expect_operation(answered, &self.operation_id)?;
        let OperationOutcome::Refused(refused) = &answered.outcome else {
            return Err(self.unexpected("a stale order revision committed"));
        };
        if refused.reason != OperationFailure::Blocked {
            return Err(self.unexpected(&format!(
                "the stale order was refused {} instead of blocked",
                failure_name(&refused.reason)
            )));
        }
        let OperationPayload::ResidentOrder(ResidentOrderResult::Order(order)) = &refused.payload
        else {
            return Err(self.unexpected("the refused order carried no member detail"));
        };
        let member = self.own_member(&order.members)?;
        if member.state != OrderMemberState::StaleRevision {
            return Err(self.unexpected("the refused order named a member that is not stale"));
        }
        Ok(vec![self.report(&format!(
            "stale-order blocked state={} members={}",
            member_state_name(member.state),
            order.members.len()
        ))])
    }

    /// The observer's own refusal: another plugin's resident is not this plugin's
    /// to change, and the owners say so in their own vocabulary.
    fn conclude_fence(
        &self,
        shape: Fence,
        answered: &OperationAnswered,
    ) -> Result<Vec<Command>, Failure> {
        self.expect_operation(answered, shape.operation())?;
        let OperationOutcome::Refused(refused) = &answered.outcome else {
            return Err(self.unexpected(&format!("the foreign {} committed", shape.name())));
        };
        // The shapes are refused by the owners' own reasons: a
        // point-of-interest call the resident owner forbids, a work call it does
        // not find among this plugin's own residents, an order batch it refuses
        // wholesale with one stale member per handle, and a foreign entity claim.
        let expected = match shape {
            Fence::Pois => OperationFailure::Forbidden,
            Fence::Claim => OperationFailure::Forbidden,
            Fence::Work => OperationFailure::NotFound,
            Fence::Order => OperationFailure::Blocked,
        };
        if refused.reason != expected {
            return Err(self.unexpected(&format!(
                "the foreign {} was refused {} instead of {}",
                shape.name(),
                failure_name(&refused.reason),
                failure_name(&expected)
            )));
        }
        let detail = match shape {
            Fence::Order => {
                let OperationPayload::ResidentOrder(ResidentOrderResult::Order(order)) =
                    &refused.payload
                else {
                    return Err(self.unexpected("the foreign order carried no member detail"));
                };
                if order.members.len() != 1
                    || order.members[0].state != OrderMemberState::StaleRevision
                {
                    return Err(self.unexpected("the foreign order named no stale member"));
                }
                format!(" members={} state=stale-revision", order.members.len())
            }
            _ => String::new(),
        };
        Ok(vec![self.report(&format!(
            "foreign-{} refused {}{detail}",
            shape.name(),
            failure_name(&refused.reason)
        ))])
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

    /// Check one mutation answer carries the durable operation id it was sent
    /// under. A query carries none, which the callers that read one check
    /// themselves.
    fn expect_operation(
        &self,
        answered: &OperationAnswered,
        expected: &str,
    ) -> Result<(), Failure> {
        if answered.operation_id.as_deref() == Some(expected) {
            return Ok(());
        }
        Err(self.unexpected(&format!(
            "the answer named operation {:?} instead of {expected}",
            answered.operation_id
        )))
    }

    /// The resident one committed answer carried, checked against the handle this
    /// fixture holds.
    fn committed_resident(
        &self,
        answered: &OperationAnswered,
    ) -> Result<ResidentSnapshot, Failure> {
        let committed = match &answered.outcome {
            OperationOutcome::Committed(committed) => committed,
            OperationOutcome::Refused(refused) => {
                return Err(self.unexpected(&format!(
                    "the resident call refused {}",
                    failure_name(&refused.reason)
                )));
            }
        };
        let OperationPayload::Resident(ResidentResult::Snapshot(snapshot)) = &committed.payload
        else {
            return Err(self.unexpected("the answer carried no resident snapshot"));
        };
        if snapshot.handle.is_empty() || ResidentLifecycle::Released == snapshot.lifecycle {
            return Err(self.unexpected("the answer carried a resident with no live identity"));
        }
        Ok(snapshot.clone())
    }

    /// One committed settlement result.
    fn committed_settlement(
        &self,
        answered: &OperationAnswered,
    ) -> Result<SettlementResult, Failure> {
        let committed = match &answered.outcome {
            OperationOutcome::Committed(committed) => committed,
            OperationOutcome::Refused(refused) => {
                return Err(self.unexpected(&format!(
                    "the settlement call refused {}",
                    failure_name(&refused.reason)
                )));
            }
        };
        let OperationPayload::Settlement(result) = &committed.payload else {
            return Err(self.unexpected("the settlement call carried no settlement result"));
        };
        Ok(result.clone())
    }

    /// One committed inventory snapshot of the endpoint it was asked about.
    fn snapshot(
        &self,
        answered: &OperationAnswered,
        endpoint: &InventoryEndpoint,
    ) -> Result<OwnedInventorySnapshot, Failure> {
        if answered.operation_id.is_some() {
            return Err(self.unexpected("an inventory read carried a durable operation id"));
        }
        let OperationOutcome::Committed(committed) = &answered.outcome else {
            return Err(self.unexpected("the inventory read refused"));
        };
        let OperationPayload::OwnedInventory(InventoryResult::Snapshot(snapshot)) =
            &committed.payload
        else {
            return Err(self.unexpected("the inventory read answered something else"));
        };
        if !same_endpoint(&snapshot.endpoint, endpoint) {
            return Err(self.unexpected("the snapshot names another endpoint than the one asked"));
        }
        Ok(snapshot.clone())
    }

    /// This fixture's own member of one order answer.
    fn own_member<'a>(
        &self,
        members: &'a [OrderMemberOutcome],
    ) -> Result<&'a OrderMemberOutcome, Failure> {
        let Some(handle) = self.handle.as_deref() else {
            return Err(self.reject("no resident was spawned to order"));
        };
        if members.len() != 1 || members[0].handle != handle {
            return Err(self.unexpected("the order answered members this fixture did not name"));
        }
        Ok(&members[0])
    }

    /// The equipment endpoint of this fixture's resident.
    fn gear_endpoint(&self) -> Result<InventoryEndpoint, Failure> {
        let Some(handle) = self.handle.clone() else {
            return Err(self.reject("no resident was spawned to read gear for"));
        };
        Ok(InventoryEndpoint::ResidentEquipment(handle))
    }

    /// One marker line to the player who asked.
    fn report(&self, body: &str) -> Command {
        message_player(&self.reporter, format!("{MARKER} {body}"))
    }

    /// Log one step this fixture could not conclude, and fail the callback with it
    /// so the batch it staged is dropped.
    fn unexpected(&self, detail: &str) -> Failure {
        let action = self
            .action
            .map_or_else(|| self.fence.map_or("none", Fence::name), Action::name);
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

/// The three points of interest one binding command named, or the fixture's own
/// defaults when it named none. `ABSENT` spells the option's absent case out.
fn binding_arguments(
    arguments: Option<&[String]>,
) -> Result<(Option<String>, Option<String>, Option<String>), Failure> {
    let Some(arguments) = arguments else {
        return Ok((
            Some(HOME_POI.to_owned()),
            Some(WORK_POI.to_owned()),
            Some(MEETING_POI.to_owned()),
        ));
    };
    let named: Vec<Option<String>> = arguments
        .iter()
        .map(|value| (value != ABSENT).then(|| value.clone()))
        .collect();
    match named.as_slice() {
        [] => Ok((
            Some(HOME_POI.to_owned()),
            Some(WORK_POI.to_owned()),
            Some(MEETING_POI.to_owned()),
        )),
        [home, work, meeting] => Ok((home.clone(), work.clone(), meeting.clone())),
        _ => Err(Failure::Invalid),
    }
}

/// One authoritative teleport of one live connection.
///
/// The SDK wraps every command this fixture sends but this one, so the record is
/// built here from the contract's own types: the guest still sends exactly what
/// `commands.wit` declares and nothing here can drift from it.
#[must_use]
fn teleport_player(request: &str, session: u64, x: f64, y: f64, z: f64) -> Command {
    Command::TeleportPlayer(solaris_plugin_sdk::commands::TeleportPlayer {
        request: request.to_owned(),
        session,
        position: solaris_plugin_sdk::types::Position { x, y, z },
    })
}

/// The first slot of one snapshot that holds at least one of `item`.
fn first_slot_of(snapshot: &OwnedInventorySnapshot, item: &str) -> Option<u8> {
    snapshot
        .slots
        .iter()
        .find(|slot| {
            slot.item
                .as_ref()
                .is_some_and(|held| held.resource_id == item && held.count >= ONE_ITEM)
        })
        .map(|slot| slot.slot)
}

/// The first empty slot of one snapshot, which is where a withdrawal puts what it
/// takes.
fn first_empty_slot(snapshot: &OwnedInventorySnapshot) -> Option<u8> {
    snapshot
        .slots
        .iter()
        .find(|slot| slot.item.is_none())
        .map(|slot| slot.slot)
}

/// Whether two endpoints name the same container.
///
/// The generated types carry no `PartialEq`, so the comparison is written out:
/// this contract compares an endpoint's whole identity - its case and its value.
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

/// Whether one event is the host answering a request this fixture made.
fn is_answer(event: &Event) -> bool {
    matches!(event, Event::OperationAnswered(_))
}

/// How one resident lifecycle is named in a marker: the contract's own vocabulary.
fn lifecycle_name(lifecycle: ResidentLifecycle) -> &'static str {
    match lifecycle {
        ResidentLifecycle::AliveLoaded => "alive-loaded",
        ResidentLifecycle::AliveUnloaded => "alive-unloaded",
        ResidentLifecycle::Dead => "dead",
        ResidentLifecycle::Released => "released",
    }
}

/// How one work state is named in a marker.
fn state_name(state: WorkState) -> &'static str {
    match state {
        WorkState::Accepted => "accepted",
        WorkState::Running => "running",
        WorkState::Paused => "paused",
        WorkState::Committed => "committed",
        WorkState::Cancelled => "cancelled",
    }
}

/// How one member state is named in a marker.
fn member_state_name(state: OrderMemberState) -> &'static str {
    match state {
        OrderMemberState::Applied => "applied",
        OrderMemberState::BlockedRoute => "blocked-route",
        OrderMemberState::Unloaded => "unloaded",
        OrderMemberState::Dead => "dead",
        OrderMemberState::Migrating => "migrating",
        OrderMemberState::Forbidden => "forbidden",
        OrderMemberState::StaleRevision => "stale-revision",
    }
}

/// How one demobilisation state is named in a marker.
fn demobilize_state_name(state: DemobilizeState) -> &'static str {
    match state {
        DemobilizeState::Demobilizing => "demobilizing",
        DemobilizeState::Civilian => "civilian",
    }
}

/// How one pause reason is named in a marker.
fn pause_reason_name(reason: WorkPauseReason) -> String {
    match reason {
        WorkPauseReason::Unloaded => "unloaded",
        WorkPauseReason::NoWorkers => "no-workers",
        WorkPauseReason::MissingInput => "missing-input",
        WorkPauseReason::MissingTool => "missing-tool",
        WorkPauseReason::MissingStation => "missing-station",
        WorkPauseReason::BlockedRoute => "blocked-route",
        WorkPauseReason::Interrupted => "interrupted",
        WorkPauseReason::Protected => "protected",
        WorkPauseReason::Unsupported => "unsupported",
        WorkPauseReason::NoStorage => "no-storage",
    }
    .to_owned()
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
