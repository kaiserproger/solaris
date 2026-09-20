//! Acceptance tests for durable settlement sites, surveys, and construction (C2).
//!
//! Every test drives the real settlement runtime over a real plugin storage
//! journal, the real durable reservation receipt path, and an in-memory
//! [`FakeWorld`]; nothing asserts plumbing.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mc_data::Identifier;
use mc_data::ItemStack;
use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_data::item_components::solaris_required_item_facts;
use mc_data::items::solaris_required_items;
use mc_script::{
    ScriptChunkAvailability, ScriptInventoryEndpoint, ScriptInventoryExpectedRevision,
    ScriptInventoryFence, ScriptInventoryMaterial, ScriptInventoryReservationQuantity,
    ScriptInventoryReservationSnapshot, ScriptInventoryResourcePlan, ScriptInventoryWorkPortion,
    ScriptOperation, ScriptOperationFailure, ScriptOperationOutcome, ScriptOperationPayload,
    ScriptOperationRequest, ScriptOwnedInventoryOperation, ScriptOwnedInventoryResult,
    ScriptOwnedItemTransfer, ScriptResidentSiteReservation, ScriptSettlementBuilding,
    ScriptSettlementOperation, ScriptSettlementResult, ScriptSettlementSite, ScriptSitePoiKind,
    ScriptSitePoiState, ScriptSiteProvenance, ScriptSiteVariant, ScriptStructureReceipt,
    ScriptStructureSnapshot, ScriptStructureState, ScriptSurveyBounds, ScriptSurveyPurpose,
    ScriptWarehouseBinding, ScriptWarehouseSource, resident_generation_id,
};
use mc_world::BlockRegistry;
use mc_worldgen::{BlueprintCatalog, SettlementSelector};

use crate::play::SessionRegistry;
use crate::play::owned_inventory::{
    WarehouseTransferRequest, resource_plan_hash, resource_plan_totals,
};
use crate::server::ShutdownHandle;

use super::PluginStorage;
use super::PreparedStorageBatch;
use super::ScriptStoragePrepareOutcome;
use super::StorageFaultPoint;
use super::settlement::{
    ContainerReading, SettlementRuntime, SettlementWorld, StructureBlockPlacement, SurveyReading,
    VillageInhabitantReading, VillagePoiReading, VillageReading, journal_test_plugin_decision,
};
use super::world_inventory::InventoryRuntime;

const OWNER: &str = "settlement";
const FOREIGN: &str = "other-plugin";
const WORLD_IDENTITY: &str = "world-identity";
const SEED: i64 = 7;
const PROFILE_REVISION: u64 = 3;
const START_CELL: [i32; 2] = [0, 0];
const ANCHOR_Y: i32 = 64;
/// The staged structure the tests build.
const COTTAGE: &str = "solaris:cottage";
/// The authored-container structure the warehouse tests bind.
const WAREHOUSE: &str = "solaris:warehouse";
/// The player endpoint the test reservations are held against.
const PLAYER: u64 = 7;

/// Deterministic world fake: a revision, one pending footprint change, a claim
/// flag, a chunk availability, a highest opaque block, and the blocks
/// every committed portion applied.
struct FakeWorld {
    revision: AtomicU64,
    changed_at: Mutex<Option<u64>>,
    claimed: AtomicBool,
    availability: Mutex<ScriptChunkAvailability>,
    opaque_y: Mutex<Option<i32>>,
    applied: Mutex<BTreeMap<String, Vec<[i32; 3]>>>,
    containers: Mutex<BTreeMap<[i32; 3], Vec<ItemStack>>>,
    /// The server-owned deposits this fake's world half accepted, so a test can
    /// assert the plan the composite committed.
    warehouse_requests: Mutex<Vec<WarehouseTransferRequest>>,
    /// The runtime's own registry, so a warehouse deposit journals its decision
    /// here exactly like the live owner turn; absent in fixtures that own no
    /// journal, where a deposit answers `runtime_unavailable`.
    warehouse: Option<Arc<SessionRegistry>>,
}

impl FakeWorld {
    fn new() -> Self {
        Self {
            revision: AtomicU64::new(1),
            changed_at: Mutex::new(None),
            claimed: AtomicBool::new(false),
            availability: Mutex::new(ScriptChunkAvailability::Loaded),
            // Nothing blocks a footprint unless a test says so.
            opaque_y: Mutex::new(Some(i32::MIN)),
            applied: Mutex::new(BTreeMap::new()),
            containers: Mutex::new(BTreeMap::new()),
            warehouse_requests: Mutex::new(Vec::new()),
            warehouse: None,
        }
    }

    /// Let this fake stand in for the server-owned composite's world half: a
    /// warehouse deposit journals its encoded receipt on the fixture's journal.
    fn journal_warehouse_transfers(mut self, sessions: Arc<SessionRegistry>) -> Self {
        self.warehouse = Some(sessions);
        self
    }

    /// Set the highest opaque block every footprint reports; `None` means the
    /// footprint is not fully loaded.
    fn set_opaque_y(&self, y: Option<i32>) {
        *self.opaque_y.lock().unwrap() = y;
    }

    /// Record one change inside every footprint at a fresh revision.
    fn mark_change(&self) {
        let revision = self.revision.fetch_add(1, Ordering::Relaxed) + 1;
        *self.changed_at.lock().unwrap() = Some(revision);
    }

    fn set_claimed(&self, claimed: bool) {
        self.claimed.store(claimed, Ordering::Relaxed);
    }

    fn set_availability(&self, availability: ScriptChunkAvailability) {
        *self.availability.lock().unwrap() = availability;
    }

    fn built_blocks(&self, structure_id: &str) -> usize {
        self.applied
            .lock()
            .unwrap()
            .get(structure_id)
            .map_or(0, Vec::len)
    }

    fn built_total(&self) -> usize {
        self.applied.lock().unwrap().values().map(Vec::len).sum()
    }

    /// Seed one loaded container at a world position; the next warehouse read
    /// of a binding that resolves there reads exactly these items.
    fn set_container(&self, position: [i32; 3], items: Vec<ItemStack>) {
        self.containers.lock().unwrap().insert(position, items);
    }

    /// The last server-owned deposit this world half accepted.
    fn last_warehouse_request(&self) -> WarehouseTransferRequest {
        self.warehouse_requests
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("the world half accepted a deposit")
    }

    /// Remove the container at a position: loaded terrain that no longer holds
    /// a container block entity.
    fn clear_container(&self, position: [i32; 3]) {
        self.containers.lock().unwrap().remove(&position);
    }
}

impl SettlementWorld for FakeWorld {
    fn survey(
        &self,
        _plugin_id: &str,
        _dimension: &str,
        bounds: ScriptSurveyBounds,
    ) -> Result<SurveyReading, ScriptOperationFailure> {
        let x = u64::try_from(bounds.max[0] - bounds.min[0] + 1).unwrap_or(0);
        let z = u64::try_from(bounds.max[2] - bounds.min[2] + 1).unwrap_or(0);
        let count = u32::try_from(x * z).unwrap_or(u32::MAX);
        Ok(SurveyReading {
            usable_plots: count,
            water_columns: 0,
            claimed: self.claimed.load(Ordering::Relaxed),
            existing_structures: 0,
            biome_tags: vec!["minecraft:plains".to_owned()],
            resource_tags: Vec::new(),
            chunk_availability: *self.availability.lock().unwrap(),
            revision: self.revision.load(Ordering::Relaxed),
        })
    }

    /// A fake world holds no generated village of its own: the settlement tests
    /// exercise the authored lane, and a generated site is supplied by
    /// [`FakeVillageGround`]. What it does hold is the marker record of the
    /// inhabitant the generated village placed, which is what the descriptor
    /// reports.
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
        Ok(VillageReading::Loaded(vec![VillageInhabitantReading {
            claim: VILLAGE_INHABITANT_CLAIM.to_owned(),
            entity_uuid: crate::settlement_identity::settlement_entity_uuid(
                VILLAGE_INHABITANT_CLAIM,
            )
            .to_string(),
            position: [48.5, 64.0, 80.5],
            age: 0,
        }]))
    }

    fn observe_footprint(&self, _bounds: ScriptSurveyBounds) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

    fn predict_structure_portion_revision(
        &self,
        _bounds: ScriptSurveyBounds,
        _blocks: &[StructureBlockPlacement],
    ) -> Result<u64, ScriptOperationFailure> {
        Ok(self.revision.load(Ordering::Relaxed).saturating_add(1))
    }

    fn footprint_changed_since(&self, _bounds: ScriptSurveyBounds, revision: u64) -> bool {
        self.changed_at
            .lock()
            .unwrap()
            .is_some_and(|changed| changed > revision)
    }

    fn claims_overlap(&self, _plugin_id: &str, _bounds: ScriptSurveyBounds) -> bool {
        self.claimed.load(Ordering::Relaxed)
    }

    fn max_opaque_y(
        &self,
        _bounds: ScriptSurveyBounds,
    ) -> Result<Option<i32>, ScriptOperationFailure> {
        Ok(*self.opaque_y.lock().unwrap())
    }

    fn container_reading(
        &self,
        position: [i32; 3],
    ) -> Result<ContainerReading, ScriptOperationFailure> {
        if *self.availability.lock().unwrap() != ScriptChunkAvailability::Loaded {
            return Ok(ContainerReading::Unloaded);
        }
        Ok(match self.containers.lock().unwrap().get(&position) {
            Some(items) => ContainerReading::Loaded(items.clone()),
            None => ContainerReading::Missing,
        })
    }

    fn village_containers(
        &self,
        bounds: ScriptSurveyBounds,
    ) -> Result<VillageReading<[i32; 3]>, ScriptOperationFailure> {
        if *self.availability.lock().unwrap() != ScriptChunkAvailability::Loaded {
            return Ok(VillageReading::Unloaded);
        }
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
            let Some(sessions) = self.warehouse.as_ref() else {
                return Err(ScriptOperationFailure::RuntimeUnavailable);
            };
            let decision_id = journal_test_plugin_decision(sessions, request.receipt.clone())?;
            self.containers.lock().unwrap().insert(
                [request.position.x, request.position.y, request.position.z],
                request.updated_container.clone(),
            );
            self.warehouse_requests.lock().unwrap().push(request);
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
            let Some(sessions) = self.warehouse.as_ref() else {
                return Err(ScriptOperationFailure::RuntimeUnavailable);
            };
            let decision_id = journal_test_plugin_decision(sessions, receipt)?;
            self.applied
                .lock()
                .unwrap()
                .entry(structure_id.to_owned())
                .or_default()
                .extend(blocks.iter().map(|block| block.pos));
            // A portion is a durable world commit, so it advances the world's
            // revision exactly like the live adapter's journal decision does.
            self.mark_change();
            Ok(decision_id)
        })
    }
}

/// A stub block registry holding the two palette blocks the catalog authoring
/// uses, mirroring the worldgen settlement tests.
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
            building_toml("tower", "guard", 0, &[]),
        ),
        ("structures/warehouse.toml".to_owned(), warehouse_toml()),
    ];
    BlueprintCatalog::from_files(&stub_registry(), "solaris", &files).unwrap()
}

/// One authored structure carrying two `empty_container` seeds; the warehouse
/// tests bind by their authored ordinal, never by coordinates.
fn warehouse_toml() -> String {
    let mut text = building_toml("warehouse", "work", 0, &[("body", &[([0, 1, 0], 0)])]);
    text.push_str(
        "[[block_entity]]\nat = [1, 1, 1]\nkind = \"empty_container\"\n\
         [[block_entity]]\nat = [2, 2, 2]\nkind = \"empty_container\"\n",
    );
    text
}

/// Flat ground for the placement rules the settlement tests exercise.
struct FlatGround;

impl mc_world::ChunkGenerator for FlatGround {
    fn generate(&self, _pos: mc_world::ChunkPos) -> mc_world::Chunk {
        panic!("the settlement tests never ask a generator for a chunk")
    }

    fn surface_height(&self, _world_x: i32, _world_z: i32) -> Option<i32> {
        Some(63)
    }
}

/// Build one settlement runtime and its world fake.
fn runtime_at(
    catalog: Arc<BlueprintCatalog>,
    world: Arc<FakeWorld>,
    selector: bool,
    adapter: bool,
) -> InventoryRuntime {
    runtime_at_with(
        catalog,
        world,
        Arc::new(SessionRegistry::new()),
        None,
        selector,
        adapter,
    )
}

/// A runtime over an explicit registry, optionally owning a real journal: a
/// fixture whose world half journals decisions needs both to be the same ones.
fn runtime_at_with(
    catalog: Arc<BlueprintCatalog>,
    world: Arc<FakeWorld>,
    sessions: Arc<SessionRegistry>,
    root: Option<&std::path::Path>,
    selector: bool,
    adapter: bool,
) -> InventoryRuntime {
    let items = Arc::new(solaris_required_items());
    let item_facts = Arc::new(solaris_required_item_facts());
    let mut runtime = match root {
        Some(root) => {
            InventoryRuntime::player_only_for_test(root, Arc::clone(&sessions), items, item_facts)
        }
        None => InventoryRuntime::new(
            None,
            &ShutdownHandle::default(),
            Arc::clone(&sessions),
            items,
            item_facts,
        ),
    };
    if selector {
        runtime = runtime.with_settlement_runtime(Arc::new(SettlementRuntime::new(
            SettlementSelector::new(SEED, PROFILE_REVISION),
            Arc::clone(&catalog),
            WORLD_IDENTITY,
            START_CELL,
            Arc::new(FlatGround),
            None,
        )));
    }
    if adapter {
        runtime = runtime.with_settlement_world(world as Arc<dyn SettlementWorld>);
    }
    runtime
}

struct Fixture {
    root: tempfile::TempDir,
    runtime: InventoryRuntime,
    world: Arc<FakeWorld>,
    catalog: Arc<BlueprintCatalog>,
    /// The runtime's registry when this fixture owns a journal.
    sessions: Option<Arc<SessionRegistry>>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_journal()
    }

    /// A fixture with neither the settlement selector nor a world adapter.
    fn bare() -> Self {
        Self::build(false, false)
    }

    fn without_world() -> Self {
        Self::build(true, false)
    }

    fn build(selector: bool, adapter: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let catalog = Arc::new(catalog());
        let world = Arc::new(FakeWorld::new());
        let runtime = runtime_at(Arc::clone(&catalog), Arc::clone(&world), selector, adapter);
        Self {
            root,
            runtime,
            world,
            catalog,
            sessions: None,
        }
    }

    /// A fixture whose runtime owns a world journal and whose world half
    /// journals every receipt-bearing server-owned decision, so construction
    /// and warehouse inventory paths exercise their whole protocol.
    fn with_journal() -> Self {
        let root = tempfile::tempdir().unwrap();
        let catalog = Arc::new(catalog());
        let sessions = Arc::new(SessionRegistry::new());
        let world = Arc::new(FakeWorld::new().journal_warehouse_transfers(Arc::clone(&sessions)));
        let runtime = runtime_at_with(
            Arc::clone(&catalog),
            Arc::clone(&world),
            Arc::clone(&sessions),
            Some(root.path()),
            true,
            true,
        );
        Self {
            root,
            runtime,
            world,
            catalog,
            sessions: Some(sessions),
        }
    }

    /// Register one connected actor holding `stacks` in its canonical slots.
    /// The returned guard keeps the session's outbound channel open.
    fn register_actor(&self, name: &str, stacks: &[(usize, u32, i32)]) -> u64 {
        let sessions = self
            .sessions
            .as_ref()
            .expect("a journal fixture registers sessions");
        let mut inventory = vec![ItemStack::EMPTY; 46];
        for (slot, item_id, count) in stacks {
            inventory[*slot] = ItemStack::new(*item_id, *count);
        }
        sessions.register_owned_inventory_test_session(name, &inventory)
    }

    /// The actor's canonical inventory and durable revision, as the boundary
    /// reports them.
    fn actor_inventory(&self, actor_id: u64) -> (Vec<ItemStack>, u64) {
        let actor = self
            .sessions
            .as_ref()
            .expect("a journal fixture registers sessions")
            .warehouse_transfer_actor(actor_id)
            .expect("the registered actor is live");
        (actor.inventory, actor.revision)
    }

    /// A second runtime over the same journal directory with a fresh world fake.
    fn reopened(&self) -> (InventoryRuntime, Arc<FakeWorld>) {
        let world = Arc::new(FakeWorld::new());
        let runtime = runtime_at(Arc::clone(&self.catalog), Arc::clone(&world), true, true);
        (runtime, world)
    }

    fn storage(&self) -> PluginStorage {
        PluginStorage::open(self.root.path()).unwrap()
    }

    /// Execute one owned-inventory request through the C1 boundary.
    async fn execute_inventory(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
    ) -> ScriptOperationOutcome {
        self.runtime
            .execute_owned_inventory(storage, plugin_id, request)
            .await
            .expect("an owned inventory request reaches the durable boundary")
    }

    fn selector(&self) -> SettlementSelector {
        SettlementSelector::new(SEED, PROFILE_REVISION)
    }

    fn candidate(&self) -> mc_worldgen::SiteCandidate {
        self.selector()
            .discover(START_CELL, 64)
            .into_iter()
            .next()
            .expect("the deterministic selector finds a site in the first window")
    }

    /// The staged structure's anchor, inside the site's own generator cell.
    fn anchor(&self) -> [i32; 3] {
        let candidate = self.candidate();
        [candidate.origin[0], ANCHOR_Y, candidate.origin[2]]
    }

    async fn execute(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
    ) -> ScriptOperationOutcome {
        self.runtime
            .execute_settlement_operation(storage, plugin_id, request)
            .await
            .expect("settlement operation reaches the durable boundary")
    }
}

