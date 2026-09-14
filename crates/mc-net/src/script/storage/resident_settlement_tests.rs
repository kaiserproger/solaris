//! Focused tests for the C4 work orders that drive C2 settlement state: the
//! `garrison` occupancy resolver and the `construct` stage advance.
//!
//! Every test drives the real resident order executor over a real plugin
//! storage journal, the real deterministic settlement selector and a
//! deterministic world adapter, so nothing here asserts plumbing.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use mc_data::Identifier;
use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_data::item_components::solaris_required_item_facts;
use mc_data::items::solaris_required_items;
use mc_entity::{FormationKind, FormationSlots, GoalState, Vec3};
use mc_script::{
    ScriptInventoryEndpoint, ScriptInventoryMaterial, ScriptInventoryReservationQuantity,
    ScriptInventoryReservationSnapshot, ScriptInventoryResourcePlan, ScriptInventoryWorkPortion,
    ScriptOperation, ScriptOperationFailure, ScriptOperationOutcome, ScriptOperationPayload,
    ScriptOperationRequest, ScriptOrderMemberState, ScriptResidentKind, ScriptResidentOperation,
    ScriptResidentOrder, ScriptResidentOrderOperation, ScriptResidentOrderResult,
    ScriptResidentProfile, ScriptResidentWorkOrder, ScriptSettlementOperation,
    ScriptSettlementResult, ScriptSettlementSite, ScriptSitePoiKind, ScriptSitePoiState,
    ScriptStructureSnapshot, ScriptSurveyBounds, ScriptWorkState, resident_generation_id,
};
use mc_world::BlockRegistry;
use mc_worldgen::{BlueprintCatalog, PoiKind, SettlementSelector};

use crate::play::SessionRegistry;
use crate::play::owned_inventory::{resource_plan_hash, resource_plan_totals};
use crate::play::resident_work::{ResidentBlock, ResidentDrop, ResidentWorld};
use crate::server::ShutdownHandle;

use super::PluginStorage;
use super::ScriptStoragePrepareOutcome;
use super::settlement::{
    ContainerReading, SettlementRuntime, SettlementWorld, StructureBlockPlacement, SurveyReading,
};
use super::world_inventory::InventoryRuntime;

const OWNER: &str = "settlement";
const WORLD_IDENTITY: &str = "resident-settlement-world";
const SEED: i64 = 7;
const PROFILE_REVISION: u64 = 3;
const START_CELL: [i32; 2] = [0, 0];
const ANCHOR_Y: i32 = 64;
/// The staged structure the construct test builds.
const COTTAGE: &str = "solaris:cottage";
/// The player endpoint the test reservation is held against.
const PLAYER: u64 = 7;

/// Deterministic world fake: it records every committed structure block and
/// answers every resident work/route probe as open flat terrain.
#[derive(Default)]
pub(crate) struct TestWorld {
    applied: Mutex<BTreeMap<String, Vec<[i32; 3]>>>,
    /// Whether point-of-interest cells refuse a standing body.
    unstandable: Mutex<bool>,
}

impl TestWorld {
    /// Make every cell refuse a standing body, as terrain above a point does.
    pub(crate) fn refuse_standing(&self) {
        *self.unstandable.lock().unwrap() = true;
    }
}

impl TestWorld {
    fn built_blocks(&self, structure_id: &str) -> usize {
        self.applied
            .lock()
            .unwrap()
            .get(structure_id)
            .map_or(0, Vec::len)
    }
}

impl SettlementWorld for TestWorld {
    fn survey(
        &self,
        _plugin_id: &str,
        _dimension: &str,
        bounds: ScriptSurveyBounds,
    ) -> Result<SurveyReading, ScriptOperationFailure> {
        let x = u64::try_from(bounds.max[0] - bounds.min[0] + 1).unwrap_or(0);
        let z = u64::try_from(bounds.max[2] - bounds.min[2] + 1).unwrap_or(0);
        Ok(SurveyReading {
            usable_plots: u32::try_from(x * z).unwrap_or(u32::MAX),
            water_columns: 0,
            claimed: false,
            existing_structures: 0,
            biome_tags: vec!["minecraft:plains".to_owned()],
            resource_tags: Vec::new(),
            chunk_availability: mc_script::ScriptChunkAvailability::Loaded,
            revision: 1,
        })
    }

    fn observe_footprint(&self, _bounds: ScriptSurveyBounds) -> u64 {
        1
    }

    fn footprint_changed_since(&self, _bounds: ScriptSurveyBounds, _revision: u64) -> bool {
        false
    }

    fn claims_overlap(&self, _plugin_id: &str, _bounds: ScriptSurveyBounds) -> bool {
        false
    }

