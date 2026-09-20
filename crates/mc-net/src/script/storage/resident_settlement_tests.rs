//! Focused tests for the C4 work orders that drive C2 settlement state: the
//! `garrison` occupancy resolver and the `construct` stage advance.
//!
//! Every test drives the real resident order executor over a real plugin
//! storage journal, the real deterministic settlement selector and a
//! deterministic world adapter, so nothing here asserts plumbing.

use mc_data::Identifier;
use mc_data::ItemStack;
use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_data::item_components::solaris_required_item_facts;
use mc_data::items::solaris_required_items;
use mc_entity::{FormationKind, FormationSlots, GoalState, Vec3};
use mc_script::{ScriptBlockPosition, ScriptWorkArea};
use mc_script::{
    ScriptInventoryEndpoint, ScriptInventoryMaterial, ScriptInventoryReservationQuantity,
    ScriptInventoryReservationSnapshot, ScriptInventoryResourcePlan, ScriptInventoryWorkPortion,
    ScriptOperation, ScriptOperationFailure, ScriptOperationOutcome, ScriptOperationPayload,
    ScriptOperationRequest, ScriptOrderMemberState, ScriptResidentKind, ScriptResidentOperation,
    ScriptResidentOrder, ScriptResidentOrderOperation, ScriptResidentOrderResult,
    ScriptResidentProfile, ScriptResidentWorkOrder, ScriptSettlementOperation,
    ScriptSettlementResult, ScriptSettlementSite, ScriptSitePoiKind, ScriptSitePoiState,
    ScriptStructureSnapshot, ScriptSurveyBounds, ScriptWorkPauseReason, ScriptWorkState,
    resident_generation_id,
};
use mc_world::BlockRegistry;
use mc_worldgen::{BlueprintCatalog, PoiKind, SettlementSelector};
use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::play::SessionRegistry;
use crate::play::owned_inventory::{
    WarehouseTransferRequest, resource_plan_hash, resource_plan_totals,
};
use crate::play::resident_work::{ResidentBlock, ResidentWorld, ResidentWorldEdit};

use super::PluginStorage;
use super::ScriptStoragePrepareOutcome;
use super::settlement::{
    ContainerReading, SettlementRuntime, SettlementWorld, StructureBlockPlacement, SurveyReading,
    VillageInhabitantReading, VillagePoiReading, VillageReading,
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
/// The authored warehouse the deposit tests bind a container through.
const WAREHOUSE: &str = "solaris:warehouse";
/// The player endpoint the test reservation is held against.
const PLAYER: u64 = 7;

/// Deterministic world fake: it records every committed structure block and
/// answers every resident work/route probe as open flat terrain. It also holds
/// the loaded containers a warehouse binding resolves to, and journals every
/// deposit it accepts onto the fixture's own world journal.
#[derive(Default)]
pub(crate) struct TestWorld {
    applied: Mutex<BTreeMap<String, Vec<[i32; 3]>>>,
    /// Whether point-of-interest cells refuse a standing body.
    unstandable: Mutex<bool>,
    /// The loaded containers this world reads.
    containers: Mutex<BTreeMap<[i32; 3], Vec<ItemStack>>>,
    /// Explicit resident-work cells beyond the fixture's default open air.
    resident_blocks: Mutex<BTreeMap<[i32; 3], ResidentBlock>>,
    /// The deposits this world half accepted, so a test can assert the plan and
    /// the count of real moves.
    deposits: Mutex<Vec<WarehouseTransferRequest>>,
    /// The registry whose world journal an accepted deposit appends to; absent
    /// in fixtures that own no journal, where a deposit is `runtime_unavailable`.
    sessions: Option<Arc<SessionRegistry>>,
}

impl TestWorld {
    /// Make every cell refuse a standing body, as terrain above a point does.
    pub(crate) fn refuse_standing(&self) {
        *self.unstandable.lock().unwrap() = true;
    }

    /// Let this fake stand in for the server-owned composite's world half: an
    /// accepted deposit journals its encoded receipt on the fixture's journal.
    fn journal_warehouse_transfers(mut self, sessions: Arc<SessionRegistry>) -> Self {
        self.sessions = Some(sessions);
        self
    }

    /// Seed one loaded container at a world position.
    fn set_container(&self, position: [i32; 3], items: Vec<ItemStack>) {
        self.containers.lock().unwrap().insert(position, items);
    }

    /// Set one loaded cell the resident-work adapter will read.
    fn set_resident_block(&self, position: [i32; 3], path: &str) {
        self.resident_blocks.lock().unwrap().insert(
            position,
            ResidentBlock {
                state: 0,
                path: path.to_owned(),
            },
        );
    }

    /// The container's current canonical slots.
    fn container(&self, position: [i32; 3]) -> Vec<ItemStack> {
        self.containers
            .lock()
            .unwrap()
            .get(&position)
            .cloned()
            .expect("the container is loaded")
    }

    /// Every deposit this world half accepted.
    fn deposits(&self) -> Vec<WarehouseTransferRequest> {
        self.deposits.lock().unwrap().clone()
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

    fn predict_structure_portion_revision(
        &self,
        _bounds: ScriptSurveyBounds,
        _blocks: &[StructureBlockPlacement],
    ) -> Result<u64, ScriptOperationFailure> {
        Ok(1)
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
        position: [i32; 3],
    ) -> Result<ContainerReading, ScriptOperationFailure> {
        Ok(match self.containers.lock().unwrap().get(&position) {
            Some(items) => ContainerReading::Loaded(items.clone()),
            None => ContainerReading::Missing,
        })
    }

    fn village_containers(
        &self,
        bounds: ScriptSurveyBounds,
    ) -> Result<VillageReading<[i32; 3]>, ScriptOperationFailure> {
        let mut containers = self
            .containers
            .lock()
            .unwrap()
            .keys()
            .copied()
            .filter(|position| {
                (bounds.min[0]..=bounds.max[0]).contains(&position[0])
                    && (bounds.min[1]..=bounds.max[1]).contains(&position[1])
                    && (bounds.min[2]..=bounds.max[2]).contains(&position[2])
            })
            .collect::<Vec<_>>();
        containers.sort_unstable();
        Ok(VillageReading::Loaded(containers))
    }

    fn commit_warehouse_transfer(
        &self,
        request: WarehouseTransferRequest,
    ) -> Pin<Box<dyn Future<Output = Result<u64, ScriptOperationFailure>> + Send + '_>> {
        Box::pin(async move {
            let Some(sessions) = self.sessions.as_ref() else {
                // This fake owns no journal, so a deposit has nowhere to ride.
                return Err(ScriptOperationFailure::RuntimeUnavailable);
            };
            let decision_id =
                super::settlement::journal_test_plugin_decision(sessions, request.receipt.clone())?;
            self.containers.lock().unwrap().insert(
                [request.position.x, request.position.y, request.position.z],
                request.updated_container.clone(),
            );
            self.deposits.lock().unwrap().push(request);
            Ok(decision_id)
        })
    }

    fn commit_structure_portion<'a>(
        &'a self,
        _plugin_id: &'a str,
        structure_id: &'a str,
        blocks: &'a [StructureBlockPlacement],
        receipt: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<u64, ScriptOperationFailure>> + Send + 'a>> {
        Box::pin(async move {
            let Some(sessions) = self.sessions.as_ref() else {
                return Err(ScriptOperationFailure::RuntimeUnavailable);
            };
            let decision_id = super::settlement::journal_test_plugin_decision(sessions, receipt)?;
            self.applied
                .lock()
                .unwrap()
                .entry(structure_id.to_owned())
                .or_default()
                .extend(blocks.iter().map(|block| block.pos));
            Ok(decision_id)
        })
    }

    /// A fake world holds no generated village: these tests exercise the
    /// authored lane, and a village site answers through its own fixtures.
    fn village_pois(
        &self,
        _bounds: ScriptSurveyBounds,
    ) -> Result<VillageReading<VillagePoiReading>, ScriptOperationFailure> {
        Ok(VillageReading::Loaded(Vec::new()))
    }

    fn village_inhabitants(
        &self,
        _bounds: ScriptSurveyBounds,
    ) -> Result<VillageReading<VillageInhabitantReading>, ScriptOperationFailure> {
        Ok(VillageReading::Loaded(Vec::new()))
    }
}

