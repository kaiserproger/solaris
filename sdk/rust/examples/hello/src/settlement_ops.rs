//! P3 settlement acceptance fixture: one settlement call per driven step.
//!
//! `mode = "settlement-operations"` runs this module. A driver runs the package's
//! `settle` root once per step, with the step's name as its only argument, and each
//! one asks the server's own settlement owner for exactly one thing:
//!
//! - `list` reads the first page of sites and remembers the first site's identity,
//!   its durable revision, its footprint and one free home point of interest. Its
//!   marker carries those values because a driver has no other way to learn where
//!   the site is: the fixture copies them from the owner's own answer and adds
//!   nothing.
//! - `query` re-reads that site's points of interest, and `query-stale` re-reads
//!   them with a cursor the site does not hold, which the owner refuses with its own
//!   `cursor-expired`.
//! - `reserve` reserves the remembered home point of interest under one durable
//!   operation id; `reserve-replay` submits byte-identical content under the same
//!   id, which must answer the reservation the first call recorded rather than
//!   minting a second one.
//! - `release` hands that reservation back, and `release-again` hands it back a
//!   second time under a different id, which is the owner's `not-found` rather than
//!   a second effect.
//! - `refresh` re-reads the site so the fixture holds the revision the release
//!   left - the fence a later plan must name.
//! - `survey` surveys a bounded region around the site's own footprint and
//!   remembers the token the owner minted.
//! - `survey-far` surveys a region the world has never generated and remembers
//!   nothing: the owner answers its own `unloaded`, which is what keeps a survey of
//!   a generated region an observation rather than a formality.
//! - `prepare-stale` plans the structure with a revision the site has moved past,
//!   which the owner refuses `stale-revision`; `prepare` plans it with the revision
//!   `refresh` read and remembers the structure the owner minted.
//! - `status` reads that structure back, `advance` advances it with a reservation
//!   the owner does not hold (its own `not-found`), `pause` pauses it, `bind` asks
//!   for an authored container the world has not materialized, and `cancel`
//!   cancels it. After that `bind-missing` asks for an id the owner never held and
//!   `bind-cancelled` asks for the cancelled structure's own container, which the
//!   owner refuses because it is no longer placed.
//! - `inventory` reads the invoking player's own canonical inventory, which is the
//!   endpoint a structure's materials are held against; `reserve-materials` holds
//!   the structure's own plan - the very plan the owner fenced with
//!   `resource-plan-hash` in the snapshot, one work portion per authored stage -
//!   against the fence that read answered, and remembers the reservation the owner
//!   minted. `advance-materials` then advances the structure against that
//!   reservation, which is the owner's own funding rule: the portion it commits is
//!   charged to the plan the reservation holds.
//! - `bind-container` asks for the same authored container `bind` asks for, once
//!   the world holds one at that structure's placed position, and reports the
//!   binding the owner minted.
//!
//! One request is outstanding at a time, and a step answers with no commands at all,
//! so a driver decides when the next one runs and can change the world in between -
//! which is what the survey step needs, because a region whose chunks the world has
//! not generated is the owner's `unloaded` and nothing else, and what the funded
//! advance needs, because the materials it spends have to be in the player's own
//! inventory before the reservation names them.
//!
//! Every step reports the outcome the owner gave it, never the outcome the fixture
//! hoped for: a committed answer logs `P3_SETTLE <step> ...` with the values the
//! answer carries, and a refused one logs `P3_SETTLE <step>-refused <reason>` with
//! the owner's own reason name. A step whose answer is a different shape than the
//! one it asked for is reported as `P3_SETTLE_UNEXPECTED <step>: <detail>` on the
//! operator log instead of a marker: the host drops the batch of a callback that
//! answered with a failure, so a diagnostic here is a log line and cannot be a chat
//! line.

use solaris_plugin_sdk::events::{
    CommandInvoked, Event, OperationAnswered, OperationFailure, OperationOutcome, OperationPayload,
};
use solaris_plugin_sdk::inventories::{
    InventoryEndpoint, InventoryFence, InventoryMaterial, InventoryReservationQuantity,
    InventoryReservationSnapshot, InventoryResourcePlan, InventoryResult, InventoryWorkPortion,
};
use solaris_plugin_sdk::settlements::{
    BlockPosition, ChunkAvailability, ResidentSiteReservation, SettlementResult, SettlementSite,
    SitePoiKind, SitePoiState, SiteProvenance, StructureMaterial, StructureSnapshot,
    StructureState, SurveyBounds, SurveyPurpose,
};
use solaris_plugin_sdk::{
    advance_structure, bind_warehouse, cancel_structure, list_settlement_sites, log,
    pause_structure, prepare_structure, query_owned_inventory, query_settlement_site,
    release_resident_site, reserve_inventory_items, reserve_resident_site, structure_status,
    survey_site, Command, Config, Failure, LogLevel,
};

/// The command root the package's manifest declares for this fixture.
pub(crate) const ROOT: &str = "settle";