fn settlement(operation: ScriptSettlementOperation) -> ScriptOperationRequest {
    let fallback = format!("{operation:?}");
    ScriptOperationRequest::try_new("request", ScriptOperation::Settlement { operation })
        .unwrap_or_else(|error| panic!("valid settlement request {fallback}: {error}"))
}

fn survey_request(dimension: &str, bounds: ScriptSurveyBounds) -> ScriptOperationRequest {
    settlement(ScriptSettlementOperation::Survey {
        dimension: dimension.to_owned(),
        bounds,
        purpose: ScriptSurveyPurpose::Settlement,
    })
}

fn prepare_request(
    operation_id: &str,
    blueprint_id: &str,
    anchor: [i32; 3],
    survey_token: &str,
    expected_site_revision: u64,
) -> ScriptOperationRequest {
    settlement(ScriptSettlementOperation::PrepareStructure {
        operation_id: operation_id.to_owned(),
        blueprint_id: blueprint_id.to_owned(),
        anchor,
        rotation: 0,
        survey_token: survey_token.to_owned(),
        expected_site_revision,
    })
}

fn advance_request(
    operation_id: &str,
    structure_id: &str,
    stage: &str,
    reservation_ref: &str,
    expected_revision: u64,
    work_units: u64,
) -> ScriptOperationRequest {
    settlement(ScriptSettlementOperation::AdvanceStructure {
        operation_id: operation_id.to_owned(),
        structure_id: structure_id.to_owned(),
        stage: stage.to_owned(),
        reservation_ref: reservation_ref.to_owned(),
        expected_revision,
        work_units,
    })
}

fn bind_warehouse_request(
    operation_id: &str,
    structure_id: &str,
    container_id: u32,
) -> ScriptOperationRequest {
    settlement(ScriptSettlementOperation::BindWarehouse {
        operation_id: operation_id.to_owned(),
        structure_id: structure_id.to_owned(),
        container_id,
    })
}

fn bind_village_warehouse_request(
    operation_id: &str,
    site_id: &str,
    container_id: u32,
) -> ScriptOperationRequest {
    settlement(ScriptSettlementOperation::BindVillageWarehouse {
        operation_id: operation_id.to_owned(),
        site_id: site_id.to_owned(),
        container_id,
    })
}

fn warehouse_query_request(handle: &str, expected_revision: Option<u64>) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::Query {
                endpoint: ScriptInventoryEndpoint::Warehouse {
                    handle: handle.to_owned(),
                },
                expected_revision,
            },
        },
    )
    .expect("a warehouse query is a valid request")
}

fn warehouse_reserve_request(
    operation_id: &str,
    handle: &str,
    resource_plan: ScriptInventoryResourcePlan,
    expected_revision: ScriptInventoryFence,
) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        "reserve-warehouse",
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::Reserve {
                operation_id: operation_id.to_owned(),
                endpoint: ScriptInventoryEndpoint::Warehouse {
                    handle: handle.to_owned(),
                },
                resource_plan,
                expected_revision,
            },
        },
    )
    .expect("a warehouse reservation is a valid request")
}

fn pause_request(
    operation_id: &str,
    structure_id: &str,
    expected_revision: u64,
) -> ScriptOperationRequest {
    settlement(ScriptSettlementOperation::PauseStructure {
        operation_id: operation_id.to_owned(),
        structure_id: structure_id.to_owned(),
        expected_revision,
    })
}

fn resume_request(
    operation_id: &str,
    structure_id: &str,
    expected_revision: u64,
) -> ScriptOperationRequest {
    settlement(ScriptSettlementOperation::ResumeStructure {
        operation_id: operation_id.to_owned(),
        structure_id: structure_id.to_owned(),
        expected_revision,
    })
}

fn cancel_request(
    operation_id: &str,
    structure_id: &str,
    expected_revision: u64,
) -> ScriptOperationRequest {
    settlement(ScriptSettlementOperation::CancelStructure {
        operation_id: operation_id.to_owned(),
        structure_id: structure_id.to_owned(),
        expected_revision,
    })
}

fn status_request(structure_id: &str) -> ScriptOperationRequest {
    settlement(ScriptSettlementOperation::Status {
        structure_id: structure_id.to_owned(),
    })
}

fn reserve_poi_request(
    operation_id: &str,
    site_id: &str,
    poi_id: &str,
    expected_site_revision: u64,
) -> ScriptOperationRequest {
    settlement(ScriptSettlementOperation::ReserveResidentSite {
        operation_id: operation_id.to_owned(),
        site_id: site_id.to_owned(),
        poi_id: poi_id.to_owned(),
        expected_site_revision,
    })
}

fn release_poi_request(operation_id: &str, spawn_site_token: &str) -> ScriptOperationRequest {
    settlement(ScriptSettlementOperation::ReleaseResidentSite {
        operation_id: operation_id.to_owned(),
        spawn_site_token: spawn_site_token.to_owned(),
    })
}

fn settlement_payload(outcome: &ScriptOperationOutcome) -> &ScriptSettlementResult {
    match outcome.payload() {
        ScriptOperationPayload::Settlement { result } => result,
        other => panic!("expected a settlement payload, got {other:?}"),
    }
}

fn site_of(outcome: &ScriptOperationOutcome) -> ScriptSettlementSite {
    match settlement_payload(outcome) {
        ScriptSettlementResult::Site { site } => site.as_ref().clone(),
        other => panic!("expected one site, got {other:?}"),
    }
}

fn page_of(outcome: &ScriptOperationOutcome) -> (Vec<ScriptSettlementSite>, Option<String>) {
    match settlement_payload(outcome) {
        ScriptSettlementResult::Sites { page } => (page.sites.clone(), page.cursor.clone()),
        other => panic!("expected a site page, got {other:?}"),
    }
}

fn survey_of(outcome: &ScriptOperationOutcome) -> mc_script::ScriptSurveySnapshot {
    match settlement_payload(outcome) {
        ScriptSettlementResult::Survey { survey } => survey.as_ref().clone(),
        other => panic!("expected a survey, got {other:?}"),
    }
}

fn structure_of(outcome: &ScriptOperationOutcome) -> ScriptStructureSnapshot {
    match settlement_payload(outcome) {
        ScriptSettlementResult::Structure { structure } => structure.as_ref().clone(),
        other => panic!("expected a structure, got {other:?}"),
    }
}

fn receipt_of(outcome: &ScriptOperationOutcome) -> ScriptStructureReceipt {
    match settlement_payload(outcome) {
        ScriptSettlementResult::Receipt { receipt } => receipt.as_ref().clone(),
        other => panic!("expected a receipt, got {other:?}"),
    }
}

fn resident_site_of(outcome: &ScriptOperationOutcome) -> ScriptResidentSiteReservation {
    match settlement_payload(outcome) {
        ScriptSettlementResult::ResidentSite { reservation } => reservation.as_ref().clone(),
        other => panic!("expected a resident site reservation, got {other:?}"),
    }
}

fn warehouse_of(outcome: &ScriptOperationOutcome) -> ScriptWarehouseBinding {
    match settlement_payload(outcome) {
        ScriptSettlementResult::Warehouse { binding } => binding.as_ref().clone(),
        other => panic!("expected a warehouse binding, got {other:?}"),
    }
}

fn owned_snapshot_of(outcome: &ScriptOperationOutcome) -> mc_script::ScriptOwnedInventorySnapshot {
    match outcome.payload() {
        ScriptOperationPayload::OwnedInventory { result } => match &**result {
            ScriptOwnedInventoryResult::Snapshot { inventory } => inventory.clone(),
            other => panic!("expected an owned inventory snapshot, got {other:?}"),
        },
        other => panic!("expected an owned inventory payload, got {other:?}"),
    }
}

fn assert_invariant(quantities: &[ScriptInventoryReservationQuantity]) {
    for quantity in quantities {
        assert_eq!(
            quantity.reserved,
            quantity.consumed + quantity.returned + quantity.remaining,
            "reserved = consumed + returned + remaining for {}",
            quantity.resource_id
        );
    }
}

trait ReservationView {
    fn quantity(&self, resource: &str) -> &ScriptInventoryReservationQuantity;
    fn remaining_total(&self) -> u64;
}

impl ReservationView for ScriptInventoryReservationSnapshot {
    fn quantity(&self, resource: &str) -> &ScriptInventoryReservationQuantity {
        self.quantities
            .iter()
            .find(|quantity| quantity.resource_id == resource)
            .unwrap_or_else(|| panic!("{resource} is reserved: {self:?}"))
    }