impl ResidentWorld for TestWorld {
    fn dimension_loaded(&self, _dimension: &str) -> bool {
        true
    }

    fn block(&self, _dimension: &str, pos: [i32; 3]) -> Option<ResidentBlock> {
        Some(
            self.resident_blocks
                .lock()
                .unwrap()
                .get(&pos)
                .cloned()
                .unwrap_or_else(|| ResidentBlock {
                    state: 0,
                    path: "air".to_owned(),
                }),
        )
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

    fn preview_break(
        &self,
        _dimension: &str,
        _pos: [i32; 3],
        _expected_state: u32,
        _tool: Option<&str>,
    ) -> Result<ResidentWorldEdit, ScriptOperationFailure> {
        Err(ScriptOperationFailure::RuntimeUnavailable)
    }

    fn commit_world_edits<'a>(
        &'a self,
        _plugin_id: &'a str,
        _dimension: &'a str,
        _breaks: &'a [ResidentWorldEdit],
        _receipt: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<u64, ScriptOperationFailure>> + Send + 'a>> {
        Box::pin(async { Err(ScriptOperationFailure::RuntimeUnavailable) })
    }

    fn preview_place(
        &self,
        _dimension: &str,
        _pos: [i32; 3],
        _state: u32,
    ) -> Result<ResidentWorldEdit, ScriptOperationFailure> {
        Err(ScriptOperationFailure::RuntimeUnavailable)
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
        ("structures/warehouse.toml".to_owned(), warehouse_toml()),
    ];
    BlueprintCatalog::from_files(&stub_registry(), "solaris", &files).unwrap()
}