    fn max_opaque_y(
        &self,
        _bounds: ScriptSurveyBounds,
    ) -> Result<Option<i32>, ScriptOperationFailure> {
        // A flat world whose surface is the structure anchor row: nothing
        // blocks a footprint.
        Ok(Some(ANCHOR_Y))
    }

    fn container_reading(
        &self,
        _position: [i32; 3],
    ) -> Result<ContainerReading, ScriptOperationFailure> {
        // This fake never holds a container: a warehouse bind against it stays
        // a typed not-found rather than an invented empty container.
        Ok(ContainerReading::Missing)
    }

    fn apply_structure_portion<'a>(
        &'a self,
        _plugin_id: &'a str,
        structure_id: &'a str,
        blocks: &'a [StructureBlockPlacement],
    ) -> Pin<Box<dyn Future<Output = Result<(), ScriptOperationFailure>> + Send + 'a>> {
        Box::pin(async move {
            self.applied
                .lock()
                .unwrap()
                .entry(structure_id.to_owned())
                .or_default()
                .extend(blocks.iter().map(|block| block.pos));
            Ok(())
        })
    }
}

impl ResidentWorld for TestWorld {
    fn dimension_loaded(&self, _dimension: &str) -> bool {
        true
    }

    fn block(&self, _dimension: &str, _pos: [i32; 3]) -> Option<ResidentBlock> {
        Some(ResidentBlock {
            state: 0,
            path: "air".to_owned(),
        })
    }

    fn standable(&self, _dimension: &str, _pos: [i32; 3]) -> Option<bool> {
        Some(!*self.unstandable.lock().unwrap())
    }

    fn route_open(&self, _dimension: &str, _from: [i32; 3], _to: [i32; 3]) -> Option<bool> {
        Some(true)
    }

    fn line_of_sight(&self, _dimension: &str, _from: Vec3, _to: Vec3) -> Option<bool> {
        Some(true)
    }

    fn foreign_zone_overlaps(
        &self,
        _plugin_id: &str,
        _dimension: &str,
        _min: [i32; 3],
        _max: [i32; 3],
    ) -> bool {
        false
    }

    fn state_for(&self, _block_path: &str) -> Option<u32> {
        Some(0)
    }

    fn break_block(
        &self,
        _dimension: &str,
        _pos: [i32; 3],
        _expected_state: u32,
        _tool: Option<&str>,
    ) -> Result<Vec<ResidentDrop>, ScriptOperationFailure> {
        Ok(Vec::new())
    }

    fn place_block(
        &self,
        _dimension: &str,
        _pos: [i32; 3],
        _state: u32,
    ) -> Result<(), ScriptOperationFailure> {
        Ok(())
    }
}

/// A stub block registry holding the two palette blocks the catalog authoring
/// uses.
fn stub_registry() -> BlockRegistry {
    let stone = BlockReport {
        id: Identifier::parse("minecraft:stone").unwrap(),
        properties: BTreeMap::new(),
        states: vec![BlockStateReport {
            id: 0,
            default: true,
            properties: BTreeMap::new(),
        }],
    };
    let planks = BlockReport {
        id: Identifier::parse("minecraft:oak_planks").unwrap(),
        properties: BTreeMap::new(),
        states: vec![BlockStateReport {
            id: 1,
            default: true,
            properties: BTreeMap::new(),
        }],
    };
    BlockRegistry::from_report(&[stone, planks]).unwrap()
}

/// One authored stage: its name and its `(local position, palette index)` cells.
type AuthoredStage<'a> = (&'a str, &'a [([i32; 3], u16)]);