    fn remaining_total(&self) -> u64 {
        self.quantities
            .iter()
            .map(|quantity| quantity.remaining)
            .sum()
    }
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

/// Commit one durable reservation receipt for `plan`, exactly the projection the
/// C1 reserve operation installs, without needing a live player session.
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
            operation: ScriptOwnedInventoryOperation::Reserve {
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
                result: Box::new(ScriptOwnedInventoryResult::Reservation {
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

/// Survey the fixture's first site and prepare the staged cottage inside it.
async fn prepared_cottage(
    fixture: &Fixture,
    storage: &mut PluginStorage,
    operation_id: &str,
) -> (ScriptStructureSnapshot, String) {
    let bounds = ScriptSurveyBounds::new([0, 0, 0], [7, 3, 7]).unwrap();
    let outcome = fixture
        .execute(
            storage,
            OWNER,
            &survey_request("minecraft:overworld", bounds),
        )
        .await;
    assert_eq!(outcome.failure(), None, "survey: {outcome:?}");
    let token = survey_of(&outcome).survey_token;
    let outcome = fixture
        .execute(
            storage,
            OWNER,
            &prepare_request(operation_id, COTTAGE, fixture.anchor(), &token, 0),
        )
        .await;
    assert_eq!(outcome.failure(), None, "prepare: {outcome:?}");
    let structure = structure_of(&outcome);
    assert_eq!(structure.state, ScriptStructureState::Prepared);
    assert!(
        structure.reservation_ref.is_none(),
        "a prepared structure holds no reservation yet"
    );
    (structure, token)
}

/// The generated village a fixture's runtime knows about.
///
/// Two pieces, one template each, in a region the site cell `[0, 0]` contains;
/// the facade the generator would place is not part of a site descriptor.
fn generated_village() -> mc_world::GeneratedVillageSite {
    mc_world::GeneratedVillageSite {
        start_chunk: (3, 5),
        min: mc_world::BlockPos {
            x: 48,
            y: 64,
            z: 80,
        },
        max: mc_world::BlockPos {
            x: 79,
            y: 72,
            z: 111,
        },
        pieces: vec![
            mc_world::GeneratedVillagePiece {
                template: Some("minecraft:village/plains/houses/plains_small_house_1".to_owned()),
                position: mc_world::BlockPos {
                    x: 48,
                    y: 64,
                    z: 80,
                },
                rotation: 1,
            },
            // A `feature_pool_element` places no template and is not a building.
            mc_world::GeneratedVillagePiece {
                template: None,
                position: mc_world::BlockPos {
                    x: 60,
                    y: 70,
                    z: 96,
                },
                rotation: 0,
            },
        ],
    }
}

/// The claim one generated village placement carries.
///
/// The generator records it with the chunk (`Chunk::settlement_inhabitants`) and
/// the spawn lane mints the villager's UUID from it, so this is the identity a
/// descriptor must publish for that villager and the identity `claim_resident`
/// takes back.
const VILLAGE_INHABITANT_CLAIM: &str =
    "minecraft:village/plains/houses/plains_small_house_1@48:64:80#0";

/// The generator's enumeration, reduced to the one village a test knows about.
struct FakeVillageGround {
    village: mc_world::GeneratedVillageSite,
}

impl crate::script::storage::VillageSiteGround for FakeVillageGround {
    fn village_sites_in_region(
        &self,
        min_chunk: (i32, i32),
        max_chunk: (i32, i32),
    ) -> Vec<mc_world::GeneratedVillageSite> {
        let start = self.village.start_chunk;
        let inside = (min_chunk.0..=max_chunk.0).contains(&start.0)
            && (min_chunk.1..=max_chunk.1).contains(&start.1);
        inside.then(|| self.village.clone()).into_iter().collect()
    }
}

/// A fixture whose runtime knows one generated vanilla village.
fn fixture_with_generated_village() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let catalog = Arc::new(catalog());
    let world = Arc::new(FakeWorld::new());
    let runtime = runtime_with_generated_village(Arc::clone(&world));
    Fixture {
        root,
        runtime,
        world,
        catalog,
        sessions: None,
    }
}

/// The same runtime the other fixtures build, with the generated village.
fn runtime_with_generated_village(world: Arc<FakeWorld>) -> InventoryRuntime {
    InventoryRuntime::new(
        None,
        &ShutdownHandle::default(),
        Arc::new(SessionRegistry::new()),
        Arc::new(solaris_required_items()),
        Arc::new(solaris_required_item_facts()),
    )
    .with_settlement_runtime(Arc::new(SettlementRuntime::new(
        SettlementSelector::new(SEED, PROFILE_REVISION),
        Arc::new(catalog()),
        WORLD_IDENTITY,
        START_CELL,
        Arc::new(FlatGround),
        Some(Arc::new(FakeVillageGround {
            village: generated_village(),
        })),
    )))
    .with_settlement_world(world as Arc<dyn SettlementWorld>)
}

/// A generated vanilla village is a site of the same page: its identity, region
/// and pieces come from the generator's own plan, and the id the page mints is
/// the id a query reverses.
#[tokio::test]
async fn a_generated_village_is_listed_and_queried_by_its_own_id() {
    let fixture = fixture_with_generated_village();
    let mut storage = fixture.storage();
    let village = generated_village();
    let expected_id = crate::script::storage::village_site_id(
        WORLD_IDENTITY,
        "minecraft:overworld",
        village.start_chunk,
    );

    assert!(
        fixture.runtime.settlement_runtime().is_some(),
        "the fixture installs a settlement runtime"
    );
    assert!(
        fixture.runtime.settlement_world().is_some(),
        "the fixture installs a settlement world"
    );
    let listed = fixture
        .execute(
            &mut storage,
            OWNER,
            &settlement(ScriptSettlementOperation::ListSites {
                cursor: None,
                limit: 64,
            }),
        )
        .await;
    assert_eq!(
        listed.failure(),
        None,
        "the page must be answered, not rejected"
    );
    let (sites, _) = page_of(&listed);
    let site = sites
        .iter()
        .find(|site| site.site_id == expected_id)
        .unwrap_or_else(|| panic!("the generated village is a site of the page: {sites:?}"));
    assert_eq!(site.provenance, ScriptSiteProvenance::VanillaVillage);
    assert_eq!(site.variant, ScriptSiteVariant::Village);
    assert_eq!(site.footprint_origin, [48, 64, 80]);
    assert_eq!(site.footprint_size, [32, 9, 32]);
    assert!(
        site.contents_known,
        "the fixture's world answers its readings, so the contents are known"
    );
    assert_eq!(
        site.inhabitant_generation_ids,
        vec![
            crate::settlement_identity::settlement_entity_uuid(VILLAGE_INHABITANT_CLAIM)
                .to_string()
        ],
        "a generated site publishes the entity identity its placement mints, which is the id \
         `claim_resident` adopts"
    );
    assert_eq!(
        site.buildings.len(),
        1,
        "only the piece that places a template is a building"
    );
    assert_eq!(
        site.buildings[0].blueprint_id,
        "minecraft:village/plains/houses/plains_small_house_1"
    );
    assert_eq!(site.buildings[0].origin, [48, 64, 80]);
    assert_eq!(
        site.buildings[0].rotation, 90,
        "the descriptor reports the plan's quarter turn in degrees"
    );

    let queried = fixture
        .execute(
            &mut storage,
            OWNER,
            &settlement(ScriptSettlementOperation::QuerySite {
                site_id: expected_id.clone(),
                cursor: None,
                limit: 64,
            }),
        )
        .await;
    assert_eq!(
        site_of(&queried).site_id,
        expected_id,
        "a site id the page minted is the id a query reverses"
    );

    // An id of the right shape that names no village the generator places is not
    // a site: enumeration answers from the generator, never from the id alone.
    let forged = format!("{}ff", &expected_id[..expected_id.len() - 2]);
    let unknown = fixture
        .execute(
            &mut storage,
            OWNER,
            &settlement(ScriptSettlementOperation::QuerySite {
                site_id: forged,
                cursor: None,
                limit: 64,
            }),
        )
        .await;
    assert_eq!(unknown.failure(), Some(ScriptOperationFailure::NotFound));

    // A well-formed id for another chunk of the same world is also absent,
    // because that chunk holds no village.
    let elsewhere =
        crate::script::storage::village_site_id(WORLD_IDENTITY, "minecraft:overworld", (4, 5));
    let absent = fixture
        .execute(
            &mut storage,
            OWNER,
            &settlement(ScriptSettlementOperation::QuerySite {
                site_id: elsewhere,
                cursor: None,
                limit: 64,
            }),
        )
        .await;
    assert_eq!(absent.failure(), Some(ScriptOperationFailure::NotFound));
}

#[tokio::test]
async fn village_warehouse_binds_one_materialized_position_without_rebinding_its_ordinal() {
    let fixture = fixture_with_generated_village();
    let village = generated_village();
    let site_id = crate::script::storage::village_site_id(
        WORLD_IDENTITY,
        "minecraft:overworld",
        village.start_chunk,
    );
    let bound_position = [64, 66, 96];
    fixture
        .world
        .set_container(bound_position, vec![ItemStack::new(BIRCH_LOG, 12)]);
    let mut storage = fixture.storage();

    let bind = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_village_warehouse_request("bind-village", &site_id, 0),
        )
        .await;
    assert_eq!(bind.failure(), None, "bind: {bind:?}");
    let binding = warehouse_of(&bind);
    assert!(matches!(
        &binding.source,
        ScriptWarehouseSource::VanillaVillage {
            site_id: bound_site_id,
            container_id,
            ..
        } if bound_site_id == &site_id && *container_id == 0
    ));

    let foreign = fixture
        .execute(
            &mut storage,
            FOREIGN,
            &bind_village_warehouse_request("bind-village-foreign", &site_id, 0),
        )
        .await;
    assert_eq!(foreign.failure(), Some(ScriptOperationFailure::Forbidden));

    // A later container sorts before the bound one. Reopening must keep reading
    // the exact resolved position, not whichever container now owns ordinal zero.
    fixture
        .world
        .set_container([50, 66, 96], vec![ItemStack::new(BIRCH_LOG, 1)]);
    let shifted_owner = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_village_warehouse_request("bind-village-owner-shifted", &site_id, 1),
        )
        .await;
    assert_eq!(
        shifted_owner.failure(),
        None,
        "shifted owner bind: {shifted_owner:?}"
    );
    assert_eq!(
        warehouse_of(&shifted_owner),
        binding,
        "the owner gets the original binding for the resolved physical container"
    );

    // The original chest is now ordinal one. That ordinal must still resolve to
    // the original binding rather than allowing a foreign handle to be minted.
    let shifted_foreign = fixture
        .execute(
            &mut storage,
            FOREIGN,
            &bind_village_warehouse_request("bind-village-foreign-shifted", &site_id, 1),
        )
        .await;
    assert_eq!(
        shifted_foreign.failure(),
        Some(ScriptOperationFailure::Forbidden),
        "the physical container remains owned after its ordinal changes"
    );
    assert!(
        storage
            .operation_receipt(FOREIGN, "bind-village-foreign-shifted")
            .is_none(),
        "the refusal mints neither a second binding nor a recoverable handle"
    );
    drop(storage);
    let mut reopened = fixture.storage();
    let read = fixture
        .runtime
        .execute_owned_inventory(
            &mut reopened,
            OWNER,
            &warehouse_query_request(&binding.handle, Some(binding.revision)),
        )
        .await
        .expect("the reopened binding reaches the same warehouse endpoint");
    assert_eq!(read.failure(), None, "reopened read: {read:?}");
    let snapshot = owned_snapshot_of(&read);
    assert_eq!(
        snapshot.slots[0].item.as_ref().map(|item| item.count),
        Some(12),
        "the binding retained the original resolved block position"
    );

    let unloaded = fixture_with_generated_village();
    unloaded
        .world
        .set_availability(ScriptChunkAvailability::Unloaded);
    let mut unloaded_storage = unloaded.storage();
    let unavailable = unloaded
        .execute(
            &mut unloaded_storage,
            OWNER,
            &bind_village_warehouse_request("bind-village-unloaded", &site_id, 0),
        )
        .await;
    assert_eq!(
        unavailable.failure(),
        Some(ScriptOperationFailure::Unloaded),
        "a partly unavailable village never yields a shortened ordinal set"
    );

    let missing = fixture_with_generated_village();
    let mut missing_storage = missing.storage();
    let absent = missing
        .execute(
            &mut missing_storage,
            OWNER,
            &bind_village_warehouse_request("bind-village-missing", &site_id, 0),
        )
        .await;
    assert_eq!(absent.failure(), Some(ScriptOperationFailure::NotFound));
}

#[tokio::test]
async fn list_and_query_sites_are_deterministic() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let list = settlement(ScriptSettlementOperation::ListSites {
        cursor: None,
        limit: 64,
    });

    let first = fixture.execute(&mut storage, OWNER, &list).await;
    let second = fixture.execute(&mut storage, OWNER, &list).await;
    assert_eq!(first, second, "discovery must not shift or duplicate ids");
    let (sites, cursor) = page_of(&first);
    assert!(!sites.is_empty(), "the first window carries sites");
    assert!(
        cursor.is_some(),
        "discovery always hands back the next cell"
    );

    let mut ids = sites
        .iter()
        .map(|site| site.site_id.clone())
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), sites.len(), "page ids are unique");

    for site in &sites {
        // The generator's variant footprint is reported verbatim.
        let cell = fixture
            .selector()
            .cell_from_site_id(&site.site_id)
            .expect("a listed site reverses to its cell");
        let candidate = fixture.selector().candidate(cell).expect("a listed cell");
        // The reported origin carries the resolved site base row: the first free
        // row above the fixture's flat 63 ground. The selected candidate itself
        // stays terrain-free.
        assert_eq!(site.footprint_origin[0], candidate.origin[0]);
        assert_eq!(site.footprint_origin[1], 64);
        assert_eq!(site.footprint_origin[2], candidate.origin[2]);
        assert_eq!(site.footprint_size, candidate.size);
        assert_eq!(site.revision, 0, "an untouched site has revision zero");

        // A repeated query of the same id is the same site, whatever order the
        // page reported it in.
        let query = settlement(ScriptSettlementOperation::QuerySite {
            site_id: site.site_id.clone(),
            cursor: None,
            limit: 64,
        });
        let outcome = fixture.execute(&mut storage, OWNER, &query).await;
        assert_eq!(&site_of(&outcome), site);

        // Inhabitant identities are the deterministic per-slot derivation.
        let homes = site
            .pois
            .iter()
            .filter(|poi| poi.kind == ScriptSitePoiKind::Home)
            .count();
        assert_eq!(site.inhabitant_generation_ids.len(), homes);
        for generation in &site.inhabitant_generation_ids {
            let slot = (0..site.pois.len())
                .find(|slot| {
                    resident_generation_id(WORLD_IDENTITY, &site.site_id, *slot as u32).unwrap()
                        == *generation
                })
                .expect("every inhabitant id names one of the site's slots");
            assert_eq!(
                generation,
                &resident_generation_id(WORLD_IDENTITY, &site.site_id, slot as u32).unwrap()
            );
        }
        assert!(
            site.buildings
                .iter()
                .any(|building: &ScriptSettlementBuilding| building.blueprint_id
                    == "solaris:cottage"),
            "a laid-out site places buildings: {site:?}"
        );
    }

    // Paging one cell at a time visits exactly the sites of the wide page.
    let mut paged = Vec::new();
    let mut cursor = None;
    for _ in 0..64 {
        let request = settlement(ScriptSettlementOperation::ListSites {
            cursor: cursor.clone(),
            limit: 1,
        });
        let outcome = fixture.execute(&mut storage, OWNER, &request).await;
        let (page, next) = page_of(&outcome);
        paged.extend(page.into_iter().map(|site| site.site_id));
        match next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    paged.sort_unstable();
    paged.dedup();
    for id in &paged {
        assert!(ids.contains(id), "paged site {id} is in the wide page");
    }
    assert_eq!(paged.len(), ids.len(), "paging loses no site of the window");

    // A malformed cursor is refused rather than scanned.
    let bad = settlement(ScriptSettlementOperation::ListSites {
        cursor: Some("north".to_owned()),
        limit: 8,
    });
    let outcome = fixture.execute(&mut storage, OWNER, &bad).await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::CursorExpired)
    );
}

#[tokio::test]
async fn query_site_pages_points_of_interest_and_rejects_a_bad_cursor() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let full_request = settlement(ScriptSettlementOperation::QuerySite {
        site_id: fixture.candidate().site_id,
        cursor: None,
        limit: 64,
    });
    let full = site_of(&fixture.execute(&mut storage, OWNER, &full_request).await);
    assert!(
        full.pois.len() > 1,
        "a site exposes several points of interest"
    );

    let page_request = settlement(ScriptSettlementOperation::QuerySite {
        site_id: full.site_id.clone(),
        cursor: None,
        limit: 1,
    });
    let page = site_of(&fixture.execute(&mut storage, OWNER, &page_request).await);
    assert_eq!(page.pois.len(), 1);
    assert_eq!(page.pois[0].poi_id, full.pois[0].poi_id);
    assert_eq!(
        page.buildings, full.buildings,
        "a page keeps the site's plan"
    );

    let next = settlement(ScriptSettlementOperation::QuerySite {
        site_id: full.site_id.clone(),
        cursor: Some(page.pois[0].poi_id.clone()),
        limit: 1,
    });
    let second = site_of(&fixture.execute(&mut storage, OWNER, &next).await);
    assert_eq!(second.pois.len(), 1);
    assert_eq!(second.pois[0].poi_id, full.pois[1].poi_id);

    let bad = settlement(ScriptSettlementOperation::QuerySite {
        site_id: full.site_id.clone(),
        cursor: Some("settlement.poi.forged".to_owned()),
        limit: 1,
    });
    let outcome = fixture.execute(&mut storage, OWNER, &bad).await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::CursorExpired)
    );

    let forged = settlement(ScriptSettlementOperation::QuerySite {
        site_id: "site_9_9_deadbeef".to_owned(),
        cursor: None,
        limit: 8,
    });
    let outcome = fixture.execute(&mut storage, OWNER, &forged).await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::NotFound));
}

#[tokio::test]
async fn prepare_builds_nothing_and_cancel_preserves_built_blocks() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-1")
        .await
        .0;

    // Preparing plans work and spends nothing.
    assert_eq!(fixture.world.built_total(), 0);
    assert!(structure.consumed.is_empty());
    assert_eq!(structure.watermark, 0);
    assert_eq!(structure.current_stage_index, 0);
    assert_eq!(structure.completed_work_units, 0);
    assert_eq!(
        structure.stages.len(),
        2,
        "one plan per stage: {structure:?}"
    );
    assert_eq!(structure.stages[0].stage, "foundation");
    assert_eq!(structure.stages[0].block_count, 3);
    assert_eq!(structure.stages[1].stage, "walls");
    assert_eq!(structure.stages[1].block_count, 2);
    assert_eq!(
        structure
            .stages
            .iter()
            .map(|stage| stage.work_units)
            .sum::<u64>(),
        5
    );

    // Cancelling a structure that never spent anything leaves the world empty.
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &cancel_request(
                "cancel-prepared",
                &structure.structure_id,
                structure.revision,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), None, "cancel: {outcome:?}");
    let cancelled = structure_of(&outcome);
    assert_eq!(cancelled.state, ScriptStructureState::Cancelled);
    assert!(cancelled.consumed.is_empty());
    assert_eq!(fixture.world.built_total(), 0);

    // A second structure builds a partial portion and then cancels: the
    // committed blocks stay in the world and the unspent remainder returns.
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-2")
        .await
        .0;
    let plan = plan_of(&structure);
    let reserved = reserve_plan(&mut storage, "res-cottage", &plan);
    assert_eq!(reserved.remaining_total(), 5);
    assert_invariant(&reserved.quantities);

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-partial",
                &structure.structure_id,
                &structure.stages[0].stage,
                "res-cottage",
                structure.revision,
                2,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), None, "advance: {outcome:?}");
    let receipt = receipt_of(&outcome);
    assert_eq!(receipt.block_count, 2);
    assert_eq!(receipt.work_units, 2);
    assert_eq!(receipt.stage, "foundation");
    assert_eq!(receipt.sequence, 1);
    assert_eq!(fixture.world.built_blocks(&structure.structure_id), 2);

    let status = fixture
        .execute(
            &mut storage,
            OWNER,
            &status_request(&structure.structure_id),
        )
        .await;
    let running = structure_of(&status);
    assert_eq!(running.state, ScriptStructureState::Running);
    assert_eq!(running.consumed[0].quantity, 2, "two stone were spent");
    assert_eq!(running.current_stage_index, 0);
    assert_eq!(running.completed_work_units, 2);

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &cancel_request("cancel-running", &structure.structure_id, running.revision),
        )
        .await;
    assert_eq!(outcome.failure(), None, "cancel: {outcome:?}");
    let cancelled = structure_of(&outcome);
    assert_eq!(cancelled.state, ScriptStructureState::Cancelled);
    assert!(
        cancelled.remaining.is_empty(),
        "a cancelled structure holds no remainder"
    );
    assert_eq!(
        fixture.world.built_blocks(&structure.structure_id),
        2,
        "cancel never removes built blocks"
    );

    let (_, projection) = storage
        .settlement_reservation(OWNER, "res-cottage")
        .expect("the reservation stays projected");
    assert!(projection.released);
    assert_eq!(projection.remaining_total(), 0);
    assert_invariant(&projection.quantities);
    assert_eq!(projection.quantity("minecraft:stone").consumed, 2);
    assert_eq!(projection.quantity("minecraft:stone").returned, 1);
    assert_eq!(projection.quantity("minecraft:oak_planks").returned, 2);
}