/// The authored warehouse the resident deposit test binds: the shared building
/// shape plus two empty containers at `[1, 1, 1]` and `[2, 2, 2]`, which is what
/// `bind_warehouse` resolves a handle through.
fn warehouse_toml() -> String {
    let mut text = building_toml("warehouse", "work", 0, &[("body", &[([0, 1, 0], 0)])]);
    text.push_str(
        "[[block_entity]]\nat = [1, 1, 1]\nkind = \"empty_container\"\n\
         [[block_entity]]\nat = [2, 2, 2]\nkind = \"empty_container\"\n",
    );
    text
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
        let sessions = Arc::new(SessionRegistry::new());
        let world =
            Arc::new(TestWorld::default().journal_warehouse_transfers(Arc::clone(&sessions)));
        let runtime = InventoryRuntime::player_only_for_test(
            root.path(),
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
            None,
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

    /// The anchor every prepared structure of this fixture is placed at.
    fn anchor(&self) -> [i32; 3] {
        let candidate = self.candidate();
        [candidate.origin[0], ANCHOR_Y, candidate.origin[2]]
    }

    /// World position of the `offset`-th authored container of a rotation-zero
    /// warehouse placed at this fixture's anchor; the blueprint authors them at
    /// `[1, 1, 1]` and `[2, 2, 2]`.
    fn container_position(&self, offset: i32) -> [i32; 3] {
        let anchor = self.anchor();
        [anchor[0] + offset, anchor[1] + offset, anchor[2] + offset]
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
        self.resident_for(storage, OWNER, slot, position).await
    }

    async fn resident_for(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        slot: u32,
        position: Vec3,
    ) -> (String, uuid::Uuid) {
        let generation =
            resident_generation_id(WORLD_IDENTITY, "site-7", slot).expect("generation id");
        let snapshot = self
            .runtime
            .materialize_resident(storage, plugin_id, &generation, position)
            .await
            .expect("resident materialises");
        let handle =
            mc_script::resident_handle_for_generation(plugin_id, &generation).expect("handle");
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
        self.execute_for(storage, OWNER, request).await
    }

    async fn execute_for(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
    ) -> ScriptOperationOutcome {
        self.runtime
            .execute_resident_order_operation(storage, plugin_id, request)
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
        self.prepared_structure(storage, operation_id, COTTAGE)
            .await
    }

    /// Survey the fixture's first site and prepare one staged structure inside
    /// it.
    async fn prepared_structure(
        &self,
        storage: &mut PluginStorage,
        operation_id: &str,
        blueprint_id: &str,
    ) -> ScriptStructureSnapshot {
        self.prepared_structure_for(storage, OWNER, operation_id, blueprint_id)
            .await
    }

    async fn prepared_structure_for(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        operation_id: &str,
        blueprint_id: &str,
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
            .execute_settlement_operation(storage, plugin_id, &survey)
            .await
            .expect("survey reaches the durable boundary");
        let token = match outcome.payload() {
            ScriptOperationPayload::Settlement { result } => match &**result {
                mc_script::ScriptSettlementResult::Survey { survey } => survey.survey_token.clone(),
                other => panic!("expected a survey, got {other:?}"),
            },
            other => panic!("expected a settlement payload, got {other:?}"),
        };
        let anchor = self.anchor();
        let prepare = ScriptOperationRequest::try_new(
            "request",
            ScriptOperation::Settlement {
                operation: mc_script::ScriptSettlementOperation::PrepareStructure {
                    operation_id: operation_id.to_owned(),
                    blueprint_id: blueprint_id.to_owned(),
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
            .execute_settlement_operation(storage, plugin_id, &prepare)
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

/// The warehouse binding one `bind_warehouse` outcome carries.
fn warehouse_of(outcome: &ScriptOperationOutcome) -> mc_script::ScriptWarehouseBinding {
    let ScriptOperationPayload::Settlement { result } = outcome.payload() else {
        panic!("expected a settlement payload, got {outcome:?}");
    };
    match &**result {
        ScriptSettlementResult::Warehouse { binding } => binding.as_ref().clone(),
        other => panic!("expected a warehouse binding, got {other:?}"),
    }
}

fn bind_warehouse_request(
    operation_id: &str,
    structure_id: &str,
    container_id: u32,
) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::Settlement {
            operation: mc_script::ScriptSettlementOperation::BindWarehouse {
                operation_id: operation_id.to_owned(),
                structure_id: structure_id.to_owned(),
                container_id,
            },
        },
    )
    .expect("valid bind request")
}

fn warehouse_query_request(handle: &str) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::Inventory {
            operation: mc_script::ScriptOwnedInventoryOperation::Query {
                endpoint: ScriptInventoryEndpoint::Warehouse {
                    handle: handle.to_owned(),
                },
                expected_revision: None,
            },
        },
    )
    .expect("valid warehouse query")
}

/// The canonical snapshot one warehouse query answered with.
fn owned_snapshot_of(outcome: &ScriptOperationOutcome) -> mc_script::ScriptOwnedInventorySnapshot {
    let ScriptOperationPayload::OwnedInventory { result } = outcome.payload() else {
        panic!("expected an owned inventory payload, got {outcome:?}");
    };
    match &**result {
        mc_script::ScriptOwnedInventoryResult::Snapshot { inventory } => inventory.clone(),
        other => panic!("expected an owned inventory snapshot, got {other:?}"),
    }
}

/// Seed the worker's own canonical carry slots through the durable order
/// ledger, and answer the revision the next assignment must fence with.
fn seed_carry(
    storage: &mut PluginStorage,
    handle: &str,
    entity_uuid: uuid::Uuid,
    carry: &[(&str, u32)],
) -> u64 {
    seed_carry_for(storage, OWNER, handle, entity_uuid, carry)
}

fn seed_carry_for(
    storage: &mut PluginStorage,
    plugin_id: &str,
    handle: &str,
    entity_uuid: uuid::Uuid,
    carry: &[(&str, u32)],
) -> u64 {
    seed_gear(storage, plugin_id, handle, entity_uuid, carry, |record| {
        &mut record.carry
    })
}
fn crafting_station() -> ScriptWorkArea {
    ScriptWorkArea::new(
        "minecraft:overworld".to_owned(),
        ScriptBlockPosition::new(9, 64, 9),
        ScriptBlockPosition::new(9, 64, 9),
    )
}

fn seed_gear(
    storage: &mut PluginStorage,
    plugin_id: &str,
    handle: &str,
    entity_uuid: uuid::Uuid,
    gear: &[(&str, u32)],
    endpoint: fn(
        &mut super::resident_orders::DurableResidentOrderRecord,
    ) -> &mut Vec<Option<super::resident_orders::DurableResidentStack>>,
) -> u64 {
    // Gear is one field of the resident's durable record; a resident that has
    // not served an order yet gets the record every other ledger change uses.
    let mut record = storage
        .resident_orders()
        .record(handle)
        .cloned()
        .unwrap_or_else(|| {
            super::resident_orders::DurableResidentOrderRecord::empty(
                handle.to_owned(),
                plugin_id.to_owned(),
                entity_uuid.to_string(),
                super::resident_orders::DurableAssignment::Civilian,
            )
        });
    for (index, (item, count)) in gear.iter().enumerate() {
        endpoint(&mut record)[index] = Some(super::resident_orders::DurableResidentStack::new(
            (*item).to_owned(),
            *count,
        ));
    }
    storage
        .append_resident_order_change(super::resident_orders::DurableResidentOrderChange::Record {
            record: Box::new(record),
        })
        .expect("seeded cargo is durable")
}

/// The worker's canonical carry, as `(resource id, count)` pairs.
fn carry_of(storage: &PluginStorage, handle: &str) -> Vec<(String, u32)> {
    gear_of(storage, handle, false)
}

/// The worker's canonical equipment, as `(resource id, count)` pairs.
fn equipment_of(storage: &PluginStorage, handle: &str) -> Vec<(String, u32)> {
    gear_of(storage, handle, true)
}

fn gear_of(storage: &PluginStorage, handle: &str, equipment: bool) -> Vec<(String, u32)> {
    storage
        .resident_orders()
        .record(handle)
        .map(|record| {
            let slots = if equipment {
                &record.equipment
            } else {
                &record.carry
            };
            slots
                .iter()
                .flatten()
                .map(|stack| (stack.item_id.clone(), stack.count))
                .collect()
        })
        .unwrap_or_default()
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
/// (R1-B) A worker's haul into a bound warehouse container really moves its
/// cargo: the container's own slots and the worker's record change commit under
/// ONE journal decision, the worker's carry ends empty, and a warehouse query
/// reads the deposited items back from the container core committed them into.
#[tokio::test]
async fn worker_haul_deposits_its_cargo_into_the_bound_warehouse() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let structure = fixture
        .prepared_structure(&mut storage, "prepare-warehouse", WAREHOUSE)
        .await;
    // The container is loaded before the binding: a bind only accepts a
    // container core can really read.
    let position = fixture.container_position(1);
    let mut chest = vec![ItemStack::EMPTY; 27];
    chest[0] = ItemStack::new(
        solaris_required_items()
            .id_of(&Identifier::parse("minecraft:birch_log").unwrap())
            .expect("birch log is a required item"),
        12,
    );
    fixture.world.set_container(position, chest);
    let bind = fixture
        .runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-warehouse", &structure.structure_id, 0),
        )
        .await
        .expect("bind reaches the durable boundary");
    assert_eq!(bind.failure(), None, "bind: {bind:?}");
    let binding = warehouse_of(&bind);
    let revision = seed_carry(&mut storage, &handle, uuid, &[("minecraft:birch_log", 3)]);

    let haul = ScriptResidentWorkOrder::Haul {
        source: ScriptInventoryEndpoint::ResidentCarry {
            handle: handle.clone(),
        },
        destination: ScriptInventoryEndpoint::Warehouse {
            handle: binding.handle.clone(),
        },
        item: None,
    };
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request("haul-warehouse", &handle, haul, 4, revision),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, None, "{assignment:?}");
    assert_eq!(assignment.work_units_done, 3, "three units really moved");
    assert_eq!(assignment.state, ScriptWorkState::Running);
    assert_eq!(
        assignment
            .changes
            .iter()
            .map(|change| (change.item_id.as_str(), change.delta))
            .collect::<Vec<_>>(),
        vec![("minecraft:birch_log", 3)],
        "the receipt reports exactly what entered the container"
    );

    // The container took the cargo, merged into the stack it already held.
    let container = fixture.world.container(position);
    assert_eq!(container[0].count, 15);
    assert_eq!(container[1], ItemStack::EMPTY);
    assert!(carry_of(&storage, &handle).is_empty(), "the carry emptied");

    // The world half saw the container's observed and planned images, and the
    // deposit carries no player participant.
    let deposits = fixture.world.deposits();
    assert_eq!(deposits.len(), 1, "one deposit, one real move");
    assert_eq!(deposits[0].expected_container[0].count, 12);
    assert_eq!(deposits[0].updated_container[0].count, 15);
    assert!(deposits[0].player.is_none());

    // ONE decision carries the container and the worker's record, and the
    // plugin's own receipt is durable with it.
    let journal = fixture
        .sessions
        .world_chunk_journal()
        .expect("the fixture owns a journal");
    let pending = journal.pending_decisions_for_test();
    assert_eq!(pending.len(), 1, "one deposit, one decision");
    assert!(
        pending[0].inventory_batch().unwrap().is_some(),
        "the worker's record change rides the container's own decision"
    );
    assert!(storage.operation_receipt(OWNER, "haul-warehouse").is_some());

    // A warehouse query reads the committed container back.
    let read = fixture
        .runtime
        .execute_owned_inventory(
            &mut storage,
            OWNER,
            &warehouse_query_request(&binding.handle),
        )
        .await
        .expect("the query reaches the durable boundary");
    let snapshot = owned_snapshot_of(&read);
    let slot = snapshot.slots[0]
        .item
        .as_ref()
        .expect("the merged stack is in the container");
    assert_eq!(slot.resource_id, "minecraft:birch_log");
    assert_eq!(slot.count, 15);
}