fn building_toml(name: &str, kind: &str, capacity: u16, stages: &[AuthoredStage<'_>]) -> String {
    let mut text = format!(
        "id = \"solaris:{name}\"\nrevision = 1\n\
         [footprint]\nsize = [4, 4, 4]\nanchor = [0, 0, 0]\n\
         [[palette]]\nindex = 0\nblock = \"minecraft:stone\"\nproperties = {{}}\n\
         [[palette]]\nindex = 1\nblock = \"minecraft:oak_planks\"\nproperties = {{}}\n\
         [[blocks]]\nx = 0\ny = 0\nz = 0\npalette = 0\n\
         [[poi]]\nid = \"{kind}\"\nkind = \"{kind}\"\nat = [1, 1, 1]\ncapacity = {capacity}\n\
         [[street_connection]]\nat = [0, 0, 0]\nfacing = \"north\"\n"
    );
    for (stage, blocks) in stages {
        text.push_str(&format!("[[stage]]\nid = \"{stage}\"\n"));
        for (at, palette) in *blocks {
            text.push_str(&format!(
                "[[stage.blocks]]\nx = {}\ny = {}\nz = {}\npalette = {palette}\n",
                at[0], at[1], at[2]
            ));
        }
    }
    text
}

fn catalog() -> BlueprintCatalog {
    let files = vec![
        (
            "structures/cottage.toml".to_owned(),
            building_toml(
                "cottage",
                "home",
                8,
                &[
                    (
                        "foundation",
                        &[([0, 0, 0], 0), ([1, 0, 0], 0), ([2, 0, 0], 0)],
                    ),
                    ("walls", &[([1, 1, 1], 1), ([2, 1, 1], 1)]),
                ],
            ),
        ),
        (
            "structures/hall.toml".to_owned(),
            building_toml("hall", "meeting", 0, &[("body", &[([0, 1, 0], 0)])]),
        ),
        (
            "structures/forge.toml".to_owned(),
            building_toml("forge", "work", 0, &[("body", &[([0, 1, 0], 0)])]),
        ),
        (
            "structures/tower.toml".to_owned(),
            building_toml("tower", "guard", 3, &[]),
        ),
    ];
    BlueprintCatalog::from_files(&stub_registry(), "solaris", &files).unwrap()
}

struct Fixture {
    root: tempfile::TempDir,
    runtime: InventoryRuntime,
    world: Arc<TestWorld>,
    catalog: Arc<BlueprintCatalog>,
    selector: SettlementSelector,
    sessions: Arc<SessionRegistry>,
}

/// Topmost solid row of the flat ground the resident tests stand on.
fn flat_ground(_world_x: i32, _world_z: i32) -> Option<i32> {
    Some(63)
}

/// Flat ground generator backing the resident fixture's runtime.
struct FlatGround;

impl mc_world::ChunkGenerator for FlatGround {
    fn generate(&self, _pos: mc_world::ChunkPos) -> mc_world::Chunk {
        panic!("the resident tests never ask a generator for a chunk")
    }

    fn surface_height(&self, world_x: i32, world_z: i32) -> Option<i32> {
        flat_ground(world_x, world_z)
    }
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let catalog = Arc::new(catalog());
        let world = Arc::new(TestWorld::default());
        let sessions = Arc::new(SessionRegistry::new());
        let runtime = InventoryRuntime::new(
            None,
            &ShutdownHandle::default(),
            Arc::clone(&sessions),
            Arc::new(solaris_required_items()),
            Arc::new(solaris_required_item_facts()),
        )
        .with_settlement_runtime(Arc::new(SettlementRuntime::new(
            SettlementSelector::new(SEED, PROFILE_REVISION),
            Arc::clone(&catalog),
            WORLD_IDENTITY,
            START_CELL,
            Arc::new(FlatGround),
        )))
        .with_settlement_world(Arc::clone(&world) as Arc<dyn SettlementWorld>)
        .with_resident_world(Arc::clone(&world) as Arc<dyn ResidentWorld>);
        Self {
            root,
            runtime,
            world,
            catalog,
            selector: SettlementSelector::new(SEED, PROFILE_REVISION),
            sessions,
        }
    }

    fn storage(&self) -> PluginStorage {
        PluginStorage::open(self.root.path()).unwrap()
    }

    /// The first deterministic site candidate.
    fn candidate(&self) -> mc_worldgen::SiteCandidate {
        self.selector
            .discover(START_CELL, 64)
            .into_iter()
            .next()
            .expect("the deterministic selector finds a site in the first window")
    }

    /// The one approved guard post the site layout authors, with its anchor and
    /// capacity.
    fn guard_post(&self) -> (String, Vec3, u16) {
        let candidate = self.candidate();
        let layout = self
            .selector
            .layout(&candidate, &self.catalog, &flat_ground)
            .expect("the catalog lays the site out");
        let poi = layout
            .pois
            .iter()
            .find(|poi| poi.kind == PoiKind::Guard)
            .expect("the catalog authors a guard point of interest");
        (
            poi.poi_id.clone(),
            Vec3::new(
                f64::from(poi.at[0]) + 0.5,
                f64::from(poi.at[1]),
                f64::from(poi.at[2]) + 0.5,
            ),
            poi.capacity,
        )
    }

    /// The engine-computed position of one guard-post slot, matching the
    /// executor's own resolver.
    fn guard_position(&self, slot: u16) -> Vec3 {
        let (_, anchor, capacity) = self.guard_post();
        FormationSlots::compute(FormationKind::Line, anchor, 0.0, 1.0, usize::from(capacity))
            .expect("guard capacity is within the formation bound")
            .slot(usize::from(slot))
            .expect("the requested slot exists")
    }

    async fn resident(
        &self,
        storage: &mut PluginStorage,
        slot: u32,
        position: Vec3,
    ) -> (String, uuid::Uuid) {
        let generation =
            resident_generation_id(WORLD_IDENTITY, "site-7", slot).expect("generation id");
        let snapshot = self
            .runtime
            .materialize_resident(storage, OWNER, &generation, position)
            .await
            .expect("resident materialises");
        let handle = mc_script::resident_handle_for_generation(OWNER, &generation).expect("handle");
        assert_eq!(handle, snapshot.handle);
        (
            handle,
            uuid::Uuid::parse_str(&snapshot.entity_uuid).unwrap(),
        )
    }

    async fn execute(
        &self,
        storage: &mut PluginStorage,
        request: &ScriptOperationRequest,
    ) -> ScriptOperationOutcome {
        self.runtime
            .execute_resident_order_operation(storage, OWNER, request)
            .await
            .expect("resident order reaches the durable boundary")
    }

    async fn goal(&self, uuid: uuid::Uuid) -> GoalState {
        self.sessions
            .resident_entity_snapshots(&[uuid])
            .await
            .into_iter()
            .next()
            .flatten()
            .expect("resident stays live")
            .goal
    }

    /// Survey the fixture's first site and prepare the staged cottage inside it.
    async fn prepared_cottage(
        &self,
        storage: &mut PluginStorage,
        operation_id: &str,
    ) -> ScriptStructureSnapshot {
        let bounds = ScriptSurveyBounds::new([0, 0, 0], [7, 3, 7]).unwrap();
        let survey = ScriptOperationRequest::try_new(
            "request",
            ScriptOperation::Settlement {
                operation: mc_script::ScriptSettlementOperation::Survey {
                    dimension: "minecraft:overworld".to_owned(),
                    bounds,
                    purpose: mc_script::ScriptSurveyPurpose::Settlement,
                },
            },
        )
        .unwrap();
        let outcome = self
            .runtime
            .execute_settlement_operation(storage, OWNER, &survey)
            .await
            .expect("survey reaches the durable boundary");
        let token = match outcome.payload() {
            ScriptOperationPayload::Settlement { result } => match &**result {
                mc_script::ScriptSettlementResult::Survey { survey } => survey.survey_token.clone(),
                other => panic!("expected a survey, got {other:?}"),
            },
            other => panic!("expected a settlement payload, got {other:?}"),
        };
        let candidate = self.candidate();
        let anchor = [candidate.origin[0], ANCHOR_Y, candidate.origin[2]];
        let prepare = ScriptOperationRequest::try_new(
            "request",
            ScriptOperation::Settlement {
                operation: mc_script::ScriptSettlementOperation::PrepareStructure {
                    operation_id: operation_id.to_owned(),
                    blueprint_id: COTTAGE.to_owned(),
                    anchor,
                    rotation: 0,
                    survey_token: token,
                    expected_site_revision: 0,
                },
            },
        )
        .unwrap();
        let outcome = self
            .runtime
            .execute_settlement_operation(storage, OWNER, &prepare)
            .await
            .expect("prepare reaches the durable boundary");
        assert_eq!(outcome.failure(), None, "prepare: {outcome:?}");
        match outcome.payload() {
            ScriptOperationPayload::Settlement { result } => match &**result {
                mc_script::ScriptSettlementResult::Structure { structure } => (**structure).clone(),
                other => panic!("expected a structure, got {other:?}"),
            },
            other => panic!("expected a settlement payload, got {other:?}"),
        }
    }
}