#[tokio::test]
async fn pause_resume_requires_the_accepted_footprint_and_releases_only_unspent_materials() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-resume")
        .await
        .0;
    let plan = plan_of(&structure);
    reserve_plan(&mut storage, "res-resume", &plan);

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &pause_request(
                "pause-prepared",
                &structure.structure_id,
                structure.revision,
            ),
        )
        .await;
    let paused = structure_of(&outcome);
    assert_eq!(paused.state, ScriptStructureState::Paused);
    assert_eq!(paused.pause_reason, None);

    let resume = resume_request("resume-prepared", &structure.structure_id, paused.revision);
    let outcome = fixture.execute(&mut storage, OWNER, &resume).await;
    let prepared = structure_of(&outcome);
    assert_eq!(prepared.state, ScriptStructureState::Prepared);
    assert!(prepared.consumed.is_empty());
    assert_eq!(
        fixture.execute(&mut storage, OWNER, &resume).await,
        outcome,
        "a repeated resume returns the accepted status rather than a new intent"
    );

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-resume",
                &structure.structure_id,
                &structure.stages[0].stage,
                "res-resume",
                prepared.revision,
                2,
            ),
        )
        .await;
    let receipt = receipt_of(&outcome);
    assert_eq!(receipt.block_count, 2);

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &pause_request("pause-running", &structure.structure_id, receipt.revision),
        )
        .await;
    let paused = structure_of(&outcome);
    assert_eq!(paused.state, ScriptStructureState::Paused);
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &resume_request("resume-running", &structure.structure_id, paused.revision),
        )
        .await;
    let running = structure_of(&outcome);
    assert_eq!(running.state, ScriptStructureState::Running);
    assert_eq!(running.consumed[0].quantity, 2);

    fixture.world.mark_change();
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &pause_request("pause-changed", &structure.structure_id, running.revision),
        )
        .await;
    let paused = structure_of(&outcome);
    assert_eq!(paused.pause_reason.as_deref(), Some("site_changed"));
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &resume_request("resume-changed", &structure.structure_id, paused.revision),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &cancel_request("cancel-resume", &structure.structure_id, paused.revision),
        )
        .await;
    let cancelled = structure_of(&outcome);
    assert_eq!(cancelled.state, ScriptStructureState::Cancelled);
    assert_eq!(fixture.world.built_blocks(&structure.structure_id), 2);
    let (_, reservation) = storage
        .settlement_reservation(OWNER, "res-resume")
        .expect("cancelled structure leaves a durable reservation projection");
    assert_eq!(reservation.quantity("minecraft:stone").consumed, 2);
    assert_eq!(reservation.quantity("minecraft:stone").returned, 1);
    assert_eq!(reservation.quantity("minecraft:oak_planks").returned, 2);
    assert_invariant(&reservation.quantities);

    let repeated = fixture
        .execute(
            &mut storage,
            OWNER,
            &cancel_request(
                "cancel-resume-again",
                &structure.structure_id,
                cancelled.revision,
            ),
        )
        .await;
    assert_eq!(repeated.failure(), Some(ScriptOperationFailure::Blocked));
}

#[tokio::test]
async fn prepare_plans_a_body_only_blueprint_as_one_stage() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let bounds = ScriptSurveyBounds::new([0, 0, 0], [7, 3, 7]).unwrap();
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &survey_request("minecraft:overworld", bounds),
        )
        .await;
    let token = survey_of(&outcome).survey_token;
    let mut anchor = fixture.anchor();
    anchor[1] += 8;

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request("prepare-body", "solaris:tower", anchor, &token, 0),
        )
        .await;
    assert_eq!(outcome.failure(), None, "prepare: {outcome:?}");
    let structure = structure_of(&outcome);
    assert_eq!(structure.stages.len(), 1);
    assert_eq!(structure.stages[0].stage, "body");
    assert_eq!(structure.stages[0].block_count, 1);
    assert_eq!(structure.stages[0].work_units, 1);
    assert_eq!(structure.stages[0].materials.len(), 1);
    assert_eq!(structure.stages[0].materials[0].resource, "minecraft:stone");
    assert_eq!(structure.stages[0].materials[0].quantity, 1);
}

#[tokio::test]
async fn advance_replays_after_reopen_without_a_second_portion() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-3")
        .await
        .0;
    let plan = plan_of(&structure);
    reserve_plan(&mut storage, "res-replay", &plan);
    let advance = advance_request(
        "advance-foundation",
        &structure.structure_id,
        &structure.stages[0].stage,
        "res-replay",
        structure.revision,
        512,
    );
    let outcome = fixture.execute(&mut storage, OWNER, &advance).await;
    assert_eq!(outcome.failure(), None, "advance: {outcome:?}");
    let receipt = receipt_of(&outcome);
    assert_eq!(receipt.block_count, 3, "the whole first stage commits");
    assert_eq!(fixture.world.built_blocks(&structure.structure_id), 3);

    let (_, projected) = storage
        .settlement_reservation(OWNER, "res-replay")
        .expect("the reservation stays projected");
    assert_eq!(projected.receipt_watermark, 1);
    assert_eq!(
        projected.bound_to.as_deref(),
        Some(structure.structure_id.as_str())
    );
    assert_eq!(projected.quantity("minecraft:stone").consumed, 3);
    assert_invariant(&projected.quantities);

    // Reopen the journal from the same directory: the settlement ledger, the
    // structure, and the reservation projection all rebuild.
    storage.force_compact_for_test().unwrap();
    drop(storage);
    let (runtime, world) = fixture.reopened();
    let mut storage = fixture.storage();
    let (_, rebuilt) = storage
        .settlement_reservation(OWNER, "res-replay")
        .expect("the projection survives a reopen");
    assert_eq!(rebuilt, projected, "the reopened projection is identical");

    let replay = runtime
        .execute_settlement_operation(&mut storage, OWNER, &advance)
        .await
        .unwrap();
    assert_eq!(replay, outcome, "the same operation id returns its receipt");
    assert_eq!(receipt_of(&replay), receipt);
    assert_eq!(
        world.built_total(),
        0,
        "a replayed advance applies no second portion"
    );

    // The reopened ledger still serves the structure's status.
    let status = runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &status_request(&structure.structure_id),
        )
        .await
        .unwrap();
    let running = structure_of(&status);
    assert_eq!(running.state, ScriptStructureState::Running);
    assert_eq!(running.watermark, 1);
    assert_eq!(running.consumed[0].quantity, 3);

    // A changed fingerprint under the same operation id conflicts.
    let conflicting = advance_request(
        "advance-foundation",
        &structure.structure_id,
        &structure.stages[1].stage,
        "res-replay",
        structure.revision,
        512,
    );
    let outcome = runtime
        .execute_settlement_operation(&mut storage, OWNER, &conflicting)
        .await
        .unwrap();
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::OperationConflict)
    );
}

#[tokio::test]
async fn structure_portion_recovers_its_receipt_after_storage_append_fails() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-recover")
        .await
        .0;
    let plan = plan_of(&structure);
    reserve_plan(&mut storage, "res-recover", &plan);
    let advance = advance_request(
        "advance-recover",
        &structure.structure_id,
        &structure.stages[0].stage,
        "res-recover",
        structure.revision,
        512,
    );

    // The world half has accepted and journaled its after-image, but the
    // storage projection loses its first append attempt.
    storage.inject_fault_for_test(StorageFaultPoint::Write);
    assert!(
        fixture
            .runtime
            .execute_settlement_operation(&mut storage, OWNER, &advance)
            .await
            .is_err()
    );
    assert_eq!(fixture.world.built_blocks(&structure.structure_id), 3);
    let Fixture {
        root,
        runtime: original_runtime,
        world: original_world,
        catalog,
        sessions,
    } = fixture;
    drop(storage);
    drop(original_runtime);
    drop(original_world);
    drop(sessions);

    let sessions = Arc::new(SessionRegistry::new());
    let world = Arc::new(FakeWorld::new().journal_warehouse_transfers(Arc::clone(&sessions)));
    let runtime = runtime_at_with(
        Arc::clone(&catalog),
        Arc::clone(&world),
        sessions,
        Some(root.path()),
        true,
        true,
    );
    let mut storage = PluginStorage::open(root.path()).unwrap();
    runtime
        .recover(&mut storage)
        .expect("the durable world receipt restores its storage projection");

    let replay = runtime
        .execute_settlement_operation(&mut storage, OWNER, &advance)
        .await
        .expect("the recovered operation remains replayable");
    assert_eq!(replay.failure(), None, "recovered receipt: {replay:?}");
    assert_eq!(receipt_of(&replay).block_count, 3);
    assert_eq!(
        world.built_total(),
        0,
        "replaying a recovered receipt submits no second world portion"
    );
    let (_, reservation) = storage
        .settlement_reservation(OWNER, "res-recover")
        .expect("the reservation projection recovered with the receipt");
    assert_eq!(reservation.quantity("minecraft:stone").consumed, 3);
    assert_invariant(&reservation.quantities);
}
#[tokio::test]
async fn competing_structure_portions_admit_only_one_revision() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-competing")
        .await
        .0;
    let plan = plan_of(&structure);
    reserve_plan(&mut storage, "res-competing", &plan);
    let first = advance_request(
        "advance-competing-a",
        &structure.structure_id,
        &structure.stages[0].stage,
        "res-competing",
        structure.revision,
        1,
    );
    let competing = advance_request(
        "advance-competing-b",
        &structure.structure_id,
        &structure.stages[0].stage,
        "res-competing",
        structure.revision,
        1,
    );

    let accepted = fixture.execute(&mut storage, OWNER, &first).await;
    assert_eq!(accepted.failure(), None, "first portion: {accepted:?}");
    let refused = fixture.execute(&mut storage, OWNER, &competing).await;
    assert_eq!(
        refused.failure(),
        Some(ScriptOperationFailure::StaleRevision),
        "the second contender cannot reuse the consumed structure revision"
    );
    let current = structure_of(&refused);
    assert_eq!(current.revision, receipt_of(&accepted).revision);
    assert_eq!(current.current_stage_index, 0);
    assert_eq!(current.completed_work_units, 1);
    assert_eq!(fixture.world.built_blocks(&structure.structure_id), 1);
    let (_, reservation) = storage
        .settlement_reservation(OWNER, "res-competing")
        .expect("the accepted portion stays projected");
    assert_eq!(reservation.quantity("minecraft:stone").consumed, 1);
    assert_invariant(&reservation.quantities);
}

#[tokio::test]
async fn advance_pauses_when_the_reserved_site_changed() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-4")
        .await
        .0;
    let plan = plan_of(&structure);
    let reserved = reserve_plan(&mut storage, "res-paused", &plan);

    fixture.world.mark_change();
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-changed",
                &structure.structure_id,
                &structure.stages[0].stage,
                "res-paused",
                structure.revision,
                512,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), None, "advance: {outcome:?}");
    let paused = structure_of(&outcome);
    assert_eq!(paused.state, ScriptStructureState::Paused);
    assert_eq!(paused.pause_reason.as_deref(), Some("site_changed"));
    assert!(paused.consumed.is_empty());
    assert_eq!(fixture.world.built_total(), 0, "a pause builds nothing");

    let (_, projection) = storage
        .settlement_reservation(OWNER, "res-paused")
        .expect("the reservation stays projected");
    assert_eq!(
        projection.quantities, reserved.quantities,
        "nothing was consumed"
    );
    for quantity in &projection.quantities {
        assert_eq!(quantity.remaining, quantity.reserved);
    }

    // A paused structure can still be cancelled, and the cancel is terminal.
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &cancel_request("cancel-paused", &structure.structure_id, paused.revision),
        )
        .await;
    assert_eq!(
        structure_of(&outcome).state,
        ScriptStructureState::Cancelled
    );
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &cancel_request("cancel-again", &structure.structure_id, paused.revision),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));
}

#[tokio::test]
async fn advance_absorbs_its_own_durable_commits_but_a_foreign_edit_still_pauses() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-own")
        .await
        .0;
    let plan = plan_of(&structure);
    reserve_plan(&mut storage, "res-own", &plan);

    // First portion: two of the foundation's three cells. Applying it is a
    // durable world commit of this structure's own work, which advances the
    // world revision exactly like the live adapter's journal decision does.
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-own-1",
                &structure.structure_id,
                &structure.stages[0].stage,
                "res-own",
                structure.revision,
                2,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), None, "first advance: {outcome:?}");
    assert_eq!(receipt_of(&outcome).block_count, 2);
    let revision = receipt_of(&outcome).revision;

    // Second portion completes the same stage. The previous portion's own
    // durable commit must not be mistaken for a site change: no foreign edit
    // happened, so this advance must build, not pause.
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-own-2",
                &structure.structure_id,
                &structure.stages[0].stage,
                "res-own",
                revision,
                2,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), None, "second advance: {outcome:?}");
    assert_eq!(receipt_of(&outcome).block_count, 1);
    let revision = receipt_of(&outcome).revision;
    let status = fixture
        .execute(
            &mut storage,
            OWNER,
            &status_request(&structure.structure_id),
        )
        .await;
    let running = structure_of(&status);
    assert_eq!(running.state, ScriptStructureState::Running);
    assert_eq!(running.pause_reason, None);
    assert_eq!(fixture.world.built_blocks(&structure.structure_id), 3);

    // A durable change this structure did not make must still pause the build.
    fixture.world.mark_change();
    let next_stage = structure
        .stages
        .get(1)
        .expect("the cottage has a second stage to advance");
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-own-3",
                &structure.structure_id,
                &next_stage.stage,
                "res-own",
                revision,
                2,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), None, "third advance: {outcome:?}");
    let paused = structure_of(&outcome);
    assert_eq!(paused.state, ScriptStructureState::Paused);
    assert_eq!(paused.pause_reason.as_deref(), Some("site_changed"));
    assert_eq!(
        fixture.world.built_blocks(&structure.structure_id),
        3,
        "the paused advance built nothing"
    );
}

#[tokio::test]
async fn prepare_rejects_stale_and_foreign_survey_tokens() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let bounds = ScriptSurveyBounds::new([0, 0, 0], [7, 3, 7]).unwrap();
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &survey_request("minecraft:overworld", bounds),
        )
        .await;
    assert_eq!(outcome.failure(), None, "survey: {outcome:?}");
    let survey = survey_of(&outcome);
    assert_eq!(survey.usable_plots, 64);
    assert_eq!(survey.chunk_availability, ScriptChunkAvailability::Loaded);
    assert_eq!(survey.revision, 1);
    let token = survey.survey_token;

    // The surveyed footprint changed under the token.
    fixture.world.mark_change();
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request("stale", COTTAGE, fixture.anchor(), &token, 0),
        )
        .await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::StaleRevision)
    );

    // A token is owner scoped.
    let outcome = fixture
        .execute(
            &mut storage,
            FOREIGN,
            &prepare_request("foreign", COTTAGE, fixture.anchor(), &token, 0),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Forbidden));

    // An unknown token is not found rather than assumed valid.
    let unknown = "0".repeat(64);
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request("unknown", COTTAGE, fixture.anchor(), &unknown, 0),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::NotFound));

    // An unknown blueprint is not found.
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request(
                "unknown-blueprint",
                "solaris:keep",
                fixture.anchor(),
                &token,
                0,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::NotFound));
}

#[tokio::test]
async fn prepare_rejects_claimed_and_overlapping_footprints() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let bounds = ScriptSurveyBounds::new([0, 0, 0], [7, 3, 7]).unwrap();
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &survey_request("minecraft:overworld", bounds),
        )
        .await;
    let token = survey_of(&outcome).survey_token;

    fixture.world.set_claimed(true);
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request("claimed", COTTAGE, fixture.anchor(), &token, 0),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));
    fixture.world.set_claimed(false);

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request("first", COTTAGE, fixture.anchor(), &token, 0),
        )
        .await;
    assert_eq!(outcome.failure(), None, "prepare: {outcome:?}");

    // The same footprint is already reserved by this plugin.
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request("overlap", COTTAGE, fixture.anchor(), &token, 0),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));

    // A stale site revision is refused.
    let mut anchor = fixture.anchor();
    anchor[1] += 8;
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request("stale-site", COTTAGE, anchor, &token, 7),
        )
        .await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::StaleRevision)
    );
}