/// (R1-B) A container with no room takes nothing: the worker keeps every item,
/// the record reports `no_storage`, and no decision is spent.
#[tokio::test]
async fn a_full_container_leaves_the_cargo_with_the_worker() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let structure = fixture
        .prepared_structure(&mut storage, "prepare-warehouse", WAREHOUSE)
        .await;
    let log = solaris_required_items()
        .id_of(&Identifier::parse("minecraft:birch_log").unwrap())
        .expect("birch log is a required item");
    // Every slot is full of the very item the worker hauls, so no slot offers
    // room and none can merge.
    let position = fixture.container_position(1);
    let full = vec![ItemStack::new(log, 64); 27];
    fixture.world.set_container(position, full.clone());
    let bind = fixture
        .runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-warehouse", &structure.structure_id, 0),
        )
        .await
        .expect("bind reaches the durable boundary");
    let binding = warehouse_of(&bind);
    let revision = seed_carry(&mut storage, &handle, uuid, &[("minecraft:birch_log", 3)]);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "haul-full",
                &handle,
                ScriptResidentWorkOrder::Haul {
                    source: ScriptInventoryEndpoint::ResidentCarry {
                        handle: handle.clone(),
                    },
                    destination: ScriptInventoryEndpoint::Warehouse {
                        handle: binding.handle.clone(),
                    },
                    item: None,
                },
                4,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::NoStorage));
    assert_eq!(assignment.work_units_done, 0);
    assert!(assignment.changes.is_empty());
    assert_eq!(
        carry_of(&storage, &handle),
        vec![("minecraft:birch_log".to_owned(), 3)],
        "the worker keeps its cargo"
    );
    assert_eq!(
        fixture.world.container(position),
        full,
        "the container is untouched"
    );
    assert!(
        fixture.world.deposits().is_empty(),
        "a container that cannot take the cargo is never asked to"
    );
    let journal = fixture
        .sessions
        .world_chunk_journal()
        .expect("the fixture owns a journal");
    assert!(
        journal.pending_decisions_for_test().is_empty(),
        "a refused deposit spends no decision"
    );
}