fn order_request(
    operation_id: &str,
    handles: &[String],
    revisions: &[u64],
    order: ScriptResidentOrder,
) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::ResidentOrder {
            operation: ScriptResidentOrderOperation::IssueOrder {
                operation_id: operation_id.to_owned(),
                handles: handles.to_vec(),
                expected_order_revisions: revisions.to_vec(),
                order,
            },
        },
    )
    .expect("valid order request")
}

fn work_request(
    operation_id: &str,
    handle: &str,
    work: ScriptResidentWorkOrder,
    work_units: u64,
    expected_revision: u64,
) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::ResidentOrder {
            operation: ScriptResidentOrderOperation::AssignWork {
                operation_id: operation_id.to_owned(),
                handle: handle.to_owned(),
                work,
                work_units,
                expected_revision,
            },
        },
    )
    .expect("valid work request")
}

fn order_of(outcome: &ScriptOperationOutcome) -> (u64, Vec<mc_script::ScriptOrderMemberOutcome>) {
    let ScriptOperationPayload::ResidentOrder { result } = outcome.payload() else {
        panic!("expected a resident order payload, got {outcome:?}");
    };
    match &**result {
        ScriptResidentOrderResult::Order {
            order_revision,
            members,
            ..
        } => (*order_revision, members.clone()),
        other => panic!("expected an order result, got {other:?}"),
    }
}

fn work_of(outcome: &ScriptOperationOutcome) -> mc_script::ScriptWorkAssignment {
    let ScriptOperationPayload::ResidentOrder { result } = outcome.payload() else {
        panic!("expected a resident order payload, got {outcome:?}");
    };
    match &**result {
        ScriptResidentOrderResult::Work { assignment } => (**assignment).clone(),
        other => panic!("expected a work result, got {other:?}"),
    }
}