#[tokio::test]
async fn prepare_refuses_a_footprint_the_terrain_rises_into() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let bounds = ScriptSurveyBounds::new([0, 0, 0], [7, 3, 7]).unwrap();
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &survey_request("minecraft:overworld", bounds),
        )
        .await;
    let token = survey_of(&outcome).survey_token;
    let anchor = fixture.anchor();

    // Terrain above the base row would be overwritten by the structure body, so
    // the placement is refused and reserves nothing.
    fixture.world.set_opaque_y(Some(anchor[1] + 1));
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request("rises-into", COTTAGE, anchor, &token, 0),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));
    assert!(
        storage.settlements().active_structures(OWNER).is_empty(),
        "a refused placement reserves nothing"
    );

    // Terrain level with the base row is the ground the structure stands on.
    fixture.world.set_opaque_y(Some(anchor[1]));
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request("flush", COTTAGE, anchor, &token, 0),
        )
        .await;
    assert_eq!(outcome.failure(), None, "flush prepare: {outcome:?}");
    assert_eq!(structure_of(&outcome).state, ScriptStructureState::Prepared);

    // An unloaded footprint can never prove the volume is free.
    fixture.world.set_opaque_y(None);
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request(
                "unloaded",
                COTTAGE,
                [anchor[0] + 4, anchor[1], anchor[2]],
                &token,
                0,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Unloaded));
}

#[tokio::test]
async fn advance_rejects_a_reservation_with_a_mismatched_plan() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-5")
        .await
        .0;
    let other = ScriptInventoryResourcePlan::new(vec![ScriptInventoryWorkPortion::new(
        1,
        vec![ScriptInventoryMaterial::new(
            "minecraft:oak_planks".to_owned(),
            1,
        )],
    )]);
    reserve_plan(&mut storage, "res-other", &other);

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-mismatched",
                &structure.structure_id,
                &structure.stages[0].stage,
                "res-other",
                structure.revision,
                512,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));
    assert_eq!(fixture.world.built_total(), 0);

    // A reservation that does not exist is not found.
    let unknown = "0".repeat(64);
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-unknown",
                &structure.structure_id,
                &structure.stages[0].stage,
                &unknown,
                structure.revision,
                512,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::NotFound));

    // A stage that is not the next one is an invalid request.
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-unknown-stage",
                &structure.structure_id,
                "cellar",
                "res-other",
                structure.revision,
                512,
            ),
        )
        .await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::InvalidRequest)
    );
}

#[tokio::test]
async fn structures_are_owner_scoped_and_unknown_ids_are_not_found() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-6")
        .await
        .0;

    let outcome = fixture
        .execute(
            &mut storage,
            FOREIGN,
            &status_request(&structure.structure_id),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Forbidden));

    let outcome = fixture
        .execute(
            &mut storage,
            FOREIGN,
            &cancel_request(
                "foreign-cancel",
                &structure.structure_id,
                structure.revision,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Forbidden));

    let unknown = "0".repeat(64);
    let outcome = fixture
        .execute(
            &mut storage,
            FOREIGN,
            &advance_request(
                "foreign-advance",
                &structure.structure_id,
                &structure.stages[0].stage,
                &unknown,
                structure.revision,
                512,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Forbidden));

    let outcome = fixture
        .execute(&mut storage, OWNER, &status_request("forged-structure"))
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::NotFound));

    // The owner still sees its structure unchanged.
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &status_request(&structure.structure_id),
        )
        .await;
    let status = structure_of(&outcome);
    assert_eq!(status.state, ScriptStructureState::Prepared);
    assert_eq!(status.revision, structure.revision);
    assert_eq!(status.reserved_footprint, structure.reserved_footprint);
}

#[tokio::test]
async fn active_structure_limit_is_enforced() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let bounds = ScriptSurveyBounds::new([0, 0, 0], [7, 3, 7]).unwrap();
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &survey_request("minecraft:overworld", bounds),
        )
        .await;
    let token = survey_of(&outcome).survey_token;
    let anchor = fixture.anchor();

    for index in 0..64 {
        let at = [anchor[0], mc_world::MIN_Y + 5 * index, anchor[2]];
        let outcome = fixture
            .execute(
                &mut storage,
                OWNER,
                &prepare_request(&format!("prepare-{index}"), COTTAGE, at, &token, 0),
            )
            .await;
        assert_eq!(outcome.failure(), None, "prepare {index}: {outcome:?}");
    }

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request(
                "prepare-overflow",
                COTTAGE,
                [anchor[0], mc_world::MIN_Y + 330, anchor[2]],
                &token,
                0,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Capacity));
}

#[tokio::test]
async fn reserve_rejects_a_taken_poi_and_mints_distinct_inhabitants() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &settlement(ScriptSettlementOperation::QuerySite {
                site_id: fixture.candidate().site_id,
                cursor: None,
                limit: 64,
            }),
        )
        .await;
    let site = site_of(&outcome);
    let homes = site
        .pois
        .iter()
        .filter(|poi| poi.kind == ScriptSitePoiKind::Home)
        .collect::<Vec<_>>();
    assert!(homes.len() > 1, "a site offers more than one home slot");
    let (first, second) = (homes[0].poi_id.clone(), homes[1].poi_id.clone());

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &reserve_poi_request("reserve-first", &site.site_id, &first, site.revision),
        )
        .await;
    assert_eq!(outcome.failure(), None, "reserve: {outcome:?}");
    let reservation = resident_site_of(&outcome);
    assert_eq!(reservation.poi_id, first);
    assert_eq!(reservation.site_id, site.site_id);

    // The same slot is not reservable twice.
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &reserve_poi_request("reserve-again", &site.site_id, &first, reservation.revision),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &reserve_poi_request(
                "reserve-second",
                &site.site_id,
                &second,
                reservation.revision,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), None, "reserve: {outcome:?}");
    let other = resident_site_of(&outcome);
    assert_ne!(
        reservation.spawn_site_token, other.spawn_site_token,
        "two home slots mint two inhabitant identities"
    );

    // The ledger reports both slots as reserved at the new site revision.
    let query = settlement(ScriptSettlementOperation::QuerySite {
        site_id: site.site_id.clone(),
        cursor: None,
        limit: 64,
    });
    let outcome = fixture.execute(&mut storage, OWNER, &query).await;
    let reserved_site = site_of(&outcome);
    assert_eq!(reserved_site.revision, other.revision);
    for poi in &reserved_site.pois {
        if poi.poi_id == first || poi.poi_id == second {
            assert_eq!(poi.state, ScriptSitePoiState::Reserved);
        }
    }

    // A foreign plugin cannot release the owner's token.
    let outcome = fixture
        .execute(
            &mut storage,
            FOREIGN,
            &release_poi_request("release-foreign", &reservation.spawn_site_token),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Forbidden));

    // The owner releases it and the slot becomes free again.
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &release_poi_request("release", &reservation.spawn_site_token),
        )
        .await;
    assert_eq!(outcome.failure(), None, "release: {outcome:?}");
    assert_eq!(resident_site_of(&outcome).poi_id, first);
    let outcome = fixture.execute(&mut storage, OWNER, &query).await;
    let released_site = site_of(&outcome);
    assert_eq!(
        released_site
            .pois
            .iter()
            .find(|poi| poi.poi_id == first)
            .expect("the released poi stays on the site")
            .state,
        ScriptSitePoiState::Free
    );
    assert_eq!(
        released_site
            .pois
            .iter()
            .find(|poi| poi.poi_id == second)
            .expect("the other poi stays reserved")
            .state,
        ScriptSitePoiState::Reserved
    );
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
fn poi_state_of(site: &ScriptSettlementSite, poi_id: &str) -> ScriptSitePoiState {
    site.pois
        .iter()
        .find(|poi| poi.poi_id == poi_id)
        .expect("the poi stays on the site")
        .state
}

/// Query the fixture's first site.
async fn query_first_site(fixture: &Fixture, storage: &mut PluginStorage) -> ScriptSettlementSite {
    let outcome = fixture
        .execute(
            storage,
            OWNER,
            &settlement(ScriptSettlementOperation::QuerySite {
                site_id: fixture.candidate().site_id,
                cursor: None,
                limit: 64,
            }),
        )
        .await;
    site_of(&outcome)
}

/// Seed the durable settlement ledger with a reservation already recorded as
/// consumed: the exact state a journal written by an earlier build can hold.
fn seed_consumed_reservation(
    storage: &mut PluginStorage,
    site_id: &str,
    poi_id: &str,
    token: &str,
    revision: u64,
) {
    let mut pois = serde_json::Map::new();
    pois.insert(
        poi_id.to_owned(),
        serde_json::json!({
            "token": token,
            "plugin_id": OWNER,
            "consumed": true,
            "released": false,
        }),
    );
    let batch: PreparedStorageBatch = serde_json::from_value(serde_json::json!({
        "transaction_id": revision,
        "plugin_id": OWNER,
        "mutations": [],
        "settlement": [{
            "kind": "site",
            "site": {
                "site_id": site_id,
                "pois": serde_json::Value::Object(pois),
                "revision": revision,
            },
        }],
    }))
    .expect("a consumed settlement reservation deserialises");
    storage
        .commit_batch(batch)
        .expect("the seeded reservation is durable");
}

/// A reservation the durable settlement mirror already records as consumed can
/// never be handed back, and the refusal commits nothing and survives a reopen.
#[tokio::test]
async fn release_refuses_a_consumed_settlement_reservation() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let site = query_first_site(&fixture, &mut storage).await;
    let first = first_home(&site);
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &reserve_poi_request("reserve", &site.site_id, &first, site.revision),
        )
        .await;
    assert_eq!(outcome.failure(), None, "reserve: {outcome:?}");
    let reservation = resident_site_of(&outcome);

    seed_consumed_reservation(
        &mut storage,
        &site.site_id,
        &first,
        &reservation.spawn_site_token,
        reservation.revision + 1,
    );

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &release_poi_request("release", &reservation.spawn_site_token),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));

    // The refusal committed nothing: the home is still occupied and unreleased.
    let refused = query_first_site(&fixture, &mut storage).await;
    assert_eq!(poi_state_of(&refused, &first), ScriptSitePoiState::Occupied);

    // Reopening the journal preserves the refusal.
    let mut reopened = fixture.storage();
    let outcome = fixture
        .execute(
            &mut reopened,
            OWNER,
            &release_poi_request("release-again", &reservation.spawn_site_token),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));
}

/// An unconsumed release still hands the home back, leaving it reservable.
#[tokio::test]
async fn an_unconsumed_release_leaves_the_home_reservable() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let site = query_first_site(&fixture, &mut storage).await;
    let first = first_home(&site);
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &reserve_poi_request("reserve", &site.site_id, &first, site.revision),
        )
        .await;
    assert_eq!(outcome.failure(), None, "reserve: {outcome:?}");
    let reservation = resident_site_of(&outcome);

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &release_poi_request("release", &reservation.spawn_site_token),
        )
        .await;
    assert_eq!(outcome.failure(), None, "release: {outcome:?}");
    let released = query_first_site(&fixture, &mut storage).await;
    assert_eq!(poi_state_of(&released, &first), ScriptSitePoiState::Free);

    // The freed home is reservable again; reservations are global world state.
    let outcome = fixture
        .execute(
            &mut storage,
            FOREIGN,
            &reserve_poi_request("reserve-other", &site.site_id, &first, released.revision),
        )
        .await;
    assert_eq!(outcome.failure(), None, "re-reserve: {outcome:?}");
    assert_ne!(
        resident_site_of(&outcome).spawn_site_token,
        reservation.spawn_site_token
    );
}

#[tokio::test]
async fn settlement_operations_fail_closed_without_core_capability() {
    let bounds = ScriptSurveyBounds::new([0, 0, 0], [7, 3, 7]).unwrap();

    // No selector and no world: every settlement call is unavailable.
    let bare = Fixture::bare();
    let mut storage = bare.storage();
    for request in [
        settlement(ScriptSettlementOperation::ListSites {
            cursor: None,
            limit: 8,
        }),
        survey_request("minecraft:overworld", bounds),
        prepare_request("prepare", COTTAGE, [0, ANCHOR_Y, 0], &"0".repeat(64), 0),
    ] {
        let outcome = bare.execute(&mut storage, OWNER, &request).await;
        assert_eq!(
            outcome.failure(),
            Some(ScriptOperationFailure::RuntimeUnavailable),
            "{request:?}"
        );
    }

    // Discovery needs only the selector: a world-less runtime still lists the
    // deterministic sites, while the terrain-backed operations answer
    // unavailable rather than guessing at a surface.
    let without_world = Fixture::without_world();
    let mut storage = without_world.storage();
    let outcome = without_world
        .execute(
            &mut storage,
            OWNER,
            &settlement(ScriptSettlementOperation::ListSites {
                cursor: None,
                limit: 8,
            }),
        )
        .await;
    assert_eq!(outcome.failure(), None, "discovery: {outcome:?}");
    for request in [
        survey_request("minecraft:overworld", bounds),
        prepare_request("prepare", COTTAGE, [0, ANCHOR_Y, 0], &"0".repeat(64), 0),
    ] {
        let outcome = without_world.execute(&mut storage, OWNER, &request).await;
        assert_eq!(
            outcome.failure(),
            Some(ScriptOperationFailure::RuntimeUnavailable),
            "{request:?}"
        );
    }

    // Unloaded chunks fail a survey rather than returning a partial reading.
    let world = Fixture::new();
    let mut storage = world.storage();
    world
        .world
        .set_availability(ScriptChunkAvailability::Unloaded);
    let outcome = world
        .execute(
            &mut storage,
            OWNER,
            &survey_request("minecraft:overworld", bounds),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Unloaded));
}

/// Chunk positions covering an inclusive block rectangle.
fn covering_chunks(min_x: i32, max_x: i32, min_z: i32, max_z: i32) -> Vec<mc_world::ChunkPos> {
    let (x0, x1) = (min_x.div_euclid(16), max_x.div_euclid(16));
    let (z0, z1) = (min_z.div_euclid(16), max_z.div_euclid(16));
    let mut positions = Vec::new();
    for x in x0..=x1 {
        for z in z0..=z1 {
            positions.push(mc_world::ChunkPos { x, z });
        }
    }
    positions
}