/// The dimension the profile owns: the only one its survey answers for.
const SURVEY_DIMENSION: &str = "minecraft:overworld";
/// How many sites one `list` asks for, and how many points of interest one
/// `query` asks for: the contract's own largest page.
const PAGE: u8 = 64;
/// How many columns the survey covers per horizontal axis.
const SURVEY_SPAN: i32 = 31;
/// The contract's own largest axis extent: the survey's vertical span.
const SURVEY_DEPTH: i32 = 127;
/// How far from the site the un-generated probe region sits, in blocks: far enough
/// that no view distance this world is opened with has published its chunks.
const SURVEY_FAR_SHIFT: i32 = 4_096;
/// The world's highest block coordinate, so a survey never names a bound past it.
const WORLD_MAX_Y: i32 = 319;
/// How far above the site's own base row the structure is planned.
///
/// The owner only admits a structure where nothing rises above its base row, and a
/// site's own base row follows the terrain it was laid out on; planning this far
/// above it keeps the fixture's anchor out of the terrain without touching the
/// owner's rule.
const ANCHOR_RISE: i32 = 64;
/// The structure is planned unrotated: the catalog's own authored facing.
const ROTATION: u16 = 0;
/// How much work one advance authorizes.
///
/// A portion is a prefix of the stage's own cells, and one commit may only span
/// one of the world's mutation regions, so the smallest portion the contract
/// admits is the one that never has to be split.
const WORK_UNITS: u64 = 1;
/// The authored container ordinal the bind asks for: the first `empty_container`
/// seed of the catalog's blueprint.
const CONTAINER: u32 = 0;
/// The reservation reference `advance` names, which no inventory reservation holds
/// - so the owner's own `not-found` is the answer this fixture observes.
const NO_RESERVATION: &str = "settle-no-reservation";
/// The structure id `bind-missing` names, which the owner does not hold.
const NO_STRUCTURE: &str = "no-such-structure";
/// A point-of-interest cursor no site holds.
const NO_CURSOR: &str = "no-such-poi";
/// The request ids the inventory steps correlate their answers by.
const INVENTORY_REQUEST: &str = "settle-inventory";
const MATERIALS_REQUEST: &str = "settle-materials";
const ADVANCE_MATERIALS_REQUEST: &str = "settle-advance-materials";
/// The durable operation ids this fixture names, one per mutation. A replay
/// deliberately repeats one; nothing else reuses an id.
const RESERVE_ID: &str = "settle-reserve";
const RELEASE_ID: &str = "settle-release";
const RELEASE_AGAIN_ID: &str = "settle-release-again";
const PREPARE_ID: &str = "settle-prepare";
const PREPARE_STALE_ID: &str = "settle-prepare-stale";
const ADVANCE_ID: &str = "settle-advance";
const ADVANCE_MATERIALS_ID: &str = "settle-advance-materials";
/// The durable id the materials reservation itself is recorded under.
const MATERIALS_ID: &str = "settle-materials";
const PAUSE_ID: &str = "settle-pause";
const CANCEL_ID: &str = "settle-cancel";
const BIND_ID: &str = "settle-bind";
const BIND_CONTAINER_ID: &str = "settle-bind-container";
const BIND_MISSING_ID: &str = "settle-bind-missing";
const BIND_CANCELLED_ID: &str = "settle-bind-cancelled";

/// One step a driver can run. Each is one request, and each answers with its own
/// marker.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    List,
    Query,
    QueryStale,
    Reserve,
    ReserveReplay,
    Release,
    ReleaseAgain,
    Refresh,
    Survey,
    SurveyFar,
    PrepareStale,
    Prepare,
    Status,
    Inventory,
    ReserveMaterials,
    Advance,
    AdvanceMaterials,
    Pause,
    Cancel,
    Bind,
    BindContainer,
    BindMissing,
    BindCancelled,
    Done,
}

impl Step {
    /// The step one argument names, or nothing for an argument this fixture does
    /// not serve.
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "list" => Self::List,
            "query" => Self::Query,
            "query-stale" => Self::QueryStale,
            "reserve" => Self::Reserve,
            "reserve-replay" => Self::ReserveReplay,
            "release" => Self::Release,
            "release-again" => Self::ReleaseAgain,
            "refresh" => Self::Refresh,
            "survey" => Self::Survey,
            "survey-far" => Self::SurveyFar,
            "prepare-stale" => Self::PrepareStale,
            "prepare" => Self::Prepare,
            "status" => Self::Status,
            "inventory" => Self::Inventory,
            "reserve-materials" => Self::ReserveMaterials,
            "advance" => Self::Advance,
            "advance-materials" => Self::AdvanceMaterials,
            "pause" => Self::Pause,
            "cancel" => Self::Cancel,
            "bind" => Self::Bind,
            "bind-container" => Self::BindContainer,
            "bind-missing" => Self::BindMissing,
            "bind-cancelled" => Self::BindCancelled,
            "done" => Self::Done,
            _ => return None,
        })
    }

    /// How this step is named in its marker.
    fn name(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Query => "query",
            Self::QueryStale => "query-stale",
            Self::Reserve => "reserve",
            Self::ReserveReplay => "reserve-replay",
            Self::Release => "release",
            Self::ReleaseAgain => "release-again",
            Self::Refresh => "refresh",
            Self::Survey => "survey",
            Self::SurveyFar => "survey-far",
            Self::PrepareStale => "prepare-stale",
            Self::Prepare => "prepare",
            Self::Status => "status",
            Self::Inventory => "inventory",
            Self::ReserveMaterials => "reserve-materials",
            Self::Advance => "advance",
            Self::AdvanceMaterials => "advance-materials",
            Self::Pause => "pause",
            Self::Cancel => "cancel",
            Self::Bind => "bind",
            Self::BindContainer => "bind-container",
            Self::BindMissing => "bind-missing",
            Self::BindCancelled => "bind-cancelled",
            Self::Done => "done",
        }
    }
}