/// The reserved quantity of one resource, or a panic when it is not reserved.
fn quantity_of<'a>(
    snapshot: &'a ScriptInventoryReservationSnapshot,
    resource: &str,
) -> &'a ScriptInventoryReservationQuantity {
    snapshot
        .quantities
        .iter()
        .find(|quantity| quantity.resource_id == resource)
        .unwrap_or_else(|| panic!("{resource} is reserved: {snapshot:?}"))
}

/// The C1 plan equivalent of one structure snapshot's stage plan.
fn plan_of(structure: &ScriptStructureSnapshot) -> ScriptInventoryResourcePlan {
    ScriptInventoryResourcePlan::new(
        structure
            .stages
            .iter()
            .map(|stage| {
                ScriptInventoryWorkPortion::new(
                    stage.work_units,
                    stage
                        .materials
                        .iter()
                        .map(|material| {
                            ScriptInventoryMaterial::new(
                                material.resource.clone(),
                                material.quantity,
                            )
                        })
                        .collect(),
                )
            })
            .collect(),
    )
}

/// Commit one durable C1 reservation receipt for `plan`, exactly the projection
/// the C1 reserve operation installs, without needing a live player session.
fn reserve_plan(
    storage: &mut PluginStorage,
    reference: &str,
    plan: &ScriptInventoryResourcePlan,
) -> ScriptInventoryReservationSnapshot {
    let quantities = resource_plan_totals(plan)
        .unwrap()
        .into_iter()
        .map(|(resource, quantity)| {
            ScriptInventoryReservationQuantity::new(resource, quantity, 0, 0, quantity)
        })
        .collect();
    let reservation = ScriptInventoryReservationSnapshot::new(
        reference.to_owned(),
        ScriptInventoryEndpoint::PlayerInventory { player_id: PLAYER },
        resource_plan_hash(plan),
        quantities,
        None,
        false,
        0,
    );
    let request = ScriptOperationRequest::try_new(
        "reserve-request",
        ScriptOperation::Inventory {
            operation: mc_script::ScriptOwnedInventoryOperation::Reserve {
                operation_id: format!("reserve-{reference}"),
                endpoint: ScriptInventoryEndpoint::PlayerInventory { player_id: PLAYER },
                resource_plan: plan.clone(),
                expected_revision: mc_script::ScriptInventoryFence::try_new(0, "0".repeat(64))
                    .unwrap(),
            },
        },
    )
    .unwrap();
    let batch = match storage
        .prepare_owned_batch(
            OWNER,
            &request,
            ScriptOperationPayload::OwnedInventory {
                result: Box::new(mc_script::ScriptOwnedInventoryResult::Reservation {
                    reservation: reservation.clone(),
                }),
            },
            None,
        )
        .unwrap()
    {
        ScriptStoragePrepareOutcome::Prepared(batch) => batch,
        ScriptStoragePrepareOutcome::Rejected => panic!("reservation projection was rejected"),
    };
    storage.commit_batch(batch).unwrap();
    reservation
}

/// One resident record's durable garrison assignment, if any.
fn garrison_slot(storage: &PluginStorage, handle: &str) -> Option<(String, u16)> {
    storage
        .resident_orders()
        .record(handle)
        .and_then(|record| record.order.as_ref())
        .and_then(|order| order.garrison.clone())
        .map(|slot| (slot.post, slot.slot))
}

/// The durable record revision fence one handle must send back for a work
/// assignment or a fresh order.
fn order_revision(storage: &PluginStorage, handle: &str) -> u64 {
    storage
        .resident_orders()
        .record(handle)
        .map_or(0, |record| record.revision)
}

/// One settlement request wrapper.
fn settlement_request(operation: ScriptSettlementOperation) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new("request", ScriptOperation::Settlement { operation })
        .expect("valid settlement request")
}

/// Query the fixture's first settlement site.
async fn query_site(fixture: &Fixture, storage: &mut PluginStorage) -> ScriptSettlementSite {
    let outcome = fixture
        .runtime
        .execute_settlement_operation(
            storage,
            OWNER,
            &settlement_request(ScriptSettlementOperation::QuerySite {
                site_id: fixture.candidate().site_id,
                cursor: None,
                limit: 64,
            }),
        )
        .await
        .expect("query reaches the durable boundary");
    match outcome.payload() {
        ScriptOperationPayload::Settlement { result } => match &**result {
            ScriptSettlementResult::Site { site } => site.as_ref().clone(),
            other => panic!("expected one site, got {other:?}"),
        },
        other => panic!("expected a settlement payload, got {other:?}"),
    }
}

/// The first home point of interest a site offers.
fn first_home(site: &ScriptSettlementSite) -> String {
    site.pois
        .iter()
        .find(|poi| poi.kind == ScriptSitePoiKind::Home)
        .expect("a site offers a home slot")
        .poi_id
        .clone()
}