/// The production settlement world adapter commits real blocks through the same
/// storage kernel a player edit uses; a flushed stage survives a world reopen.
#[tokio::test]
async fn live_world_stage_commit_is_durable_across_a_world_reopen() {
    use mc_world::{BlockPos, BlockStateId, Chunk, WorldStorage};

    let catalog = Arc::new(catalog());
    let plugin_root = tempfile::tempdir().unwrap();
    let world_root = tempfile::tempdir().unwrap();
    let blocks = Arc::new(stub_registry());
    let selector = SettlementSelector::new(SEED, PROFILE_REVISION);
    let candidate = selector
        .discover(START_CELL, 64)
        .into_iter()
        .next()
        .expect("the deterministic selector finds a site");
    let anchor = [candidate.origin[0], ANCHOR_Y, candidate.origin[2]];
    let survey_bounds = ScriptSurveyBounds::new(
        [anchor[0] - 4, 0, anchor[2] - 4],
        [anchor[0] + 8, 3, anchor[2] + 8],
    )
    .unwrap();
    let biome = Identifier::parse("minecraft:plains").unwrap();
    std::fs::create_dir_all(
        world_root
            .path()
            .join("dimensions/minecraft/overworld/region"),
    )
    .unwrap();
    let handle: crate::server::WorldHandle = Arc::new(tokio::sync::Mutex::new(
        WorldStorage::open(world_root.path(), Arc::clone(&blocks)).unwrap(),
    ));

    let (structure, reservation) = {
        {
            let mut world = handle.lock().await;
            for position in covering_chunks(
                survey_bounds.min[0],
                survey_bounds.max[0],
                survey_bounds.min[2],
                survey_bounds.max[2],
            ) {
                world
                    .insert_generated_chunk(
                        position,
                        Chunk::empty(position, BlockStateId(0), biome.clone()),
                    )
                    .unwrap();
            }
        }
        let read = handle.lock().await.read_view();
        let owner_read = read.clone();
        let owner_mutation = handle.lock().await.mutation_view();
        let owner_cpu = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let items = Arc::new(mc_data::items::solaris_required_items());
        let sessions = Arc::new(SessionRegistry::new());
        let (journal, _) = crate::play::world_journal::WorldChunkJournal::open_for_test(
            world_root.path(),
            Arc::clone(&blocks),
            Arc::clone(&items),
        )
        .unwrap();
        sessions.install_world_chunk_journal(journal);
        // test drives a real owner over the same world storage.
        let (simulation, mut owner) = crate::play::simulation_channel();
        let _driver = {
            let sessions = Arc::clone(&sessions);
            let world = Arc::clone(&handle);
            tokio::spawn(async move {
                while owner.wait_for_command().await {
                    owner
                        .process_commands_with_world_views(
                            &sessions,
                            Some(&world),
                            crate::play::SimulationWorldAccess {
                                read: Some(&owner_read),
                                mutation: Some(&owner_mutation),
                                cpu: Some(&owner_cpu),
                                light: None,
                            },
                            None,
                            1,
                        )
                        .await;
                }
            })
        };
        let adapter = crate::settlement::LiveSettlementWorld::new(
            read,
            Arc::clone(&blocks),
            Arc::new(mc_data::tags::TagsData::default()),
            None,
            simulation,
        );
        let runtime = InventoryRuntime::new(
            Some(world_root.path()),
            &ShutdownHandle::default(),
            Arc::clone(&sessions),
            Arc::clone(&items),
            Arc::new(mc_data::item_components::solaris_required_item_facts()),
        )
        .with_settlement_runtime(Arc::new(SettlementRuntime::new(
            selector.clone(),
            Arc::clone(&catalog),
            WORLD_IDENTITY,
            START_CELL,
            Arc::new(FlatGround),
            None,
        )))
        .with_settlement_world(Arc::new(adapter) as Arc<dyn SettlementWorld>);

        let mut storage = PluginStorage::open(plugin_root.path()).unwrap();
        let outcome = runtime
            .execute_settlement_operation(
                &mut storage,
                OWNER,
                &survey_request("minecraft:overworld", survey_bounds),
            )
            .await
            .unwrap();
        assert_eq!(outcome.failure(), None, "survey: {outcome:?}");
        let survey = survey_of(&outcome);
        assert_eq!(survey.chunk_availability, ScriptChunkAvailability::Loaded);
        assert_eq!(survey.usable_plots, 13 * 13);

        let outcome = runtime
            .execute_settlement_operation(
                &mut storage,
                OWNER,
                &prepare_request("prepare-live", COTTAGE, anchor, &survey.survey_token, 0),
            )
            .await
            .unwrap();
        assert_eq!(outcome.failure(), None, "prepare: {outcome:?}");
        let prepared = structure_of(&outcome);
        assert_eq!(prepared.state, ScriptStructureState::Prepared);

        let plan = plan_of(&prepared);
        let reserved = reserve_plan(&mut storage, "res-live", &plan);
        assert_eq!(reserved.remaining_total(), 5);

        // Advance both authored stages; every portion commits through the live
        // world adapter.
        let mut revision = prepared.revision;
        for stage in &prepared.stages {
            let outcome = runtime
                .execute_settlement_operation(
                    &mut storage,
                    OWNER,
                    &advance_request(
                        &format!("advance-{}-live", stage.stage),
                        &prepared.structure_id,
                        &stage.stage,
                        "res-live",
                        revision,
                        stage.work_units,
                    ),
                )
                .await
                .unwrap();
            assert_eq!(
                outcome.failure(),
                None,
                "advance {}: {outcome:?}",
                stage.stage
            );
            revision = receipt_of(&outcome).revision;
        }

        // The stage blocks are visible in the live world.
        let (planks_one, planks_two, planks) = {
            let mut world = handle.lock().await;
            let one = world
                .get_block(BlockPos {
                    x: anchor[0] + 1,
                    y: anchor[1] + 1,
                    z: anchor[2] + 1,
                })
                .unwrap();
            let two = world
                .get_block(BlockPos {
                    x: anchor[0] + 2,
                    y: anchor[1] + 1,
                    z: anchor[2] + 1,
                })
                .unwrap();
            let mut planks = 0_u32;
            for x in anchor[0]..anchor[0] + 4 {
                for y in anchor[1]..anchor[1] + 4 {
                    for z in anchor[2]..anchor[2] + 4 {
                        if world.get_block(BlockPos { x, y, z }).unwrap() == Some(BlockStateId(1)) {
                            planks += 1;
                        }
                    }
                }
            }
            (one, two, planks)
        };
        assert_eq!(planks_one, Some(BlockStateId(1)), "both wall blocks landed");
        assert_eq!(planks_two, Some(BlockStateId(1)));
        assert_eq!(planks, 2, "only the authored wall blocks were placed");

        // The receipt arithmetic stays inside the reservation.
        let (_, projection) = storage
            .settlement_reservation(OWNER, "res-live")
            .expect("the reservation stays projected");
        assert_eq!(projection.quantity("minecraft:stone").consumed, 3);
        assert_eq!(projection.quantity("minecraft:oak_planks").consumed, 2);

        // Flush the world exactly like the periodic dirty-flush owner does.
        assert!(handle.lock().await.flush_dirty().unwrap() > 0);

        let status = runtime
            .execute_settlement_operation(
                &mut storage,
                OWNER,
                &status_request(&prepared.structure_id),
            )
            .await
            .unwrap();
        let committed = structure_of(&status);
        assert_eq!(committed.watermark, 2, "one receipt per stage");
        assert_eq!(committed.consumed.len(), 2);
        assert_eq!(committed.current_stage_index, committed.stages.len() as u32);
        assert_eq!(committed.completed_work_units, 0);
        (prepared, projection)
    };

    // Re-open the world from disk: a committed stage is durable.
    let mut reopened = WorldStorage::open(world_root.path(), Arc::clone(&blocks)).unwrap();
    let landed = reopened
        .get_block(BlockPos {
            x: anchor[0] + 1,
            y: anchor[1] + 1,
            z: anchor[2] + 1,
        })
        .unwrap();
    assert_eq!(
        landed,
        Some(BlockStateId(1)),
        "the committed wall survives a world reopen"
    );
    drop(reopened);

    // Re-open the settlement ledger: the structure and its spent reservation
    // replay without re-materializing a second portion.
    let storage = PluginStorage::open(plugin_root.path()).unwrap();
    let (_, projection) = storage
        .settlement_reservation(OWNER, "res-live")
        .expect("the reservation replay rebuilds the projection");
    assert_eq!(projection.quantity("minecraft:oak_planks").consumed, 2);
    assert_eq!(projection.remaining_total(), 0);
    assert_eq!(reservation.remaining_total(), 0);
    assert_eq!(structure.stages.len(), 2);
}

/// The world keeps committing chunk frames for the chunk that carries a
/// structure's staged blocks: scheduled block ticks (fluid, snow, leaf decay),
/// block drops and worldgen flushes all stamp a newer durable position on that
/// chunk without the settlement pipeline writing anything. The live fence must
/// scope to the reserved footprint's blocks, not to the chunk's durable
/// position, while an edit that does land inside the footprint still parks the
/// build.
#[tokio::test]
async fn live_fence_scopes_to_the_footprint_and_not_to_the_chunk_durable_position() {
    use mc_world::{BlockPos, BlockStateId, Chunk, WorldStorage};

    let catalog = Arc::new(catalog());
    let plugin_root = tempfile::tempdir().unwrap();
    let world_root = tempfile::tempdir().unwrap();
    let blocks = Arc::new(stub_registry());
    let items = Arc::new(mc_data::items::solaris_required_items());
    let selector = SettlementSelector::new(SEED, PROFILE_REVISION);
    let candidate = selector
        .discover(START_CELL, 64)
        .into_iter()
        .next()
        .expect("the deterministic selector finds a site");
    let anchor = [candidate.origin[0], ANCHOR_Y, candidate.origin[2]];
    let survey_bounds = ScriptSurveyBounds::new(
        [anchor[0] - 4, 0, anchor[2] - 4],
        [anchor[0] + 8, 3, anchor[2] + 8],
    )
    .unwrap();
    let biome = Identifier::parse("minecraft:plains").unwrap();
    std::fs::create_dir_all(
        world_root
            .path()
            .join("dimensions/minecraft/overworld/region"),
    )
    .unwrap();
    let handle: crate::server::WorldHandle = Arc::new(tokio::sync::Mutex::new(
        WorldStorage::open(world_root.path(), Arc::clone(&blocks)).unwrap(),
    ));
    {
        let mut world = handle.lock().await;
        for position in covering_chunks(
            survey_bounds.min[0],
            survey_bounds.max[0],
            survey_bounds.min[2],
            survey_bounds.max[2],
        ) {
            world
                .insert_generated_chunk(
                    position,
                    Chunk::empty(position, BlockStateId(0), biome.clone()),
                )
                .unwrap();
        }
    }

    let sessions = Arc::new(SessionRegistry::new());
    let (journal, _) = crate::play::world_journal::WorldChunkJournal::open_for_test(
        world_root.path(),
        Arc::clone(&blocks),
        Arc::clone(&items),
    )
    .unwrap();
    // One durable decision precedes every observation, so the journal names a
    // real position instead of an empty log.
    let first_decision = journal.reserve_decision_ids(1).unwrap()[0];
    journal
        .record_reserved_snapshot_groups(1, vec![(first_decision, Vec::new())])
        .unwrap();
    sessions.install_world_chunk_journal(journal.clone());

    let read = handle.lock().await.read_view();
    let owner_read = read.clone();
    let owner_mutation = handle.lock().await.mutation_view();
    let owner_cpu = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (simulation, mut owner) = crate::play::simulation_channel();
    let _driver = {
        let sessions = Arc::clone(&sessions);
        let world = Arc::clone(&handle);
        tokio::spawn(async move {
            while owner.wait_for_command().await {
                owner
                    .process_commands_with_world_views(
                        &sessions,
                        Some(&world),
                        crate::play::SimulationWorldAccess {
                            read: Some(&owner_read),
                            mutation: Some(&owner_mutation),
                            cpu: Some(&owner_cpu),
                            light: None,
                        },
                        None,
                        1,
                    )
                    .await;
            }
        })
    };
    let adapter = crate::settlement::LiveSettlementWorld::new(
        read,
        Arc::clone(&blocks),
        Arc::new(mc_data::tags::TagsData::default()),
        None,
        simulation.clone(),
    );
    let runtime = InventoryRuntime::new(
        Some(world_root.path()),
        &ShutdownHandle::default(),
        Arc::clone(&sessions),
        Arc::clone(&items),
        Arc::new(mc_data::item_components::solaris_required_item_facts()),
    )
    .with_settlement_runtime(Arc::new(SettlementRuntime::new(
        selector.clone(),
        Arc::clone(&catalog),
        WORLD_IDENTITY,
        START_CELL,
        Arc::new(FlatGround),
        None,
    )))
    .with_settlement_world(Arc::new(adapter) as Arc<dyn SettlementWorld>);
    let mut storage = PluginStorage::open(plugin_root.path()).unwrap();

    let outcome = runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &survey_request("minecraft:overworld", survey_bounds),
        )
        .await
        .unwrap();
    assert_eq!(outcome.failure(), None, "survey: {outcome:?}");
    let survey = survey_of(&outcome);
    let outcome = runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &prepare_request("prepare-scope", COTTAGE, anchor, &survey.survey_token, 0),
        )
        .await
        .unwrap();
    assert_eq!(outcome.failure(), None, "prepare: {outcome:?}");
    let prepared = structure_of(&outcome);
    let plan = plan_of(&prepared);
    reserve_plan(&mut storage, "res-scope", &plan);

    // The first portion of the foundation. It commits its staged blocks, and the
    // structure re-reads the footprint it just built into.
    let first = &prepared.stages[0];
    let outcome = runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-scope-1",
                &prepared.structure_id,
                &first.stage,
                "res-scope",
                prepared.revision,
                2,
            ),
        )
        .await
        .unwrap();
    assert_eq!(outcome.failure(), None, "first advance: {outcome:?}");
    assert_eq!(receipt_of(&outcome).block_count, 2);
    let revision = receipt_of(&outcome).revision;

    // A durable chunk frame lands in the footprint's own chunk exactly the way
    // the world commits one: a scheduled block tick edits a block of that chunk
    // that is outside the reserved footprint, through the resident transaction
    // kernel, and journals the chunk image it stamps.
    commit_world_chunk_frame_outside_footprint(&handle, &journal, &prepared, anchor).await;

    let outcome = runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-scope-2",
                &prepared.structure_id,
                &first.stage,
                "res-scope",
                revision,
                2,
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        outcome.failure(),
        None,
        "a world write in the footprint's chunk but outside the footprint is not a site change: {outcome:?}"
    );
    assert_eq!(receipt_of(&outcome).block_count, 1);
    let revision = receipt_of(&outcome).revision;

    let next = prepared
        .stages
        .get(1)
        .expect("the cottage has a second stage to advance");
    // Half of the second stage, so a later advance can still fence it.
    let first_half_of_next_stage = next.work_units / 2;
    let outcome = runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-scope-3",
                &prepared.structure_id,
                &next.stage,
                "res-scope",
                revision,
                first_half_of_next_stage,
            ),
        )
        .await
        .unwrap();
    assert_eq!(outcome.failure(), None, "third advance: {outcome:?}");

    // An edit inside the reserved footprint is a site change and parks the
    // build before it commits anything.
    let footprint_corner = [
        prepared.origin[0] + prepared.reserved_footprint[0] - 1,
        prepared.origin[1] + prepared.reserved_footprint[1] - 1,
        prepared.origin[2] + prepared.reserved_footprint[2] - 1,
    ];
    let revision = receipt_of(&outcome).revision;
    let landed = simulation
        .apply_server_owned_block_edits(
            FOREIGN,
            vec![crate::play::BlockEdit::new(
                BlockPos {
                    x: footprint_corner[0],
                    y: footprint_corner[1],
                    z: footprint_corner[2],
                },
                BlockStateId(1),
            )],
            None,
        )
        .await
        .unwrap();
    assert!(landed.is_some(), "the foreign edit landed in the world");
    let outcome = runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-scope-4",
                &prepared.structure_id,
                &next.stage,
                "res-scope",
                revision,
                first_half_of_next_stage,
            ),
        )
        .await
        .unwrap();
    assert_eq!(outcome.failure(), None, "fourth advance: {outcome:?}");
    let paused = structure_of(&outcome);
    assert_eq!(paused.state, ScriptStructureState::Paused);
    assert_eq!(paused.pause_reason.as_deref(), Some("site_changed"));
}

/// Commit one scheduled-block-tick chunk frame the way
/// `run_scheduled_block_ticks_owned` does: reserve a decision, prepare the
/// resident transaction, then journal the chunk image the commit stamps.
async fn commit_world_chunk_frame_outside_footprint(
    handle: &crate::server::WorldHandle,
    journal: &crate::play::world_journal::WorldChunkJournal,
    structure: &ScriptStructureSnapshot,
    anchor: [i32; 3],
) {
    use mc_world::{BlockPos, BlockStateId};

    let chunk_origin = [anchor[0].div_euclid(16) * 16, anchor[2].div_euclid(16) * 16];
    let x = if anchor[0] - chunk_origin[0] < 8 {
        chunk_origin[0] + 15
    } else {
        chunk_origin[0]
    };
    let z = if anchor[2] - chunk_origin[1] < 8 {
        chunk_origin[1] + 15
    } else {
        chunk_origin[1]
    };
    let position = BlockPos { x, y: anchor[1], z };
    assert!(
        x < structure.origin[0]
            || x > structure.origin[0] + structure.reserved_footprint[0] - 1
            || z < structure.origin[2]
            || z > structure.origin[2] + structure.reserved_footprint[2] - 1,
        "the tick must edit outside the reserved footprint"
    );

    let read = handle.lock().await.read_view();
    let snapshot = read.snapshot_chunks(&[mc_world::ChunkPos {
        x: x.div_euclid(16),
        z: z.div_euclid(16),
    }]);
    let expected_state = snapshot
        .get_cached_block(position)
        .expect("the tick position is loaded");
    let expected_token = snapshot
        .block_mutation_token(position)
        .expect("the tick position carries a token");
    let mutation = handle.lock().await.mutation_view();

    let decision_id = journal.reserve_decision_ids(1).unwrap()[0];
    journal.wait_for_append_turn(decision_id).await.unwrap();
    let edits = [mc_world::ResidentBlockEdit {
        pos: position,
        new_state: BlockStateId(1),
        preserve_light: false,
    }];
    let preconditions = [mc_world::ResidentBlockPrecondition {
        pos: position,
        expected_state,
        expected_token,
    }];
    let prepared = mutation.prepare_cross_region_scheduled_block_tick_transaction(
        Some(decision_id),
        &mc_world::ResidentScheduledBlockTickPlan {
            consumed_ticks: &[],
            edits: &edits,
            preconditions: &preconditions,
            light_table: None,
            leaf_trigger_tick: None,
        },
    );
    let mc_world::resident::ResidentCrossRegionScheduledBlockTickPrepareResult::Prepared(
        transaction,
    ) = prepared
    else {
        panic!("the world chunk frame scheduled tick did not prepare a transaction");
    };
    let committed = transaction.commit_durably(|snapshots| {
        journal.record_reserved_snapshot_groups(2, vec![(decision_id, snapshots)])
    });
    assert!(
        matches!(
            committed,
            mc_world::resident::ResidentCrossRegionScheduledBlockTickCommitResult::Applied(_)
        ),
        "the world chunk frame committed"
    );
}