/// One site the fixture read, reduced to what a later step needs.
struct Site {
    id: String,
    /// The durable revision the owner reported for the site's reservation ledger;
    /// zero until the site holds one.
    revision: u64,
    origin: BlockPosition,
    /// The point of interest a reserve names.
    home: String,
}

/// The fixture itself: what it read from the owner, and the one request it waits
/// on.
pub struct Fixture {
    /// The catalog blueprint `prepare` plans, which the package's own configuration
    /// names: the fixture cannot enumerate the deployment's catalog.
    blueprint: String,
    site: Option<Site>,
    /// The revision the last reserve sent, kept because a replay must submit
    /// byte-identical content.
    reserved_at: Option<u64>,
    /// The token the survey minted, or the reserve recorded.
    token: Option<String>,
    /// The structure `prepare` minted.
    structure: Option<String>,
    /// The structure's own revision, as the owner last reported it.
    revision: u64,
    /// The first stage of the prepared structure, which `advance` names.
    stage: Option<String>,
    /// The player the last command was invoked for: the endpoint a materials
    /// reservation is held against.
    session: u64,
    /// The structure's own material plan, built from the stages the owner
    /// answered, and the plan hash the owner fences that plan with.
    plan: Option<(InventoryResourcePlan, String)>,
    /// The player inventory fence `reserve-materials` names.
    fence: Option<InventoryFence>,
    /// The reservation `reserve-materials` minted, which `advance-materials`
    /// spends from.
    reservation: Option<String>,
    /// The step whose answer this instance waits for.
    pending: Option<Step>,
}