/// (R1-B, REC-02 shape) Replaying the same work operation after reopening the
/// durable storage deposits once: the stored receipt answers, and neither the
/// container nor the record moves again.
#[tokio::test]
async fn a_replayed_haul_deposits_once() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let structure = fixture
        .prepared_structure(&mut storage, "prepare-warehouse", WAREHOUSE)
        .await;
    let position = fixture.container_position(1);
    fixture
        .world
        .set_container(position, vec![ItemStack::EMPTY; 27]);
    let bind = fixture
        .runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-warehouse", &structure.structure_id, 0),
        )
        .await
        .expect("bind reaches the durable boundary");
    let binding = warehouse_of(&bind);
    let revision = seed_carry(&mut storage, &handle, uuid, &[("minecraft:birch_log", 3)]);

    let request = work_request(
        "haul-replay",
        &handle,
        ScriptResidentWorkOrder::Haul {
            source: ScriptInventoryEndpoint::ResidentCarry {
                handle: handle.clone(),
            },
            destination: ScriptInventoryEndpoint::Warehouse {
                handle: binding.handle.clone(),
            },
            item: None,
        },
        4,
        revision,
    );
    let first = fixture.execute(&mut storage, &request).await;
    assert_eq!(first.failure(), None, "first deposit: {first:?}");
    drop(storage);
    let mut reopened = fixture.storage();
    let replay = fixture.execute(&mut reopened, &request).await;
    assert_eq!(replay.failure(), None, "replay: {replay:?}");
    assert_eq!(
        work_of(&replay),
        work_of(&first),
        "the replay answers the stored receipt"
    );
    assert_eq!(
        fixture.world.deposits().len(),
        1,
        "one deposit, one real move"
    );
    assert_eq!(fixture.world.container(position)[0].count, 3);
    assert!(carry_of(&reopened, &handle).is_empty());
    assert_eq!(
        fixture
            .sessions
            .world_chunk_journal()
            .expect("the fixture owns a journal")
            .pending_decisions_for_test()
            .len(),
        1,
        "one deposit, one decision"
    );
}

/// (R1-B) A stack the container cannot take does not stall the rest of the
/// cargo: the worker deposits what fits, keeps what does not, and reports the
/// units it really moved.
#[tokio::test]
async fn a_haul_deposits_past_a_stack_the_container_refuses() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let structure = fixture
        .prepared_structure(&mut storage, "prepare-warehouse", WAREHOUSE)
        .await;
    let items = solaris_required_items();
    let stone = items
        .id_of(&Identifier::parse("minecraft:stone").unwrap())
        .expect("stone is a required item");
    let log = items
        .id_of(&Identifier::parse("minecraft:birch_log").unwrap())
        .expect("birch log is a required item");
    // Every slot is full of stone, except one that is four logs short of a full
    // birch-log stack: wheat fits nowhere, the logs still do.
    let position = fixture.container_position(1);
    let mut chest = vec![ItemStack::new(stone, 64); 27];
    chest[0] = ItemStack::new(log, 60);
    fixture.world.set_container(position, chest);
    let bind = fixture
        .runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-warehouse", &structure.structure_id, 0),
        )
        .await
        .expect("bind reaches the durable boundary");
    let binding = warehouse_of(&bind);
    let revision = seed_carry(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:wheat", 3), ("minecraft:birch_log", 3)],
    );

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "haul-mixed",
                &handle,
                ScriptResidentWorkOrder::Haul {
                    source: ScriptInventoryEndpoint::ResidentCarry {
                        handle: handle.clone(),
                    },
                    destination: ScriptInventoryEndpoint::Warehouse {
                        handle: binding.handle.clone(),
                    },
                    item: None,
                },
                8,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, None, "{assignment:?}");
    assert_eq!(
        assignment.work_units_done, 3,
        "the logs moved although the wheat could not"
    );
    assert_eq!(
        assignment
            .changes
            .iter()
            .map(|change| (change.item_id.as_str(), change.delta))
            .collect::<Vec<_>>(),
        vec![("minecraft:birch_log", 3)],
        "the receipt names only what entered the container"
    );
    let container = fixture.world.container(position);
    assert_eq!(container[0].count, 63);
    assert_eq!(
        carry_of(&storage, &handle),
        vec![("minecraft:wheat".to_owned(), 3)],
        "the stack that fit nowhere stays with the worker"
    );
}