/// Survey the fixture's first site and prepare the authored-container
/// warehouse inside it.
async fn prepared_warehouse(
    fixture: &Fixture,
    storage: &mut PluginStorage,
    operation_id: &str,
) -> ScriptStructureSnapshot {
    let bounds = ScriptSurveyBounds::new([0, 0, 0], [7, 3, 7]).unwrap();
    let outcome = fixture
        .execute(
            storage,
            OWNER,
            &survey_request("minecraft:overworld", bounds),
        )
        .await;
    assert_eq!(outcome.failure(), None, "survey: {outcome:?}");
    let token = survey_of(&outcome).survey_token;
    let outcome = fixture
        .execute(
            storage,
            OWNER,
            &prepare_request(operation_id, WAREHOUSE, fixture.anchor(), &token, 0),
        )
        .await;
    assert_eq!(outcome.failure(), None, "prepare: {outcome:?}");
    structure_of(&outcome)
}

/// World position of the `offset`-th authored container of a rotation-zero
/// warehouse placed at the fixture anchor; index 0 is authored at [1, 1, 1].
fn container_position(fixture: &Fixture, offset: i32) -> [i32; 3] {
    let anchor = fixture.anchor();
    [anchor[0] + offset, anchor[1] + offset, anchor[2] + offset]
}

const BIRCH_LOG: u32 = 136;

#[tokio::test]
async fn warehouse_read_returns_the_bound_loaded_container() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_warehouse(&fixture, &mut storage, "prepare-wh").await;
    let mut chest = vec![ItemStack::EMPTY; 27];
    chest[0] = ItemStack::new(BIRCH_LOG, 12);
    fixture
        .world
        .set_container(container_position(&fixture, 1), chest);

    let bind = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-wh", &structure.structure_id, 0),
        )
        .await;
    assert_eq!(bind.failure(), None, "bind: {bind:?}");
    let binding = warehouse_of(&bind);
    let revision = bind
        .revision()
        .expect("a committed bind carries a revision");
    assert!(matches!(
        &binding.source,
        ScriptWarehouseSource::Authored {
            structure_id,
            container_id,
            ..
        } if structure_id == &structure.structure_id && *container_id == 0
    ));
    assert_eq!(binding.revision, revision);

    let read = fixture
        .runtime
        .execute_owned_inventory(
            &mut storage,
            OWNER,
            &warehouse_query_request(&binding.handle, Some(revision)),
        )
        .await
        .expect("a warehouse query reaches the durable boundary");
    assert_eq!(read.failure(), None, "read: {read:?}");
    let snapshot = owned_snapshot_of(&read);
    assert_eq!(snapshot.fence.revision, revision);
    assert_eq!(
        snapshot.endpoint,
        ScriptInventoryEndpoint::Warehouse {
            handle: binding.handle.clone(),
        }
    );
    assert_eq!(snapshot.slots.len(), 27, "a whole chest is reported");
    assert_eq!(snapshot.slots[0].slot, 0);
    let item = snapshot.slots[0]
        .item
        .as_ref()
        .expect("the seeded stack is reported");
    assert_eq!(item.resource_id, "minecraft:birch_log");
    assert_eq!(item.count, 12);
    assert!(
        snapshot.slots[1].item.is_none(),
        "an empty slot is reported empty, never omitted"
    );

    // A repeated identical bind and a repeat under a fresh operation id are the
    // same binding: same handle, same revision, no second ledger entry — and
    // each accepted request still records a receipt keyed by its operation id,
    // so a plugin's durable intent recovers through `operation_status`.
    for operation_id in ["bind-wh", "bind-wh-again"] {
        let repeat = fixture
            .execute(
                &mut storage,
                OWNER,
                &bind_warehouse_request(operation_id, &structure.structure_id, 0),
            )
            .await;
        let repeated = warehouse_of(&repeat);
        assert_eq!(repeated, binding, "repeat {operation_id} is idempotent");
        let receipt = storage
            .operation_receipt(OWNER, operation_id)
            .unwrap_or_else(|| panic!("{operation_id} records its receipt"));
        assert_eq!(
            receipt.outcome, repeat,
            "{operation_id} is recoverable by operation_status"
        );
        assert_eq!(
            warehouse_of(&receipt.outcome).revision,
            revision,
            "{operation_id} keeps the minted binding revision"
        );
    }

    // A stale fence never reads the container.
    let stale = fixture
        .runtime
        .execute_owned_inventory(
            &mut storage,
            OWNER,
            &warehouse_query_request(&binding.handle, Some(revision + 1)),
        )
        .await
        .unwrap();
    assert_eq!(stale.failure(), Some(ScriptOperationFailure::StaleRevision));
}

#[tokio::test]
async fn warehouse_reservations_hold_live_stock_across_restart_and_reject_overpromise() {
    let fixture = Fixture::with_journal();
    let mut storage = fixture.storage();
    let structure = prepared_warehouse(&fixture, &mut storage, "prepare-reserve-wh").await;
    let position = container_position(&fixture, 1);
    let mut chest = vec![ItemStack::EMPTY; 27];
    chest[0] = ItemStack::new(BIRCH_LOG, 12);
    fixture.world.set_container(position, chest);

    let bind = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-reserve-wh", &structure.structure_id, 0),
        )
        .await;
    assert_eq!(bind.failure(), None, "bind: {bind:?}");
    let binding = warehouse_of(&bind);
    let snapshot = fixture
        .execute_inventory(
            &mut storage,
            OWNER,
            &warehouse_query_request(&binding.handle, Some(binding.revision)),
        )
        .await;
    assert_eq!(snapshot.failure(), None, "snapshot: {snapshot:?}");
    let expected = owned_snapshot_of(&snapshot).fence.clone();
    let plan = |quantity| {
        ScriptInventoryResourcePlan::new(vec![ScriptInventoryWorkPortion::new(
            1,
            vec![ScriptInventoryMaterial::new(
                "minecraft:birch_log".to_owned(),
                quantity,
            )],
        )])
    };

    // Two construction projects reserve all twelve real logs. A third request
    // must not turn a stale warehouse image into a material promise.
    for (operation_id, quantity) in [("reserve-wh-one", 7), ("reserve-wh-two", 5)] {
        let outcome = fixture
            .execute_inventory(
                &mut storage,
                OWNER,
                &warehouse_reserve_request(
                    operation_id,
                    &binding.handle,
                    plan(quantity),
                    expected.clone(),
                ),
            )
            .await;
        assert_eq!(outcome.failure(), None, "{operation_id}: {outcome:?}");
    }
    let overpromised = fixture
        .execute_inventory(
            &mut storage,
            OWNER,
            &warehouse_reserve_request(
                "reserve-wh-overpromise",
                &binding.handle,
                plan(1),
                expected.clone(),
            ),
        )
        .await;
    assert_eq!(
        overpromised.failure(),
        Some(ScriptOperationFailure::InsufficientItems)
    );
    fixture.world.clear_container(position);
    let destroyed = fixture
        .execute_inventory(
            &mut storage,
            OWNER,
            &warehouse_reserve_request("reserve-wh-destroyed", &binding.handle, plan(1), expected),
        )
        .await;
    assert_eq!(destroyed.failure(), Some(ScriptOperationFailure::NotFound));

    drop(storage);
    let Fixture {
        root,
        runtime,
        world,
        catalog,
        sessions,
    } = fixture;
    drop(runtime);
    drop(world);
    drop(sessions);

    let restored_sessions = Arc::new(SessionRegistry::new());
    let restored_world = Arc::new(FakeWorld::new());
    restored_world.set_container(position, vec![ItemStack::new(BIRCH_LOG, 12)]);
    restored_world.set_availability(ScriptChunkAvailability::Unloaded);
    let restored = runtime_at_with(
        catalog,
        Arc::clone(&restored_world),
        Arc::clone(&restored_sessions),
        Some(root.path()),
        true,
        true,
    );
    let mut reopened = PluginStorage::open(root.path()).unwrap();
    restored.recover(&mut reopened).unwrap();
    restored_world.set_availability(ScriptChunkAvailability::Loaded);
    let mut after = mc_world::ChestBlockEntity::default();
    after.slots[0].item_id = BIRCH_LOG;
    after.slots[0].count = 11;
    let position = mc_world::BlockPos {
        x: position[0],
        y: position[1],
        z: position[2],
    };
    assert!(
        !restored_sessions.warehouse_reservation_stock_survives(&[position], &[after]),
        "the recovered physical floor still protects all twelve promised logs"
    );
}

#[tokio::test]
async fn warehouse_bind_refuses_foreign_unknown_inactive_and_unloaded() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_warehouse(&fixture, &mut storage, "prepare-wh-refuse").await;
    fixture.world.set_container(
        container_position(&fixture, 1),
        vec![ItemStack::new(BIRCH_LOG, 1)],
    );

    // A foreign plugin neither owns the structure nor may bind its container.
    let foreign = fixture
        .execute(
            &mut storage,
            FOREIGN,
            &bind_warehouse_request("bind-foreign", &structure.structure_id, 0),
        )
        .await;
    assert_eq!(foreign.failure(), Some(ScriptOperationFailure::Forbidden));

    let unknown = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-unknown", "0".repeat(64).as_str(), 0),
        )
        .await;
    assert_eq!(unknown.failure(), Some(ScriptOperationFailure::NotFound));

    // An ordinal past the authored seeds names no container.
    let ordinal = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-ordinal", &structure.structure_id, 9),
        )
        .await;
    assert_eq!(ordinal.failure(), Some(ScriptOperationFailure::NotFound));

    // The authored position exists, but the chunk is not observable.
    fixture
        .world
        .set_availability(ScriptChunkAvailability::Unloaded);
    let unloaded = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-unloaded", &structure.structure_id, 0),
        )
        .await;
    assert_eq!(unloaded.failure(), Some(ScriptOperationFailure::Unloaded));
    fixture
        .world
        .set_availability(ScriptChunkAvailability::Loaded);

    // A cancelled structure is no longer active and cannot issue handles.
    let cancelled = fixture
        .execute(
            &mut storage,
            OWNER,
            &cancel_request("cancel-wh", &structure.structure_id, structure.revision),
        )
        .await;
    assert_eq!(cancelled.failure(), None, "cancel: {cancelled:?}");
    let inactive = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-inactive", &structure.structure_id, 0),
        )
        .await;
    assert_eq!(inactive.failure(), Some(ScriptOperationFailure::Blocked));
}

#[tokio::test]
async fn warehouse_read_refuses_foreign_unknown_inactive_and_unloaded() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_warehouse(&fixture, &mut storage, "prepare-wh-read").await;
    fixture.world.set_container(
        container_position(&fixture, 1),
        vec![ItemStack::new(BIRCH_LOG, 3)],
    );
    let bind = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-wh-read", &structure.structure_id, 0),
        )
        .await;
    let binding = warehouse_of(&bind);

    let unknown = fixture
        .runtime
        .execute_owned_inventory(
            &mut storage,
            OWNER,
            &warehouse_query_request("warehouse:missing", None),
        )
        .await
        .unwrap();
    assert_eq!(unknown.failure(), Some(ScriptOperationFailure::NotFound));

    let foreign = fixture
        .runtime
        .execute_owned_inventory(
            &mut storage,
            FOREIGN,
            &warehouse_query_request(&binding.handle, None),
        )
        .await
        .unwrap();
    assert_eq!(foreign.failure(), Some(ScriptOperationFailure::Forbidden));

    // The container block/entity is gone: loaded, but not a container.
    fixture
        .world
        .clear_container(container_position(&fixture, 1));
    let missing = fixture
        .runtime
        .execute_owned_inventory(
            &mut storage,
            OWNER,
            &warehouse_query_request(&binding.handle, None),
        )
        .await
        .unwrap();
    assert_eq!(missing.failure(), Some(ScriptOperationFailure::NotFound));

    fixture
        .world
        .set_availability(ScriptChunkAvailability::Unloaded);
    let unloaded = fixture
        .runtime
        .execute_owned_inventory(
            &mut storage,
            OWNER,
            &warehouse_query_request(&binding.handle, None),
        )
        .await
        .unwrap();
    assert_eq!(unloaded.failure(), Some(ScriptOperationFailure::Unloaded));
    fixture
        .world
        .set_availability(ScriptChunkAvailability::Loaded);

    let cancelled = fixture
        .execute(
            &mut storage,
            OWNER,
            &cancel_request(
                "cancel-wh-read",
                &structure.structure_id,
                structure.revision,
            ),
        )
        .await;
    assert_eq!(cancelled.failure(), None, "cancel: {cancelled:?}");
    let inactive = fixture
        .runtime
        .execute_owned_inventory(
            &mut storage,
            OWNER,
            &warehouse_query_request(&binding.handle, None),
        )
        .await
        .unwrap();
    assert_eq!(inactive.failure(), Some(ScriptOperationFailure::Blocked));
}

#[tokio::test]
async fn warehouse_binding_survives_a_journal_reopen() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_warehouse(&fixture, &mut storage, "prepare-wh-reopen").await;
    let mut chest = vec![ItemStack::EMPTY; 27];
    chest[0] = ItemStack::new(BIRCH_LOG, 7);
    fixture
        .world
        .set_container(container_position(&fixture, 2), chest.clone());
    let bind = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-wh-reopen", &structure.structure_id, 1),
        )
        .await;
    let binding = warehouse_of(&bind);
    let revision = bind
        .revision()
        .expect("a committed bind carries a revision");
    drop(storage);

    // A second runtime over the same journal, with a fresh world.
    let (runtime, world) = fixture.reopened();
    world.set_container(container_position(&fixture, 2), chest);
    let mut storage = fixture.storage();
    let read = runtime
        .execute_owned_inventory(
            &mut storage,
            OWNER,
            &warehouse_query_request(&binding.handle, None),
        )
        .await
        .unwrap();
    assert_eq!(read.failure(), None, "reopened read: {read:?}");
    let snapshot = owned_snapshot_of(&read);
    assert_eq!(
        snapshot.fence.revision, revision,
        "the binding revision replays"
    );
    assert_eq!(snapshot.slots.len(), 27);
    assert_eq!(snapshot.slots[0].item.as_ref().unwrap().count, 7);

    // The reopened ledger also still answers the identical bind.
    let repeat = runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-wh-reopen-again", &structure.structure_id, 1),
        )
        .await
        .unwrap();
    assert_eq!(warehouse_of(&repeat), binding);
}