/// One point of interest's projected state.
fn poi_state(site: &ScriptSettlementSite, poi_id: &str) -> ScriptSitePoiState {
    site.pois
        .iter()
        .find(|poi| poi.poi_id == poi_id)
        .expect("the poi stays on the site")
        .state
}

/// Reserve one home point of interest of `site` for the owner.
async fn reserve_home(
    fixture: &Fixture,
    storage: &mut PluginStorage,
    site: &ScriptSettlementSite,
    poi_id: &str,
) -> ScriptOperationOutcome {
    fixture
        .runtime
        .execute_settlement_operation(
            storage,
            OWNER,
            &settlement_request(ScriptSettlementOperation::ReserveResidentSite {
                operation_id: "reserve-home".to_owned(),
                site_id: site.site_id.clone(),
                poi_id: poi_id.to_owned(),
                expected_site_revision: site.revision,
            }),
        )
        .await
        .expect("reserve reaches the durable boundary")
}

/// The spawn-site token of a committed reservation.
fn resident_token(outcome: &ScriptOperationOutcome) -> String {
    match outcome.payload() {
        ScriptOperationPayload::Settlement { result } => match &**result {
            ScriptSettlementResult::ResidentSite { reservation } => {
                reservation.spawn_site_token.clone()
            }
            other => panic!("expected a resident site reservation, got {other:?}"),
        },
        other => panic!("expected a settlement payload, got {other:?}"),
    }
}

/// Release one spawn site for the owner.
fn release_request(operation_id: &str, token: &str) -> ScriptOperationRequest {
    settlement_request(ScriptSettlementOperation::ReleaseResidentSite {
        operation_id: operation_id.to_owned(),
        spawn_site_token: token.to_owned(),
    })
}

/// (C4) A garrison order occupies free approved guard posts from C2's committed
/// site layout with distinct engine-computed positions; a second order never
/// double-books a slot, occupancy survives a reload, and a member with no free
/// reachable slot honestly reports `blocked_route`.
#[tokio::test]
async fn garrison_occupies_free_posts_without_double_booking() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let (first, first_uuid) = fixture
        .resident(&mut storage, 1, Vec3::new(0.5, 64.0, 0.5))
        .await;
    let (second, second_uuid) = fixture
        .resident(&mut storage, 2, Vec3::new(1.5, 64.0, 0.5))
        .await;
    let (third, third_uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 0.5))
        .await;
    let (fourth, fourth_uuid) = fixture
        .resident(&mut storage, 4, Vec3::new(3.5, 64.0, 0.5))
        .await;
    let (post, _, capacity) = fixture.guard_post();
    assert_eq!(capacity, 3, "the catalog authors three guard slots");
    let garrison = ScriptResidentOrder::Garrison {
        posts: vec![post.clone()],
        engagement_radius: 8,
    };

    let outcome = fixture
        .execute(
            &mut storage,
            &order_request("g1", std::slice::from_ref(&first), &[0], garrison.clone()),
        )
        .await;
    let (_, members) = order_of(&outcome);
    assert_eq!(members[0].state, ScriptOrderMemberState::Applied);
    assert_eq!(
        fixture.goal(first_uuid).await,
        GoalState::FollowPosition {
            target: fixture.guard_position(0),
            speed: 1.0,
        },
        "the first member really holds slot 0"
    );
    assert_eq!(garrison_slot(&storage, &first), Some((post.clone(), 0)));

    // A second order must not double-book slot 0.
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request("g2", std::slice::from_ref(&second), &[0], garrison.clone()),
        )
        .await;
    let (_, members) = order_of(&outcome);
    assert_eq!(members[0].state, ScriptOrderMemberState::Applied);
    let second_goal = fixture.goal(second_uuid).await;
    assert_eq!(
        second_goal,
        GoalState::FollowPosition {
            target: fixture.guard_position(1),
            speed: 1.0,
        },
        "the second member occupies a distinct slot"
    );
    assert_ne!(
        second_goal,
        fixture.goal(first_uuid).await,
        "two members never share a coordinate"
    );

    // Effective only after a reload: the durable occupancy survives the journal.
    storage.force_compact_for_test().unwrap();
    drop(storage);
    let mut storage = fixture.storage();
    assert_eq!(garrison_slot(&storage, &first), Some((post.clone(), 0)));

    // A committed garrison admission replayed after the reload reconstructs the
    // same post goal, so the member returns to its post without a fresh order.
    let recovered = storage
        .resident_orders()
        .record(&first)
        .cloned()
        .expect("the garrison record survived");
    storage
        .force_pending_admission_for_test(
            OWNER,
            "recover-garrison",
            std::slice::from_ref(&recovered),
        )
        .unwrap();
    fixture.runtime.recover_resident_orders(&mut storage).await;
    assert_eq!(
        fixture.goal(first_uuid).await,
        GoalState::FollowPosition {
            target: fixture.guard_position(0),
            speed: 1.0,
        },
        "a replayed garrison order returns to the occupied post"
    );

    let outcome = fixture
        .execute(
            &mut storage,
            &order_request("g3", std::slice::from_ref(&third), &[0], garrison.clone()),
        )
        .await;
    assert_eq!(
        order_of(&outcome).1[0].state,
        ScriptOrderMemberState::Applied
    );
    assert_eq!(
        fixture.goal(third_uuid).await,
        GoalState::FollowPosition {
            target: fixture.guard_position(2),
            speed: 1.0,
        },
        "the reloaded claims still hold slots 0 and 1"
    );

    // Every slot is now occupied: the fourth member fails closed.
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request("g4", std::slice::from_ref(&fourth), &[0], garrison.clone()),
        )
        .await;
    assert_eq!(
        order_of(&outcome).1[0].state,
        ScriptOrderMemberState::BlockedRoute,
        "no free approved post means a typed blocked_route"
    );
    assert_eq!(
        fixture.goal(fourth_uuid).await,
        GoalState::Idle,
        "a blocked member receives no goal"
    );

    // An unknown post resolves to nothing rather than a guessed position.
    let revision = order_revision(&storage, &fourth);
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "g5",
                std::slice::from_ref(&fourth),
                &[revision],
                ScriptResidentOrder::Garrison {
                    posts: vec!["site_0_0_forged.0.guard".to_owned()],
                    engagement_radius: 8,
                },
            ),
        )
        .await;
    assert_eq!(
        order_of(&outcome).1[0].state,
        ScriptOrderMemberState::BlockedRoute
    );
}