/// (CP-013) A full named warehouse pauses a haul until that exact destination
/// changes; a different inventory event cannot scan or wake it.
#[tokio::test]
async fn warehouse_change_resumes_the_same_paused_haul_once() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let structure = fixture
        .prepared_structure(&mut storage, "prepare-warehouse", WAREHOUSE)
        .await;
    let stone = solaris_required_items()
        .id_of(&Identifier::parse("minecraft:stone").unwrap())
        .expect("stone is a required item");
    let position = fixture.container_position(1);
    fixture
        .world
        .set_container(position, vec![ItemStack::new(stone, 64); 27]);
    let binding = warehouse_of(
        &fixture
            .runtime
            .execute_settlement_operation(
                &mut storage,
                OWNER,
                &bind_warehouse_request("bind-full-warehouse", &structure.structure_id, 0),
            )
            .await
            .expect("full warehouse binding reaches the durable boundary"),
    );
    let revision = seed_carry(&mut storage, &handle, uuid, &[("minecraft:birch_log", 1)]);
    let endpoint = ScriptInventoryEndpoint::Warehouse {
        handle: binding.handle.clone(),
    };
    let paused = fixture
        .execute(
            &mut storage,
            &work_request(
                "pause-full-warehouse-haul",
                &handle,
                ScriptResidentWorkOrder::Haul {
                    source: ScriptInventoryEndpoint::ResidentCarry {
                        handle: handle.clone(),
                    },
                    destination: endpoint.clone(),
                    item: None,
                },
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(
        work_of(&paused).reason,
        Some(ScriptWorkPauseReason::NoStorage)
    );
    assert!(
        fixture
            .runtime
            .resume_paused_work_for_inventory_change(&mut storage, &[], &[], |_| true)
            .await
            .expect("unrelated inventory event stays bounded")
            .is_empty()
    );

    fixture
        .world
        .set_container(position, vec![ItemStack::EMPTY; 27]);
    let resumed = fixture
        .runtime
        .resume_paused_work_for_inventory_change(
            &mut storage,
            &[],
            std::slice::from_ref(&endpoint),
            |_| true,
        )
        .await
        .expect("matching warehouse event resumes haul");
    assert_eq!(resumed.len(), 1);
    assert_eq!(
        work_of(&resumed[0].outcome).state,
        ScriptWorkState::Committed
    );
    assert_eq!(fixture.world.container(position)[0].count, 1);
    assert!(
        fixture
            .runtime
            .resume_paused_work_for_inventory_change(
                &mut storage,
                &[],
                std::slice::from_ref(&endpoint),
                |_| true,
            )
            .await
            .expect("completed haul cannot loop")
            .is_empty()
    );
}

/// (CP-003) A worker issued an item out of a bound warehouse really takes that
/// item: the named resource is the one that moves even when it sits behind
/// another stack, the container's own slots and the worker's record commit
/// under ONE journal decision, and the assignment reports the units the
/// container gave up - consumed, not produced.
#[tokio::test]
async fn worker_withdraws_the_named_item_from_the_bound_warehouse() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let structure = fixture
        .prepared_structure(&mut storage, "prepare-warehouse", WAREHOUSE)
        .await;
    let items = solaris_required_items();
    let axe = items
        .id_of(&Identifier::parse("minecraft:iron_axe").unwrap())
        .expect("an iron axe is a required item");
    let log = items
        .id_of(&Identifier::parse("minecraft:birch_log").unwrap())
        .expect("birch log is a required item");
    // The item the worker was issued sits behind a stack it did not ask for:
    // storage order alone would hand it the logs.
    let position = fixture.container_position(1);
    let mut chest = vec![ItemStack::EMPTY; 27];
    chest[0] = ItemStack::new(log, 10);
    chest[1] = ItemStack::new(axe, 5);
    fixture.world.set_container(position, chest);
    let bind = fixture
        .runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-warehouse", &structure.structure_id, 0),
        )
        .await
        .expect("bind reaches the durable boundary");
    assert_eq!(bind.failure(), None, "bind: {bind:?}");
    let binding = warehouse_of(&bind);
    let revision = seed_carry(&mut storage, &handle, uuid, &[]);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "withdraw-warehouse",
                &handle,
                ScriptResidentWorkOrder::Haul {
                    source: ScriptInventoryEndpoint::Warehouse {
                        handle: binding.handle.clone(),
                    },
                    destination: ScriptInventoryEndpoint::ResidentCarry {
                        handle: handle.clone(),
                    },
                    item: Some("minecraft:iron_axe".to_owned()),
                },
                2,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, None, "{assignment:?}");
    assert_eq!(assignment.work_units_done, 2, "two units really moved");
    assert_eq!(
        assignment.state,
        ScriptWorkState::Committed,
        "the whole plan is done"
    );
    assert_eq!(
        assignment
            .changes
            .iter()
            .map(|change| (change.item_id.as_str(), change.delta))
            .collect::<Vec<_>>(),
        vec![("minecraft:iron_axe", -2)],
        "the receipt reports what left the container"
    );
    assert_eq!(
        carry_of(&storage, &handle),
        vec![("minecraft:iron_axe".to_owned(), 1); 2],
        "the worker holds the item it was issued, and only it - one slot per axe, \
         because a tool does not stack"
    );

    let container = fixture.world.container(position);
    assert_eq!(container[0].count, 10, "the stack it did not ask for stays");
    assert_eq!(container[1].count, 3, "the container gave up two axes");

    // The container half fences the observed and planned images, and the move
    // carries no player participant.
    let transfers = fixture.world.deposits();
    assert_eq!(transfers.len(), 1, "one transfer, one real move");
    assert_eq!(transfers[0].expected_container[1].count, 5);
    assert_eq!(transfers[0].updated_container[1].count, 3);
    assert!(transfers[0].player.is_none());

    // ONE decision carries the container and the worker's record, and the
    // plugin's own receipt is durable with it.
    let journal = fixture
        .sessions
        .world_chunk_journal()
        .expect("the fixture owns a journal");
    let pending = journal.pending_decisions_for_test();
    assert_eq!(pending.len(), 1, "one transfer, one decision");
    assert!(
        pending[0].inventory_batch().unwrap().is_some(),
        "the worker's record change rides the container's own decision"
    );
    assert!(
        storage
            .operation_receipt(OWNER, "withdraw-warehouse")
            .is_some()
    );

    // A warehouse query reads the container the worker was issued from.
    let read = fixture
        .runtime
        .execute_owned_inventory(
            &mut storage,
            OWNER,
            &warehouse_query_request(&binding.handle),
        )
        .await
        .expect("the query reaches the durable boundary");
    let snapshot = owned_snapshot_of(&read);
    let slot = snapshot.slots[1]
        .item
        .as_ref()
        .expect("the remaining axes are still in the container");
    assert_eq!(slot.resource_id, "minecraft:iron_axe");
    assert_eq!(slot.count, 3);
}

/// (CP-003) The same withdrawal can land in the worker's own equipment: the
/// issued tool reaches the endpoint a worker really uses it from.
#[tokio::test]
async fn worker_withdraws_into_its_own_equipment() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let structure = fixture
        .prepared_structure(&mut storage, "prepare-warehouse", WAREHOUSE)
        .await;
    let hoe = solaris_required_items()
        .id_of(&Identifier::parse("minecraft:iron_hoe").unwrap())
        .expect("an iron hoe is a required item");
    let position = fixture.container_position(1);
    let mut chest = vec![ItemStack::EMPTY; 27];
    chest[0] = ItemStack::new(hoe, 4);
    fixture.world.set_container(position, chest);
    let bind = fixture
        .runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-warehouse", &structure.structure_id, 0),
        )
        .await
        .expect("bind reaches the durable boundary");
    let binding = warehouse_of(&bind);
    let revision = seed_carry(&mut storage, &handle, uuid, &[]);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "withdraw-equipment",
                &handle,
                ScriptResidentWorkOrder::Haul {
                    source: ScriptInventoryEndpoint::Warehouse {
                        handle: binding.handle.clone(),
                    },
                    destination: ScriptInventoryEndpoint::ResidentEquipment {
                        handle: handle.clone(),
                    },
                    item: Some("minecraft:iron_hoe".to_owned()),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, None, "{assignment:?}");
    assert_eq!(assignment.work_units_done, 1);
    assert_eq!(
        assignment
            .changes
            .iter()
            .map(|change| (change.item_id.as_str(), change.delta))
            .collect::<Vec<_>>(),
        vec![("minecraft:iron_hoe", -1)]
    );
    assert_eq!(
        equipment_of(&storage, &handle),
        vec![("minecraft:iron_hoe".to_owned(), 1)],
        "the issued hoe is in the worker's equipment"
    );
    assert!(carry_of(&storage, &handle).is_empty());
    assert_eq!(fixture.world.container(position)[0].count, 3);
}