impl Fixture {
    /// The fixture `config.toml` configures: the blueprint `prepare` plans.
    pub fn configure(config: &Config) -> Result<Self, Failure> {
        let blueprint = config
            .toml()
            .and_then(|value| {
                value
                    .get("blueprint")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned)
            })
            .ok_or(Failure::Failed)?;
        Ok(Self {
            blueprint,
            site: None,
            reserved_at: None,
            token: None,
            structure: None,
            revision: 0,
            stage: None,
            session: 0,
            plan: None,
            fence: None,
            reservation: None,
            pending: None,
        })
    }

    /// Nothing is asked before a driver runs a step: every step is driven by the
    /// package's own command, so a driver decides the order and can change the
    /// world between two of them.
    pub fn init(&mut self) -> Result<Vec<Command>, Failure> {
        Ok(Vec::new())
    }

    /// The commands one delivered batch answers with: one step's request for the
    /// driver's command, or the reading of one answer.
    pub fn on_events(&mut self, events: &[Event]) -> Result<Vec<Command>, Failure> {
        let mut commands = Vec::new();
        for event in events {
            match event {
                Event::CommandInvoked(invoked) if invoked.name == ROOT => {
                    if let Some(command) = self.run(invoked) {
                        commands.push(command);
                    }
                }
                Event::OperationAnswered(answer) => self.answered(answer),
                _ => {}
            }
        }
        Ok(commands)
    }

    /// Ask for one step, or report the step it cannot ask for.
    fn run(&mut self, invoked: &CommandInvoked) -> Option<Command> {
        let Some(name) = invoked.arguments.first() else {
            self.reject("no-step", "the settle root needs one step argument");
            return None;
        };
        let Some(step) = Step::parse(name) else {
            self.reject(name, "unknown step");
            return None;
        };
        if self.pending.is_some() {
            self.reject(step.name(), "one request is still outstanding");
            return None;
        }
        // Every command a step arrives with names the player whose own inventory
        // a materials reservation is held against, so the last invocation is the
        // endpoint the inventory steps address.
        self.session = invoked.session;
        if step == Step::Done {
            log(LogLevel::Info, "P3_SETTLE done");
            return None;
        }
        let command = match step {
            Step::List => list_settlement_sites("settle-list", None, PAGE),
            Step::Query | Step::Refresh => match self.site.as_ref() {
                Some(site) => query_settlement_site("settle-query", &site.id, None, PAGE),
                None => {
                    self.reject(step.name(), "no site was read yet");
                    return None;
                }
            },
            Step::QueryStale => match self.site.as_ref() {
                Some(site) => {
                    query_settlement_site("settle-query-stale", &site.id, Some(NO_CURSOR), PAGE)
                }
                None => {
                    self.reject(step.name(), "no site was read yet");
                    return None;
                }
            },
            Step::Reserve | Step::ReserveReplay => {
                let Some(site) = self.site.as_ref() else {
                    self.reject(step.name(), "no site was read yet");
                    return None;
                };
                // A replay must submit byte-identical content, so it names the
                // revision the first reserve sent rather than the one the answer
                // reported back.
                let expected = if step == Step::ReserveReplay {
                    let Some(expected) = self.reserved_at else {
                        self.reject(step.name(), "no reservation was sent yet");
                        return None;
                    };
                    expected
                } else {
                    self.reserved_at = Some(site.revision);
                    site.revision
                };
                let site_id = site.id.clone();
                let home = site.home.clone();
                reserve_resident_site("settle-reserve", RESERVE_ID, &site_id, &home, expected)
            }
            Step::Release | Step::ReleaseAgain => {
                let Some(token) = self.token.as_ref() else {
                    self.reject(step.name(), "no reservation is held");
                    return None;
                };
                let operation_id = if step == Step::Release {
                    RELEASE_ID
                } else {
                    RELEASE_AGAIN_ID
                };
                release_resident_site("settle-release", operation_id, token)
            }
            Step::Survey => match self.site.as_ref() {
                Some(site) => survey_site(
                    "settle-survey",
                    SURVEY_DIMENSION,
                    survey_bounds(site.origin),
                    SurveyPurpose::Settlement,
                ),
                None => {
                    self.reject(step.name(), "no site was read yet");
                    return None;
                }
            },
            Step::SurveyFar => match self.site.as_ref() {
                Some(site) => survey_site(
                    "settle-survey-far",
                    SURVEY_DIMENSION,
                    far_bounds(site.origin),
                    SurveyPurpose::Settlement,
                ),
                None => {
                    self.reject(step.name(), "no site was read yet");
                    return None;
                }
            },
            Step::Prepare | Step::PrepareStale => {
                let (Some(site), Some(token)) = (self.site.as_ref(), self.token.as_ref()) else {
                    self.reject(step.name(), "no survey token is held");
                    return None;
                };
                let (operation_id, expected) = if step == Step::PrepareStale {
                    (PREPARE_STALE_ID, 0)
                } else {
                    (PREPARE_ID, site.revision)
                };
                prepare_structure(
                    "settle-prepare",
                    operation_id,
                    &site.id,
                    &self.blueprint,
                    anchor(site.origin),
                    ROTATION,
                    token,
                    expected,
                    None,
                )
            }
            Step::Status => match self.structure.as_ref() {
                Some(structure) => structure_status("settle-status", structure),
                None => {
                    self.reject(step.name(), "no structure was planned yet");
                    return None;
                }
            },
            Step::Inventory => {
                query_owned_inventory(INVENTORY_REQUEST, player_endpoint(self.session), None)
            }
            Step::ReserveMaterials => {
                let (Some((plan, _)), Some(fence)) = (self.plan.as_ref(), self.fence.as_ref())
                else {
                    self.reject(step.name(), "no materials plan or inventory fence is held");
                    return None;
                };
                reserve_inventory_items(
                    MATERIALS_REQUEST,
                    MATERIALS_ID,
                    player_endpoint(self.session),
                    plan.clone(),
                    fence.clone(),
                )
            }
            Step::Advance => {
                let (Some(structure), Some(stage)) = (self.structure.as_ref(), self.stage.as_ref())
                else {
                    self.reject(step.name(), "no structure was planned yet");
                    return None;
                };
                advance_structure(
                    "settle-advance",
                    ADVANCE_ID,
                    structure,
                    stage,
                    NO_RESERVATION,
                    self.revision,
                    WORK_UNITS,
                )
            }
            Step::AdvanceMaterials => {
                let (Some(structure), Some(stage), Some(reservation)) = (
                    self.structure.as_ref(),
                    self.stage.as_ref(),
                    self.reservation.as_ref(),
                ) else {
                    self.reject(step.name(), "no structure or materials reservation is held");
                    return None;
                };
                advance_structure(
                    ADVANCE_MATERIALS_REQUEST,
                    ADVANCE_MATERIALS_ID,
                    structure,
                    stage,
                    reservation,
                    self.revision,
                    WORK_UNITS,
                )
            }
            Step::Pause => match self.structure.as_ref() {
                Some(structure) => {
                    pause_structure("settle-pause", PAUSE_ID, structure, self.revision)
                }
                None => {
                    self.reject(step.name(), "no structure was planned yet");
                    return None;
                }
            },
            Step::Cancel => match self.structure.as_ref() {
                Some(structure) => {
                    cancel_structure("settle-cancel", CANCEL_ID, structure, self.revision)
                }
                None => {
                    self.reject(step.name(), "no structure was planned yet");
                    return None;
                }
            },
            Step::Bind => match self.structure.as_ref() {
                Some(structure) => bind_warehouse("settle-bind", BIND_ID, structure, CONTAINER),
                None => {
                    self.reject(step.name(), "no structure was planned yet");
                    return None;
                }
            },
            Step::BindContainer => match self.structure.as_ref() {
                Some(structure) => {
                    bind_warehouse("settle-bind", BIND_CONTAINER_ID, structure, CONTAINER)
                }
                None => {
                    self.reject(step.name(), "no structure was planned yet");
                    return None;
                }
            },
            Step::BindMissing => {
                bind_warehouse("settle-bind", BIND_MISSING_ID, NO_STRUCTURE, CONTAINER)
            }
            Step::BindCancelled => match self.structure.as_ref() {
                Some(structure) => {
                    bind_warehouse("settle-bind", BIND_CANCELLED_ID, structure, CONTAINER)
                }
                None => {
                    self.reject(step.name(), "no structure was planned yet");
                    return None;
                }
            },
            Step::Done => unreachable!("the done step answers no command"),
        };
        self.pending = Some(step);
        Some(command)
    }

    /// Read one answer against the step that asked for it.
    fn answered(&mut self, answer: &OperationAnswered) {
        let Some(step) = self.pending.take() else {
            // An answer to nothing this instance asked for is the world going on.
            return;
        };
        let name = step.name();
        match step {
            Step::List => match self.reading(answer, name) {
                Reading::Result(SettlementResult::Sites(page)) => {
                    // The flows this fixture drives are the authored-site ones:
                    // core mints an authored site from the deployment's own catalog,
                    // while a generated village is described where it already
                    // stands and has no catalog candidate to plan against.
                    let Some(site) = page.sites.iter().find(|site| {
                        site.provenance == SiteProvenance::Authored && free_home(site).is_some()
                    }) else {
                        self.reject(name, "the page carries no authored site with a free home");
                        return;
                    };
                    let home = free_home(site).expect("the site was chosen for its free home");
                    log(
                        LogLevel::Info,
                        &format!(
                            "P3_SETTLE list sites={} cursor={} first={} origin={},{},{} size={} pois={} revision={}",
                            page.sites.len(),
                            if page.cursor.is_some() { "some" } else { "none" },
                            site.site_id,
                            site.footprint_origin.x,
                            site.footprint_origin.y,
                            site.footprint_origin.z,
                            size_of(site),
                            site.pois.len(),
                            site.revision,
                        ),
                    );
                    self.site = Some(Site {
                        id: site.site_id.clone(),
                        revision: site.revision,
                        origin: site.footprint_origin,
                        home,
                    });
                }
                Reading::Result(other) => {
                    self.reject(name, &format!("expected a site page, saw {other:?}"));
                }
                Reading::Refused(reason) => self.refuse(name, reason),
                Reading::Mismatch => {}
            },
            Step::Query | Step::Refresh => match self.reading(answer, name) {
                Reading::Result(SettlementResult::Site(site)) => {
                    let revision = site.revision;
                    if let Some(facts) = self.site.as_mut() {
                        facts.revision = revision;
                    }
                    log(
                        LogLevel::Info,
                        &format!(
                            "P3_SETTLE {} site={} pois={} revision={}",
                            name,
                            site.site_id,
                            site.pois.len(),
                            revision,
                        ),
                    );
                }
                Reading::Result(other) => {
                    self.reject(name, &format!("expected a site, saw {other:?}"));
                }
                Reading::Refused(reason) => self.refuse(name, reason),
                Reading::Mismatch => {}
            },
            Step::QueryStale => match self.reading(answer, name) {
                Reading::Result(other) => {
                    self.reject(name, &format!("expected a refusal, saw {other:?}"));
                }
                Reading::Refused(reason) => self.refuse(name, reason),
                Reading::Mismatch => {}
            },
            Step::Reserve | Step::ReserveReplay => match self.reading(answer, name) {
                Reading::Result(SettlementResult::ResidentSite(reservation)) => {
                    self.accept_reservation(name, step, reservation);
                }
                Reading::Result(other) => {
                    self.reject(name, &format!("expected a reservation, saw {other:?}"));
                }
                Reading::Refused(reason) => self.refuse(name, reason),
                Reading::Mismatch => {}
            },
            Step::Release | Step::ReleaseAgain => match self.reading(answer, name) {
                Reading::Result(SettlementResult::ResidentSite(reservation)) => {
                    log(
                        LogLevel::Info,
                        &format!(
                            "P3_SETTLE {} token={} revision={}",
                            name, reservation.spawn_site_token, reservation.revision
                        ),
                    );
                }
                Reading::Result(other) => {
                    self.reject(name, &format!("expected a reservation, saw {other:?}"));
                }
                Reading::Refused(reason) => self.refuse(name, reason),
                Reading::Mismatch => {}
            },
            Step::SurveyFar => match self.reading(answer, name) {
                Reading::Result(other) => {
                    self.reject(name, &format!("expected a refusal, saw {other:?}"));
                }
                Reading::Refused(reason) => self.refuse(name, reason),
                Reading::Mismatch => {}
            },
            Step::Survey => match self.reading(answer, name) {
                Reading::Result(SettlementResult::Survey(survey)) => {
                    log(
                        LogLevel::Info,
                        &format!(
                            "P3_SETTLE survey token={} usable={} water={} availability={} revision={}",
                            survey.survey_token,
                            survey.usable_plots,
                            survey.water_columns,
                            availability_name(survey.chunk_availability),
                            survey.revision,
                        ),
                    );
                    self.token = Some(survey.survey_token.clone());
                }
                Reading::Result(other) => {
                    self.reject(name, &format!("expected a survey, saw {other:?}"));
                }
                Reading::Refused(reason) => self.refuse(name, reason),
                Reading::Mismatch => {}
            },
            Step::Prepare | Step::PrepareStale => match self.reading(answer, name) {
                Reading::Result(SettlementResult::Structure(structure)) => {
                    log(
                        LogLevel::Info,
                        &format!(
                            "P3_SETTLE prepare structure={} state={} stages={} revision={} plan={} origin={},{},{} rotation={}",
                            structure.structure_id,
                            state_name(structure.state),
                            structure.stages.len(),
                            structure.revision,
                            structure.resource_plan_hash,
                            structure.origin.x,
                            structure.origin.y,
                            structure.origin.z,
                            structure.rotation,
                        ),
                    );
                    self.remember_structure(structure);
                }
                Reading::Result(other) => {
                    self.reject(name, &format!("expected a structure, saw {other:?}"));
                }
                Reading::Refused(reason) => self.refuse(name, reason),
                Reading::Mismatch => {}
            },
            Step::Status => match self.reading(answer, name) {
                Reading::Result(SettlementResult::Structure(structure)) => {
                    log(
                        LogLevel::Info,
                        &format!(
                            "P3_SETTLE status structure={} state={} watermark={} revision={} plan={} consumed={} remaining={}",
                            structure.structure_id,
                            state_name(structure.state),
                            structure.watermark,
                            structure.revision,
                            structure.resource_plan_hash,
                            materials(&structure.consumed),
                            materials(&structure.remaining),
                        ),
                    );
                    self.remember_structure(structure);
                }
                Reading::Result(other) => {
                    self.reject(name, &format!("expected a structure, saw {other:?}"));
                }
                Reading::Refused(reason) => self.refuse(name, reason),
                Reading::Mismatch => {}
            },
            Step::Inventory => match self.inventory_reading(answer, name) {
                Some(InventoryResult::Snapshot(snapshot)) => {
                    if !matches!(snapshot.endpoint, InventoryEndpoint::PlayerInventory(session) if session == self.session)
                    {
                        self.reject(
                            name,
                            "the snapshot names another endpoint than the one asked",
                        );
                        return;
                    }
                    let revision = snapshot.fence.revision;
                    self.fence = Some(snapshot.fence.clone());
                    log(
                        LogLevel::Info,
                        &format!(
                            "P3_SETTLE inventory session={} slots={} revision={}",
                            self.session,
                            snapshot.slots.len(),
                            revision,
                        ),
                    );
                }
                Some(other) => {
                    self.reject(
                        name,
                        &format!("expected an inventory snapshot, saw {other:?}"),
                    );
                }
                None => {}
            },
            Step::ReserveMaterials => match self.inventory_reading(answer, name) {
                Some(InventoryResult::Reservation(reservation)) => {
                    self.accept_materials(name, reservation);
                }
                Some(other) => {
                    self.reject(name, &format!("expected a reservation, saw {other:?}"));
                }
                None => {}
            },
            Step::Advance | Step::AdvanceMaterials => match self.reading(answer, name) {
                Reading::Result(SettlementResult::Receipt(receipt)) => {
                    log(
                        LogLevel::Info,
                        &format!(
                            "P3_SETTLE {} receipt={} stage={} sequence={} blocks={} work={} consumed={} revision={}",
                            name,
                            receipt.structure_id,
                            receipt.stage,
                            receipt.sequence,
                            receipt.block_count,
                            receipt.work_units,
                            materials(&receipt.consumed),
                            receipt.revision,
                        ),
                    );
                    // The receipt carries the fence the commit left, which is what
                    // the next structural call names.
                    self.revision = receipt.revision;
                }
                // The owner pauses a structure whose site changed and answers the
                // snapshot instead of a receipt: that is a real answer, not a gap.
                Reading::Result(SettlementResult::Structure(structure)) => {
                    log(
                        LogLevel::Info,
                        &format!(
                            "P3_SETTLE {} paused state={} reason={} revision={}",
                            name,
                            state_name(structure.state),
                            structure.pause_reason.as_deref().unwrap_or("none"),
                            structure.revision,
                        ),
                    );
                    self.remember_structure(structure);
                }
                Reading::Result(other) => {
                    self.reject(name, &format!("expected a receipt, saw {other:?}"));
                }
                Reading::Refused(reason) => self.refuse(name, reason),
                Reading::Mismatch => {}
            },
            Step::Pause | Step::Cancel => match self.reading(answer, name) {
                Reading::Result(SettlementResult::Structure(structure)) => {
                    log(
                        LogLevel::Info,
                        &format!(
                            "P3_SETTLE {} state={} reason={} revision={}",
                            name,
                            state_name(structure.state),
                            structure.pause_reason.as_deref().unwrap_or("none"),
                            structure.revision,
                        ),
                    );
                    self.remember_structure(structure);
                }
                Reading::Result(other) => {
                    self.reject(name, &format!("expected a structure, saw {other:?}"));
                }
                Reading::Refused(reason) => self.refuse(name, reason),
                Reading::Mismatch => {}
            },
            Step::Bind | Step::BindContainer | Step::BindMissing | Step::BindCancelled => {
                match self.reading(answer, name) {
                    Reading::Result(SettlementResult::Warehouse(binding)) => {
                        log(
                            LogLevel::Info,
                            &format!(
                                "P3_SETTLE {name} handle={} source={:?} revision={}",
                                binding.handle, binding.source, binding.revision,
                            ),
                        );
                    }
                    Reading::Result(other) => {
                        self.reject(name, &format!("expected a binding, saw {other:?}"));
                    }
                    Reading::Refused(reason) => self.refuse(name, reason),
                    Reading::Mismatch => {}
                }
            }
            Step::Done => {}
        }
    }

    /// One reservation answer: the first reserve remembers the token a replay must
    /// match, and the replay checks it was handed the same one.
    fn accept_reservation(
        &mut self,
        name: &str,
        step: Step,
        reservation: &ResidentSiteReservation,
    ) {
        let token = reservation.spawn_site_token.clone();
        let marker = if step == Step::ReserveReplay {
            format!(
                "P3_SETTLE {name} token={token} same={}",
                if self.token.as_deref() == Some(token.as_str()) {
                    "yes"
                } else {
                    "no"
                }
            )
        } else {
            format!(
                "P3_SETTLE {name} token={token} revision={}",
                reservation.revision
            )
        };
        self.token = Some(token);
        log(LogLevel::Info, &marker);
    }

    /// Remember the structure one answer named, with the revision a later step
    /// fences against.
    fn remember_structure(&mut self, structure: &StructureSnapshot) {
        self.revision = structure.revision;
        if self.structure.is_none() {
            self.structure = Some(structure.structure_id.clone());
            self.stage = structure.stages.first().map(|stage| stage.stage.clone());
            // The snapshot's stages are the structure's whole material plan, and
            // the hash it carries is what a reservation must answer to fund this
            // structure: both are the owner's own, so a reservation this fixture
            // makes from them is the one the structure may spend from.
            self.plan = Some((plan_of(structure), structure.resource_plan_hash.clone()));
        }
    }

    /// One materials reservation: the owner holds the structure's own plan
    /// against the fence the inventory read answered, so the reservation the
    /// structure spends from is the one this fixture made.
    fn accept_materials(&mut self, name: &str, reservation: &InventoryReservationSnapshot) {
        let expected = self.plan.as_ref().map(|(_, hash)| hash.as_str());
        if Some(reservation.resource_plan_hash.as_str()) != expected {
            self.reject(
                name,
                "the reservation holds another plan than the structure's",
            );
            return;
        }
        if reservation.released || reservation.quantities.iter().any(|held| held.reserved == 0) {
            self.reject(name, "a fresh reservation answered released or empty");
            return;
        }
        log(
            LogLevel::Info,
            &format!(
                "P3_SETTLE {name} ref={} hash={} resources={} reserved={}",
                reservation.reservation_ref,
                reservation.resource_plan_hash,
                reservation.quantities.len(),
                reserved_quantities(&reservation.quantities),
            ),
        );
        self.reservation = Some(reservation.reservation_ref.clone());
    }

    /// One owned-inventory answer: this family's result, or the owner's refusal
    /// reported as this step's marker.
    fn inventory_reading<'a>(
        &self,
        answer: &'a OperationAnswered,
        name: &str,
    ) -> Option<&'a InventoryResult> {
        match &answer.outcome {
            OperationOutcome::Committed(committed) => match &committed.payload {
                OperationPayload::OwnedInventory(result) => Some(result),
                other => {
                    self.reject(
                        name,
                        &format!("committed a non-inventory payload: {other:?}"),
                    );
                    None
                }
            },
            OperationOutcome::Refused(refused) => {
                self.refuse(name, refused.reason);
                None
            }
        }
    }

    /// What one answer is: this family's result, the owner's refusal, or a shape
    /// this step did not ask for.
    fn reading<'a>(&self, answer: &'a OperationAnswered, name: &str) -> Reading<'a> {
        match &answer.outcome {
            OperationOutcome::Committed(committed) => match &committed.payload {
                OperationPayload::Settlement(result) => Reading::Result(result),
                other => {
                    self.reject(
                        name,
                        &format!("committed a non-settlement payload: {other:?}"),
                    );
                    Reading::Mismatch
                }
            },
            OperationOutcome::Refused(refused) => Reading::Refused(refused.reason),
        }
    }

    /// Report the owner's own refusal as this step's `-refused` marker.
    fn refuse(&self, name: &str, reason: OperationFailure) {
        log(
            LogLevel::Info,
            &format!("P3_SETTLE {name}-refused {}", failure_name(reason)),
        );
    }

    /// Report a step that did not answer what it asked for, as a diagnostic rather
    /// than a marker.
    fn reject(&self, step: &str, detail: &str) {
        log(
            LogLevel::Error,
            &format!("P3_SETTLE_UNEXPECTED {step}: {detail}"),
        );
    }
}