/// (C4) `construct` work drives a prepared C2 stage: it consumes exactly the
/// reserved portion, commits the world portion and reports the receipt's work
/// units. A repeated portion neither double-consumes nor commits blocks twice.
#[tokio::test]
async fn construct_work_consumes_the_reserved_portion_and_commits_the_stage() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let (handle, _uuid) = fixture
        .resident(&mut storage, 1, Vec3::new(0.5, 64.0, 0.5))
        .await;
    let structure = fixture
        .prepared_cottage(&mut storage, "prepare-construct")
        .await;
    let stage = structure.stages[0].clone();
    let plan = plan_of(&structure);
    reserve_plan(&mut storage, "res-construct", &plan);

    let work = ScriptResidentWorkOrder::Construct {
        structure_id: structure.structure_id.clone(),
        stage: stage.stage.clone(),
        expected_revision: structure.revision,
    };
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request("construct-1", &handle, work.clone(), stage.work_units, 0),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(
        assignment.state,
        ScriptWorkState::Committed,
        "{assignment:?}"
    );
    assert_eq!(
        assignment.work_units_done, stage.work_units,
        "the work units reported are the receipt's committed portion"
    );
    assert_eq!(
        fixture.world.built_blocks(&structure.structure_id),
        usize::try_from(stage.work_units).unwrap(),
        "the world portion really committed"
    );

    // The receipt's consumed vector is coupled to the C1 reservation.
    let (_, projection) = storage
        .settlement_reservation(OWNER, "res-construct")
        .expect("the reservation stays projected");
    assert_eq!(
        quantity_of(&projection, "minecraft:stone").consumed,
        stage
            .materials
            .iter()
            .find(|material| material.resource == "minecraft:stone")
            .map_or(0, |material| material.quantity),
        "the reserved stone is consumed exactly once"
    );
    for material in &stage.materials {
        let quantity = quantity_of(&projection, &material.resource);
        assert_eq!(quantity.consumed, material.quantity);
        assert_eq!(
            quantity.reserved,
            quantity.consumed + quantity.returned + quantity.remaining,
            "reserved = consumed + returned + remaining for {}",
            material.resource
        );
    }

    // Replaying the same work request changes nothing.
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request("construct-1", &handle, work.clone(), stage.work_units, 0),
        )
        .await;
    assert_eq!(
        assignment.work_units_done,
        work_of(&outcome).work_units_done
    );
    assert_eq!(
        fixture.world.built_blocks(&structure.structure_id),
        usize::try_from(stage.work_units).unwrap(),
        "a replay commits no second portion"
    );
    let (_, projection) = storage
        .settlement_reservation(OWNER, "res-construct")
        .expect("the reservation stays projected");
    assert_eq!(
        quantity_of(&projection, "minecraft:stone").consumed,
        stage.materials[0].quantity,
        "a replay consumes nothing"
    );

    // A stage that genuinely cannot resolve its structure pauses with a typed
    // reason and commits nothing.
    let unknown = ScriptResidentWorkOrder::Construct {
        structure_id: "0".repeat(64),
        stage: stage.stage.clone(),
        expected_revision: 1,
    };
    let revision = order_revision(&storage, &handle);
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request("construct-2", &handle, unknown, stage.work_units, revision),
        )
        .await;
    assert_eq!(
        work_of(&outcome).reason,
        Some(mc_script::ScriptWorkPauseReason::MissingInput)
    );
    assert_eq!(
        fixture.world.built_blocks(&"0".repeat(64)),
        0,
        "an unresolvable structure commits no blocks"
    );
}