/// (CP-003) An item the container does not hold stops the job with
/// `missing_input`: nothing moves, the worker's record is untouched, and no
/// decision is spent.
#[tokio::test]
async fn a_withdrawal_of_an_item_the_container_does_not_hold_pauses_with_missing_input() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let structure = fixture
        .prepared_structure(&mut storage, "prepare-warehouse", WAREHOUSE)
        .await;
    let log = solaris_required_items()
        .id_of(&Identifier::parse("minecraft:birch_log").unwrap())
        .expect("birch log is a required item");
    let position = fixture.container_position(1);
    let chest = vec![
        ItemStack::new(log, 10),
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
    ];
    fixture.world.set_container(position, chest.clone());
    let bind = fixture
        .runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-warehouse", &structure.structure_id, 0),
        )
        .await
        .expect("bind reaches the durable boundary");
    let binding = warehouse_of(&bind);
    let revision = seed_carry(&mut storage, &handle, uuid, &[]);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "withdraw-absent",
                &handle,
                ScriptResidentWorkOrder::Haul {
                    source: ScriptInventoryEndpoint::Warehouse {
                        handle: binding.handle.clone(),
                    },
                    destination: ScriptInventoryEndpoint::ResidentCarry {
                        handle: handle.clone(),
                    },
                    item: Some("minecraft:iron_axe".to_owned()),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::MissingInput));
    assert_eq!(assignment.work_units_done, 0);
    assert!(assignment.changes.is_empty());
    assert!(carry_of(&storage, &handle).is_empty());
    assert_eq!(
        fixture.world.container(position),
        chest,
        "the container is untouched"
    );
    assert!(
        fixture.world.deposits().is_empty(),
        "a warehouse that holds nothing to issue is never asked to move anything"
    );
    assert!(
        fixture
            .sessions
            .world_chunk_journal()
            .expect("the fixture owns a journal")
            .pending_decisions_for_test()
            .is_empty(),
        "a refused withdrawal spends no decision"
    );
}

/// (CP-003) A worker with no room left for the issued item pauses the step as
/// interrupted - the item stays in the container until the worker has room -
/// rather than reporting storage that is really there.
#[tokio::test]
async fn a_withdrawal_into_a_full_carry_leaves_the_warehouse_untouched() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let structure = fixture
        .prepared_structure(&mut storage, "prepare-warehouse", WAREHOUSE)
        .await;
    let axe = solaris_required_items()
        .id_of(&Identifier::parse("minecraft:iron_axe").unwrap())
        .expect("an iron axe is a required item");
    let position = fixture.container_position(1);
    let mut chest = vec![ItemStack::EMPTY; 27];
    chest[0] = ItemStack::new(axe, 5);
    fixture.world.set_container(position, chest.clone());
    let bind = fixture
        .runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-warehouse", &structure.structure_id, 0),
        )
        .await
        .expect("bind reaches the durable boundary");
    let binding = warehouse_of(&bind);
    // Every carry slot is full of another item the axe cannot merge into.
    let revision = seed_carry(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:birch_log", 64); 8],
    );

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "withdraw-full",
                &handle,
                ScriptResidentWorkOrder::Haul {
                    source: ScriptInventoryEndpoint::Warehouse {
                        handle: binding.handle.clone(),
                    },
                    destination: ScriptInventoryEndpoint::ResidentCarry {
                        handle: handle.clone(),
                    },
                    item: Some("minecraft:iron_axe".to_owned()),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::Interrupted));
    assert_eq!(assignment.work_units_done, 0);
    assert_eq!(
        fixture.world.container(position),
        chest,
        "the warehouse keeps the item it could not hand over"
    );
    assert_eq!(
        carry_of(&storage, &handle),
        vec![("minecraft:birch_log".to_owned(), 64); 8],
        "the worker's own slots are exactly as they were"
    );
    assert!(fixture.world.deposits().is_empty());
    assert!(
        fixture
            .sessions
            .world_chunk_journal()
            .expect("the fixture owns a journal")
            .pending_decisions_for_test()
            .is_empty(),
        "a step with nowhere to put the item spends no decision"
    );
}

/// (CP-003) A handle core cannot resolve is `no_storage`, not a silent move:
/// the worker keeps its hands off a warehouse it cannot see.
#[tokio::test]
async fn a_withdrawal_through_an_unknown_handle_pauses_with_no_storage() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let position = fixture.container_position(1);
    let mut chest = vec![ItemStack::EMPTY; 27];
    chest[0] = ItemStack::new(
        solaris_required_items()
            .id_of(&Identifier::parse("minecraft:iron_axe").unwrap())
            .expect("an iron axe is a required item"),
        5,
    );
    fixture.world.set_container(position, chest.clone());
    let revision = seed_carry(&mut storage, &handle, uuid, &[]);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "withdraw-unknown",
                &handle,
                ScriptResidentWorkOrder::Haul {
                    source: ScriptInventoryEndpoint::Warehouse {
                        handle: "warehouse:settlement:nobody:0".to_owned(),
                    },
                    destination: ScriptInventoryEndpoint::ResidentCarry {
                        handle: handle.clone(),
                    },
                    item: Some("minecraft:iron_axe".to_owned()),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::NoStorage));
    assert_eq!(assignment.work_units_done, 0);
    assert!(carry_of(&storage, &handle).is_empty());
    assert_eq!(fixture.world.container(position), chest);
    assert!(fixture.world.deposits().is_empty());
    // The pause itself is durable: a worker that could not see the warehouse
    // stays paused with the storage reason until the next assignment.
    let record = storage
        .resident_orders()
        .record(&handle)
        .cloned()
        .expect("the worker's record is durable");
    let work = record.work.expect("the paused assignment is durable");
    assert_eq!(work.reason, Some(ScriptWorkPauseReason::NoStorage));
    assert_eq!(work.done, 0);
    assert_eq!(work.state, ScriptWorkState::Paused);
}