/// A warehouse handle is a handle on a *placed* structure, not merely one under
/// construction: a container bound while building stays readable after the
/// structure completes, and a container of an already-completed structure can
/// still be bound and read. Only an unknown structure is not placed at all.
#[tokio::test]
async fn warehouse_survives_structure_completion_and_binds_after_it() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_warehouse(&fixture, &mut storage, "prepare-wh-complete").await;
    let mut first = vec![ItemStack::EMPTY; 27];
    first[0] = ItemStack::new(BIRCH_LOG, 4);
    let mut second = vec![ItemStack::EMPTY; 27];
    second[0] = ItemStack::new(BIRCH_LOG, 9);
    fixture
        .world
        .set_container(container_position(&fixture, 1), first);
    fixture
        .world
        .set_container(container_position(&fixture, 2), second);

    // Bind while the structure is still under construction, then finish it.
    let bind = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-wh-during", &structure.structure_id, 0),
        )
        .await;
    assert_eq!(bind.failure(), None, "bind during construction: {bind:?}");
    let binding = warehouse_of(&bind);
    let plan = plan_of(&structure);
    let reserved = reserve_plan(&mut storage, "res-wh-complete", &plan);
    assert_invariant(&reserved.quantities);
    let advance = fixture
        .execute(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-wh-complete",
                &structure.structure_id,
                &structure.stages[0].stage,
                "res-wh-complete",
                structure.revision,
                structure.stages[0].work_units,
            ),
        )
        .await;
    assert_eq!(advance.failure(), None, "advance: {advance:?}");
    let status = fixture
        .execute(
            &mut storage,
            OWNER,
            &status_request(&structure.structure_id),
        )
        .await;
    assert_eq!(
        structure_of(&status).state,
        ScriptStructureState::Committed,
        "the structure completed"
    );

    // The handle minted during construction still reads the completed container.
    let read = fixture
        .runtime
        .execute_owned_inventory(
            &mut storage,
            OWNER,
            &warehouse_query_request(&binding.handle, None),
        )
        .await
        .unwrap();
    assert_eq!(read.failure(), None, "read after completion: {read:?}");
    assert_eq!(
        owned_snapshot_of(&read).slots[0]
            .item
            .as_ref()
            .unwrap()
            .count,
        4
    );

    // A container of the already-completed structure binds and reads.
    let completed_bind = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-wh-after", &structure.structure_id, 1),
        )
        .await;
    assert_eq!(
        completed_bind.failure(),
        None,
        "bind after completion: {completed_bind:?}"
    );
    let completed = warehouse_of(&completed_bind);
    assert_ne!(completed.handle, binding.handle, "one handle per container");
    let read = fixture
        .runtime
        .execute_owned_inventory(
            &mut storage,
            OWNER,
            &warehouse_query_request(&completed.handle, None),
        )
        .await
        .unwrap();
    assert_eq!(read.failure(), None, "read completed bind: {read:?}");
    assert_eq!(
        owned_snapshot_of(&read).slots[0]
            .item
            .as_ref()
            .unwrap()
            .count,
        9
    );

    // A structure that was never placed refuses; it is not a warehouse.
    let unplaced = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-unplaced", &"f".repeat(64), 0),
        )
        .await;
    assert_eq!(unplaced.failure(), Some(ScriptOperationFailure::NotFound));
}

/// One warehouse transfer request: the container's handle, the actor it acts
/// for, and the slot-to-slot moves between them.
fn warehouse_transfer_request(
    operation_id: &str,
    actor_id: u64,
    transfers: Vec<ScriptOwnedItemTransfer>,
    expected_revisions: Vec<ScriptInventoryExpectedRevision>,
) -> ScriptOperationRequest {
    let transfers_debug = transfers.clone();
    let fences_debug = expected_revisions.clone();
    ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::Transfer {
                operation_id: operation_id.to_owned(),
                actor_id,
                transfers,
                expected_revisions,
            },
        },
    )
    .unwrap_or_else(|error| {
        panic!(
            "a warehouse transfer is a valid request: {error:?} transfers={transfers:?} fences={expected_revisions:?}",
            transfers = transfers_debug,
            expected_revisions = fences_debug,
        )
    })
}

/// The fence one endpoint reports for an observed state, exactly as the read
/// path hands it to a plugin.
fn owned_fence(
    endpoint: &ScriptInventoryEndpoint,
    revision: u64,
    slots: &[ItemStack],
) -> ScriptInventoryFence {
    crate::play::owned_inventory::owned_inventory_snapshot(
        endpoint.clone(),
        revision,
        slots,
        &solaris_required_items(),
    )
    .expect("an observed endpoint is a canonical window")
    .fence
}

/// The resulting fences of one transfer receipt.
fn transfer_fences(outcome: &ScriptOperationOutcome) -> Vec<ScriptInventoryExpectedRevision> {
    match outcome.payload() {
        ScriptOperationPayload::OwnedInventory { result } => match &**result {
            ScriptOwnedInventoryResult::Transfer { inventories } => inventories.clone(),
            other => panic!("expected a transfer payload, got {other:?}"),
        },
        other => panic!("expected an owned inventory payload, got {other:?}"),
    }
}

/// A warehouse write resolves through the same durable binding and loaded
/// container the read path uses, and every refusal answers its own family
/// without moving an item.
#[tokio::test]
async fn warehouse_transfer_refuses_foreign_unknown_unloaded_and_stale_containers() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_warehouse(&fixture, &mut storage, "prepare-write").await;
    let mut chest = vec![ItemStack::EMPTY; 27];
    chest[0] = ItemStack::new(BIRCH_LOG, 12);
    let position = container_position(&fixture, 1);
    fixture.world.set_container(position, chest.clone());
    let bind = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-write", &structure.structure_id, 0),
        )
        .await;
    let binding = warehouse_of(&bind);
    let revision = bind
        .revision()
        .expect("a committed bind carries a revision");
    let warehouse = ScriptInventoryEndpoint::Warehouse {
        handle: binding.handle.clone(),
    };
    let player = ScriptInventoryEndpoint::PlayerInventory { player_id: PLAYER };
    let fence = owned_fence(&warehouse, revision, &chest);
    let deposit = vec![ScriptOwnedItemTransfer::new(
        player.clone(),
        9,
        warehouse.clone(),
        1,
        4,
    )];
    let player_fence = ScriptInventoryExpectedRevision::new(player.clone(), fence.clone());

    // A handle core never minted, and a handle another plugin owns. The
    // request's fences must name the endpoints of its own transfers, so the
    // unknown handle is the one the transfer addresses.
    for (plugin, handle, failure) in [
        (
            OWNER,
            "warehouse:settlement:nobody:0".to_owned(),
            ScriptOperationFailure::NotFound,
        ),
        (
            FOREIGN,
            binding.handle.clone(),
            ScriptOperationFailure::Forbidden,
        ),
    ] {
        let named = ScriptInventoryEndpoint::Warehouse {
            handle: handle.clone(),
        };
        let request = warehouse_transfer_request(
            "write-unknown",
            PLAYER,
            vec![ScriptOwnedItemTransfer::new(
                player.clone(),
                9,
                named.clone(),
                1,
                4,
            )],
            vec![
                player_fence.clone(),
                ScriptInventoryExpectedRevision::new(named, fence.clone()),
            ],
        );
        let outcome = fixture
            .execute_inventory(&mut storage, plugin, &request)
            .await;
        assert_eq!(outcome.failure(), Some(failure), "handle {handle}");
        assert!(
            storage.operation_receipt(plugin, "write-unknown").is_none(),
            "a refused deposit records no receipt"
        );
    }

    // A container fence the plugin no longer holds.
    let moved = {
        let mut moved = chest.clone();
        moved[0] = ItemStack::new(BIRCH_LOG, 11);
        moved
    };
    let stale = owned_fence(&warehouse, revision, &moved);
    let request = warehouse_transfer_request(
        "write-stale",
        PLAYER,
        deposit.clone(),
        vec![
            player_fence.clone(),
            ScriptInventoryExpectedRevision::new(warehouse.clone(), stale),
        ],
    );
    let outcome = fixture
        .execute_inventory(&mut storage, OWNER, &request)
        .await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::StaleRevision)
    );

    // A resident endpoint is not a participant of this composite, even when
    // every endpoint of the request is fenced as the DTO requires.
    let resident = ScriptInventoryEndpoint::ResidentCarry {
        handle: "resident-1".to_owned(),
    };
    let request = warehouse_transfer_request(
        "write-resident",
        PLAYER,
        vec![ScriptOwnedItemTransfer::new(
            resident.clone(),
            0,
            warehouse.clone(),
            1,
            1,
        )],
        vec![
            ScriptInventoryExpectedRevision::new(
                resident.clone(),
                owned_fence(&resident, 0, &[ItemStack::EMPTY; 8]),
            ),
            ScriptInventoryExpectedRevision::new(warehouse.clone(), fence.clone()),
        ],
    );
    let outcome = fixture
        .execute_inventory(&mut storage, OWNER, &request)
        .await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::InvalidRequest)
    );

    // Loaded terrain whose container is gone, and a chunk core cannot observe.
    fixture.world.clear_container(position);
    let request = warehouse_transfer_request(
        "write-missing",
        PLAYER,
        deposit.clone(),
        vec![
            player_fence.clone(),
            ScriptInventoryExpectedRevision::new(warehouse.clone(), fence.clone()),
        ],
    );
    let outcome = fixture
        .execute_inventory(&mut storage, OWNER, &request)
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::NotFound));

    fixture.world.set_container(position, chest.clone());
    fixture
        .world
        .set_availability(ScriptChunkAvailability::Unloaded);
    let request = warehouse_transfer_request(
        "write-unloaded",
        PLAYER,
        deposit.clone(),
        vec![
            player_fence.clone(),
            ScriptInventoryExpectedRevision::new(warehouse.clone(), fence.clone()),
        ],
    );
    let outcome = fixture
        .execute_inventory(&mut storage, OWNER, &request)
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Unloaded));
    fixture
        .world
        .set_availability(ScriptChunkAvailability::Loaded);

    // No refusal moved an item: the container still holds exactly what it held.
    let read = fixture
        .runtime
        .execute_owned_inventory(
            &mut storage,
            OWNER,
            &warehouse_query_request(&binding.handle, None),
        )
        .await
        .unwrap();
    let snapshot = owned_snapshot_of(&read);
    assert_eq!(
        snapshot.slots[0].item.as_ref().map(|item| item.count),
        Some(12),
        "no refusal moved an item"
    );

    // A cancelled structure stops being warehouse-addressable.
    let cancel = fixture
        .execute(
            &mut storage,
            OWNER,
            &cancel_request("cancel-write", &structure.structure_id, structure.revision),
        )
        .await;
    assert_eq!(cancel.failure(), None, "cancel: {cancel:?}");
    let request = warehouse_transfer_request(
        "write-cancelled",
        PLAYER,
        deposit,
        vec![
            player_fence,
            ScriptInventoryExpectedRevision::new(warehouse, fence),
        ],
    );
    let outcome = fixture
        .execute_inventory(&mut storage, OWNER, &request)
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));
}

/// A warehouse deposit moves real container slots and the actor's canonical
/// inventory under ONE journal decision, and the plugin's stored receipt names
/// the container's resulting fence.
#[tokio::test]
async fn warehouse_transfer_commits_both_participants_under_one_decision() {
    let fixture = Fixture::with_journal();
    let mut storage = fixture.storage();
    let structure = prepared_warehouse(&fixture, &mut storage, "prepare-write").await;
    let mut chest = vec![ItemStack::EMPTY; 27];
    chest[0] = ItemStack::new(BIRCH_LOG, 12);
    let position = container_position(&fixture, 1);
    fixture.world.set_container(position, chest.clone());
    let bind = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-write", &structure.structure_id, 0),
        )
        .await;
    let binding = warehouse_of(&bind);
    let revision = bind
        .revision()
        .expect("a committed bind carries a revision");
    let warehouse = ScriptInventoryEndpoint::Warehouse {
        handle: binding.handle.clone(),
    };
    let actor = fixture.register_actor("WarehouseSettler", &[(9, BIRCH_LOG, 10)]);
    let player = ScriptInventoryEndpoint::PlayerInventory { player_id: actor };
    let (actor_inventory, actor_revision) = fixture.actor_inventory(actor);
    let request = warehouse_transfer_request(
        "write-deposit",
        actor,
        vec![ScriptOwnedItemTransfer::new(
            player.clone(),
            9,
            warehouse.clone(),
            1,
            4,
        )],
        vec![
            ScriptInventoryExpectedRevision::new(
                player.clone(),
                owned_fence(&player, actor_revision, &actor_inventory),
            ),
            ScriptInventoryExpectedRevision::new(
                warehouse.clone(),
                owned_fence(&warehouse, revision, &chest),
            ),
        ],
    );
    let outcome = fixture
        .execute_inventory(&mut storage, OWNER, &request)
        .await;
    assert_eq!(outcome.failure(), None, "deposit: {outcome:?}");

    // The container's own slots moved, and the receipt names the fence a
    // subsequent warehouse read reports.
    let read = fixture
        .runtime
        .execute_owned_inventory(
            &mut storage,
            OWNER,
            &warehouse_query_request(&binding.handle, None),
        )
        .await
        .unwrap();
    let snapshot = owned_snapshot_of(&read);
    let slot = snapshot.slots[1]
        .item
        .as_ref()
        .expect("the deposited stack is in the container");
    assert_eq!(slot.resource_id, "minecraft:birch_log");
    assert_eq!(slot.count, 4);
    assert_eq!(
        snapshot.slots[0].item.as_ref().map(|item| item.count),
        Some(12),
        "the rest of the container is untouched"
    );
    let fences = transfer_fences(&outcome);
    assert_eq!(fences.len(), 1);
    assert_eq!(fences[0].endpoint, warehouse);
    assert_eq!(fences[0].fence, snapshot.fence);

    // The world half was handed the observed and the planned actor inventory,
    // and the actor's durable revision is the decision the composite journaled:
    // the fence a later transfer round-trips.
    let accepted = fixture.world.last_warehouse_request();
    let participant = accepted
        .player
        .as_ref()
        .expect("the deposit moves the actor's own inventory");
    assert_eq!(participant.actor_id, actor);
    assert_eq!(
        participant.expected_inventory[9],
        ItemStack::new(BIRCH_LOG, 10)
    );
    assert_eq!(
        participant.updated_inventory[9],
        ItemStack::new(BIRCH_LOG, 6)
    );
    assert_eq!(accepted.updated_container[1], ItemStack::new(BIRCH_LOG, 4));
    let (_after, after_revision) = fixture.actor_inventory(actor);
    let sessions = fixture.sessions.as_ref().unwrap();
    let journal = sessions.world_chunk_journal().unwrap();
    let pending = journal.pending_decisions_for_test();
    assert_eq!(pending.len(), 1, "one deposit, one decision");
    assert_eq!(
        pending[0].id(),
        after_revision,
        "the actor's revision is the decision the receipt rode"
    );
    assert!(
        pending[0].inventory_batch().unwrap().is_some(),
        "the receipt rides the container's own decision"
    );
    assert_eq!(
        journal.watermark(),
        Some(pending[0].id()),
        "the decision is acknowledged once both participants are projected"
    );
    assert!(
        storage.operation_receipt(OWNER, "write-deposit").is_some(),
        "a committed deposit records its receipt"
    );

    // A stale actor fence and a recovering actor each refuse without moving
    // anything; the container's content is unchanged by both.
    // A stale actor fence: the same revision, but the state this plugin read is
    // no longer the actor's.
    let (_actor_slots, actor_revision) = fixture.actor_inventory(actor);
    let request = warehouse_transfer_request(
        "write-stale-actor",
        actor,
        vec![ScriptOwnedItemTransfer::new(
            player.clone(),
            9,
            warehouse.clone(),
            2,
            1,
        )],
        vec![
            ScriptInventoryExpectedRevision::new(
                player.clone(),
                owned_fence(&player, actor_revision, &[ItemStack::EMPTY; 46]),
            ),
            ScriptInventoryExpectedRevision::new(
                warehouse.clone(),
                owned_fence(&warehouse, revision, &chest),
            ),
        ],
    );
    let outcome = fixture
        .execute_inventory(&mut storage, OWNER, &request)
        .await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::StaleRevision)
    );

    let read = fixture
        .runtime
        .execute_owned_inventory(
            &mut storage,
            OWNER,
            &warehouse_query_request(&binding.handle, None),
        )
        .await
        .unwrap();
    let snapshot = owned_snapshot_of(&read);
    assert_eq!(
        snapshot.slots[1].item.as_ref().map(|item| item.count),
        Some(4),
        "a refused deposit leaves the container as it was"
    );
}