/// What one answer carries, in the three shapes a step can meet.
enum Reading<'a> {
    /// This family's own result.
    Result(&'a SettlementResult),
    /// The owner refused, with its own reason.
    Refused(OperationFailure),
    /// Neither: already reported as a diagnostic.
    Mismatch,
}

/// One free home point of interest of one site, or none when it holds none.
fn free_home(site: &SettlementSite) -> Option<String> {
    site.pois
        .iter()
        .find(|poi| poi.kind == SitePoiKind::Home && poi.state == SitePoiState::Free)
        .map(|poi| poi.poi_id.clone())
}

/// The endpoint one player's own canonical inventory is addressed by.
fn player_endpoint(player_id: u64) -> InventoryEndpoint {
    InventoryEndpoint::PlayerInventory(player_id)
}

/// The material plan of one structure, in the owner's own shape: one work portion
/// per authored stage, carrying that stage's materials.
///
/// The snapshot's stages are the plan the owner fences with `resource-plan-hash`,
/// so a reservation built from them is byte-identical to the plan the structure
/// was prepared against.
fn plan_of(structure: &StructureSnapshot) -> InventoryResourcePlan {
    InventoryResourcePlan {
        portions: structure
            .stages
            .iter()
            .map(|stage| InventoryWorkPortion {
                work_units: stage.work_units,
                materials: stage
                    .materials
                    .iter()
                    .map(|material| InventoryMaterial {
                        resource_id: material.resource.clone(),
                        quantity: material.quantity,
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// One material list as a marker carries it: `resource:quantity` pairs in the
/// owner's own order.
fn materials(materials: &[StructureMaterial]) -> String {
    materials
        .iter()
        .map(|material| format!("{}:{}", material.resource, material.quantity))
        .collect::<Vec<_>>()
        .join(",")
}

/// One reservation's held units as a marker carries them: `resource:reserved`
/// pairs in the owner's own order.
fn reserved_quantities(quantities: &[InventoryReservationQuantity]) -> String {
    quantities
        .iter()
        .map(|held| format!("{}:{}", held.resource_id, held.reserved))
        .collect::<Vec<_>>()
        .join(",")
}

/// The site's own footprint size, as the marker prints it.
fn size_of(site: &SettlementSite) -> String {
    format!(
        "{},{},{}",
        site.footprint_size.x, site.footprint_size.y, site.footprint_size.z
    )
}

/// The region one survey covers: the site's own footprint origin, one contract
/// axis wide and one contract axis deep, clamped to the world's own ceiling.
fn survey_bounds(origin: BlockPosition) -> SurveyBounds {
    SurveyBounds {
        min: origin,
        max: BlockPosition {
            x: origin.x + SURVEY_SPAN,
            y: (origin.y + SURVEY_DEPTH).min(WORLD_MAX_Y),
            z: origin.z + SURVEY_SPAN,
        },
    }
}

/// The region the un-generated probe covers: one survey's own extents, shifted to
/// a part of the world nobody has generated.
fn far_bounds(origin: BlockPosition) -> SurveyBounds {
    let shifted = BlockPosition {
        x: origin.x + SURVEY_FAR_SHIFT,
        y: origin.y,
        z: origin.z + SURVEY_FAR_SHIFT,
    };
    survey_bounds(shifted)
}

/// The base row one structure is planned at: the site's own base row, raised out
/// of the terrain the site was laid out on.
fn anchor(origin: BlockPosition) -> BlockPosition {
    BlockPosition {
        x: origin.x,
        y: origin.y + ANCHOR_RISE,
        z: origin.z,
    }
}

/// Whether one survey's chunks were loaded, in the contract's own vocabulary.
fn availability_name(value: ChunkAvailability) -> &'static str {
    match value {
        ChunkAvailability::Loaded => "loaded",
        ChunkAvailability::Unloaded => "unloaded",
    }
}

/// One structure's durable state, in the contract's own vocabulary.
fn state_name(value: StructureState) -> &'static str {
    match value {
        StructureState::Prepared => "prepared",
        StructureState::Running => "running",
        StructureState::Paused => "paused",
        StructureState::Committed => "committed",
        StructureState::Cancelled => "cancelled",
    }
}

/// The contract's own name for one operation failure, which is what a marker
/// reports: a plugin reads the server's vocabulary instead of parsing text.
fn failure_name(failure: OperationFailure) -> &'static str {
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