/// (CP-003, REC-02 shape) Replaying the same withdrawal takes the item once:
/// the stored receipt answers, and neither the container nor the record moves
/// again.
#[tokio::test]
async fn a_replayed_withdrawal_takes_the_item_once() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let structure = fixture
        .prepared_structure(&mut storage, "prepare-warehouse", WAREHOUSE)
        .await;
    let axe = solaris_required_items()
        .id_of(&Identifier::parse("minecraft:iron_axe").unwrap())
        .expect("an iron axe is a required item");
    let position = fixture.container_position(1);
    let mut chest = vec![ItemStack::EMPTY; 27];
    chest[0] = ItemStack::new(axe, 5);
    fixture.world.set_container(position, chest);
    let bind = fixture
        .runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-warehouse", &structure.structure_id, 0),
        )
        .await
        .expect("bind reaches the durable boundary");
    let binding = warehouse_of(&bind);
    let revision = seed_carry(&mut storage, &handle, uuid, &[]);

    let request = work_request(
        "withdraw-replay",
        &handle,
        ScriptResidentWorkOrder::Haul {
            source: ScriptInventoryEndpoint::Warehouse {
                handle: binding.handle.clone(),
            },
            destination: ScriptInventoryEndpoint::ResidentCarry {
                handle: handle.clone(),
            },
            item: Some("minecraft:iron_axe".to_owned()),
        },
        2,
        revision,
    );
    let first = fixture.execute(&mut storage, &request).await;
    assert_eq!(first.failure(), None, "first withdrawal: {first:?}");
    let replay = fixture.execute(&mut storage, &request).await;
    assert_eq!(replay.failure(), None, "replay: {replay:?}");
    assert_eq!(
        work_of(&replay),
        work_of(&first),
        "the replay answers the stored receipt"
    );
    assert_eq!(
        fixture.world.deposits().len(),
        1,
        "one transfer, one real move"
    );
    assert_eq!(fixture.world.container(position)[0].count, 3);
    assert_eq!(
        carry_of(&storage, &handle),
        vec![("minecraft:iron_axe".to_owned(), 1); 2]
    );
}

/// (CP-012) A workshop chain withdraws warehouse stock, uses the named loaded
/// station, and returns the exact recipe output without replaying the craft.
#[tokio::test]
async fn workshop_returns_warehouse_inputs_as_one_real_recipe_output() {
    let fixture = Fixture::new();
    fixture
        .world
        .set_resident_block([9, 64, 9], "crafting_table");
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let structure = fixture
        .prepared_structure(&mut storage, "prepare-workshop-warehouse", WAREHOUSE)
        .await;
    let position = fixture.container_position(1);
    let items = solaris_required_items();
    let oak_log = items
        .id_of(&Identifier::parse("minecraft:oak_log").unwrap())
        .expect("oak log is a required item");
    let oak_planks = items
        .id_of(&Identifier::parse("minecraft:oak_planks").unwrap())
        .expect("oak planks are a required item");
    let mut chest = vec![ItemStack::EMPTY; 27];
    chest[0] = ItemStack::new(oak_log, 2);
    fixture.world.set_container(position, chest);
    let bind = fixture
        .runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-workshop-warehouse", &structure.structure_id, 0),
        )
        .await
        .expect("warehouse binding reaches the durable boundary");
    assert_eq!(bind.failure(), None, "{bind:?}");
    let binding = warehouse_of(&bind);
    let revision = seed_carry(&mut storage, &handle, uuid, &[]);

    let withdrawn = fixture
        .execute(
            &mut storage,
            &work_request(
                "withdraw-workshop-log",
                &handle,
                ScriptResidentWorkOrder::Haul {
                    source: ScriptInventoryEndpoint::Warehouse {
                        handle: binding.handle.clone(),
                    },
                    destination: ScriptInventoryEndpoint::ResidentCarry {
                        handle: handle.clone(),
                    },
                    item: Some("minecraft:oak_log".to_owned()),
                },
                1,
                revision,
            ),
        )
        .await;
    let withdrawn = work_of(&withdrawn);
    assert_eq!(withdrawn.state, ScriptWorkState::Committed, "{withdrawn:?}");
    assert_eq!(withdrawn.work_units_done, 1);

    let craft_request = work_request(
        "craft-workshop-planks",
        &handle,
        ScriptResidentWorkOrder::Craft {
            recipe: "minecraft:oak_planks".to_owned(),
            count: 1,
            station: crafting_station(),
        },
        1,
        withdrawn.revision,
    );
    let crafted = fixture.execute(&mut storage, &craft_request).await;
    let crafted = work_of(&crafted);
    assert_eq!(crafted.state, ScriptWorkState::Committed, "{crafted:?}");
    assert_eq!(crafted.work_units_done, 1);
    assert_eq!(
        crafted
            .changes
            .iter()
            .map(|change| (change.item_id.as_str(), change.delta))
            .collect::<BTreeMap<_, _>>(),
        BTreeMap::from([("minecraft:oak_log", -1), ("minecraft:oak_planks", 4)]),
    );
    let replay = fixture.execute(&mut storage, &craft_request).await;
    assert_eq!(
        work_of(&replay),
        crafted,
        "the accepted craft receipt replays without rolling new output"
    );

    let returned = fixture
        .execute(
            &mut storage,
            &work_request(
                "return-workshop-planks",
                &handle,
                ScriptResidentWorkOrder::Haul {
                    source: ScriptInventoryEndpoint::ResidentCarry {
                        handle: handle.clone(),
                    },
                    destination: ScriptInventoryEndpoint::Warehouse {
                        handle: binding.handle,
                    },
                    item: Some("minecraft:oak_planks".to_owned()),
                },
                4,
                crafted.revision,
            ),
        )
        .await;
    let returned = work_of(&returned);
    assert_eq!(returned.state, ScriptWorkState::Committed, "{returned:?}");
    assert_eq!(returned.work_units_done, 4);
    let container = fixture.world.container(position);
    assert_eq!(
        container
            .iter()
            .filter(|stack| stack.item_id == oak_log)
            .map(|stack| stack.count)
            .sum::<i32>(),
        1,
        "one actual warehouse log was consumed"
    );
    assert_eq!(
        container
            .iter()
            .filter(|stack| stack.item_id == oak_planks)
            .map(|stack| stack.count)
            .sum::<i32>(),
        4,
        "the output returns to the bound warehouse"
    );
}