/// (C4) The construct work refuses a stale expected revision without consuming
/// the reservation or committing blocks.
#[tokio::test]
async fn construct_work_refuses_a_stale_structure_without_consuming() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let (handle, _uuid) = fixture
        .resident(&mut storage, 1, Vec3::new(0.5, 64.0, 0.5))
        .await;
    let structure = fixture
        .prepared_cottage(&mut storage, "prepare-stale")
        .await;
    let stage = structure.stages[0].clone();
    let plan = plan_of(&structure);
    reserve_plan(&mut storage, "res-stale", &plan);

    let stale = ScriptResidentWorkOrder::Construct {
        structure_id: structure.structure_id.clone(),
        stage: stage.stage.clone(),
        expected_revision: structure.revision + 9,
    };
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request("construct-stale", &handle, stale, stage.work_units, 0),
        )
        .await;
    assert_eq!(
        work_of(&outcome).reason,
        Some(mc_script::ScriptWorkPauseReason::Interrupted)
    );
    assert_eq!(fixture.world.built_blocks(&structure.structure_id), 0);
    let (_, projection) = storage
        .settlement_reservation(OWNER, "res-stale")
        .expect("the reservation stays projected");
    assert_eq!(quantity_of(&projection, "minecraft:stone").consumed, 0);
    assert_eq!(quantity_of(&projection, "minecraft:stone").remaining, 3);
}

/// A reservation consumed by a resident spawn refuses release, commits nothing,
/// and keeps refusing after the journal reopens.
#[tokio::test]
async fn release_refuses_a_reservation_consumed_by_a_resident_spawn() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let site = query_site(&fixture, &mut storage).await;
    let home = first_home(&site);
    let outcome = reserve_home(&fixture, &mut storage, &site, &home).await;
    assert_eq!(outcome.failure(), None, "reserve: {outcome:?}");
    let token = resident_token(&outcome);

    // The real resident spawn path consumes the spawn site.
    let spawn = ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::Resident {
            operation: ScriptResidentOperation::Spawn {
                operation_id: "spawn".to_owned(),
                spawn_site_token: token.clone(),
                profile: ScriptResidentProfile::new(ScriptResidentKind::Villager),
            },
        },
    )
    .expect("valid spawn request");
    let outcome = fixture
        .runtime
        .execute_resident_operation(&mut storage, OWNER, &spawn)
        .await
        .expect("spawn reaches the durable boundary");
    assert_eq!(outcome.failure(), None, "spawn: {outcome:?}");

    let outcome = fixture
        .runtime
        .execute_settlement_operation(&mut storage, OWNER, &release_request("release", &token))
        .await
        .expect("release reaches the durable boundary");
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));

    // The refusal committed nothing: the home is still occupied.
    let occupied = query_site(&fixture, &mut storage).await;
    assert_eq!(poi_state(&occupied, &home), ScriptSitePoiState::Occupied);

    // Reopening the journal preserves the refusal.
    let mut reopened = fixture.storage();
    let outcome = fixture
        .runtime
        .execute_settlement_operation(
            &mut reopened,
            OWNER,
            &release_request("release-again", &token),
        )
        .await
        .expect("release reaches the durable boundary");
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));
}

/// A home the resident ledger already binds to an inhabitant refuses release,
/// even before any spawn marked the spawn site consumed.
#[tokio::test]
async fn release_refuses_a_home_the_resident_ledger_already_binds() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let site = query_site(&fixture, &mut storage).await;
    let home = first_home(&site);
    let outcome = reserve_home(&fixture, &mut storage, &site, &home).await;
    assert_eq!(outcome.failure(), None, "reserve: {outcome:?}");
    let token = resident_token(&outcome);

    let (generation, position) = {
        let resident_site = storage
            .residents()
            .site(&token)
            .expect("the reservation minted a resident site");
        assert!(!resident_site.consumed, "the reservation starts unconsumed");
        (
            resident_site.generation_id.clone(),
            resident_site
                .position()
                .expect("the reserved position is finite"),
        )
    };
    fixture
        .runtime
        .materialize_resident(&mut storage, OWNER, &generation, position)
        .await
        .expect("the bound inhabitant materialises");

    let outcome = fixture
        .runtime
        .execute_settlement_operation(&mut storage, OWNER, &release_request("release", &token))
        .await
        .expect("release reaches the durable boundary");
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));
}
