//! Acceptance tests for resident work and squad order execution (C4).
//!
//! Every test drives the real order executor over a real plugin storage journal,
//! the real regional entity owner and a real world storage kernel: nothing here
//! asserts plumbing. The scenarios are the contract's A08–A12 plus the
//! demobilisation rule.

use std::collections::BTreeMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mc_data::blocks::solaris_required_blocks_report;
use mc_data::item_components::solaris_required_item_facts;
use mc_data::items::solaris_required_items;
use mc_data::{Identifier, ItemStack};
use mc_entity::{SpawnEntity, Vec3};
use mc_protocol::codec;
use mc_script::{
    COMPONENT_PLUGIN_API_VERSION, MAX_RESIDENT_CARRY_SLOTS, MAX_RESIDENT_EQUIPMENT_SLOTS,
    ScriptAxisAlignedZone, ScriptBlockPosition, ScriptEngagementPolicy, ScriptEventKind,
    ScriptFormation, ScriptFormationKind, ScriptHostileCategory, ScriptInventoryEndpoint,
    ScriptOperation, ScriptOperationFailure, ScriptOperationOutcome, ScriptOperationPayload,
    ScriptOperationRequest, ScriptOrderMemberState, ScriptOrderTargetRef,
    ScriptOwnedInventoryOperation, ScriptPluginManifest, ScriptPosition, ScriptResidentOrder,
    ScriptResidentOrderOperation, ScriptResidentOrderResult, ScriptResidentWorkOrder,
    ScriptWorkArea, ScriptWorkPauseReason, ScriptWorkState, resident_generation_id,
    script_boundary_pair,
};
use mc_world::{BlockPos, BlockStateId, Chunk, ChunkPos, WorldStorage};
use uuid::Uuid;

use crate::play::SessionRegistry;
use crate::play::resident_work::{LiveResidentWorld, ResidentWorld, ResidentWorldEdit};
use crate::script::storage::resident_orders::{DurableResidentOrderChange, DurableResidentStack};
use crate::script::storage::world_inventory::InventoryRuntime;
use crate::script::storage::{PluginStorage, PluginStorageHandle, StorageFaultPoint};
use crate::server::{ScriptEventSink, ShutdownHandle, WorldHandle};

const OWNER: &str = "settlement";
const WORLD_IDENTITY: &str = "resident-order-world";
/// Flat terrain surface of the test world.
const SURFACE_Y: i32 = 63;

/// Test adapter that replaces the source block after loot preview but before
/// the simulation consumes the preview's precondition.
struct ReplacingResidentWorld {
    inner: Arc<LiveResidentWorld>,
    world: WorldHandle,
    replacement: BlockStateId,
    only_placements: bool,
}

impl ResidentWorld for ReplacingResidentWorld {
    fn dimension_loaded(&self, dimension: &str) -> bool {
        self.inner.dimension_loaded(dimension)
    }

    fn block(
        &self,
        dimension: &str,
        pos: [i32; 3],
    ) -> Option<crate::play::resident_work::ResidentBlock> {
        self.inner.block(dimension, pos)
    }

    fn standable(&self, dimension: &str, pos: [i32; 3]) -> Option<bool> {
        self.inner.standable(dimension, pos)
    }

    fn route_open(&self, dimension: &str, from: [i32; 3], to: [i32; 3]) -> Option<bool> {
        self.inner.route_open(dimension, from, to)
    }

    fn line_of_sight(&self, dimension: &str, from: Vec3, to: Vec3) -> Option<bool> {
        self.inner.line_of_sight(dimension, from, to)
    }

    fn foreign_zone_overlaps(
        &self,
        plugin_id: &str,
        dimension: &str,
        min: [i32; 3],
        max: [i32; 3],
    ) -> bool {
        self.inner
            .foreign_zone_overlaps(plugin_id, dimension, min, max)
    }

    fn state_for(&self, block_path: &str) -> Option<u32> {
        self.inner.state_for(block_path)
    }

    fn preview_break(
        &self,
        dimension: &str,
        pos: [i32; 3],
        expected_state: u32,
        tool: Option<&str>,
    ) -> Result<ResidentWorldEdit, ScriptOperationFailure> {
        self.inner
            .preview_break(dimension, pos, expected_state, tool)
    }

    fn commit_world_edits<'a>(
        &'a self,
        plugin_id: &'a str,
        dimension: &'a str,
        breaks: &'a [ResidentWorldEdit],
        receipt: Vec<u8>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<u64, ScriptOperationFailure>> + Send + 'a>>
    {
        let inner = Arc::clone(&self.inner);
        let world = Arc::clone(&self.world);
        let replacement = self.replacement;
        let only_placements = self.only_placements;
        Box::pin(async move {
            let preview = breaks
                .first()
                .ok_or(ScriptOperationFailure::InvalidRequest)?;
            if !only_placements || preview.drops.is_empty() {
                world
                    .lock()
                    .await
                    .set_block_at(preview.precondition.pos, replacement)
                    .map_err(|_| ScriptOperationFailure::RuntimeUnavailable)?;
            }
            inner
                .commit_world_edits(plugin_id, dimension, breaks, receipt)
                .await
        })
    }

    fn preview_place(
        &self,
        dimension: &str,
        pos: [i32; 3],
        state: u32,
    ) -> Result<ResidentWorldEdit, ScriptOperationFailure> {
        self.inner.preview_place(dimension, pos, state)
    }
}

/// Test-only protection authority whose transition models an accepted zone
/// removal while every other live-world read remains real.
struct ProtectionGatedResidentWorld {
    inner: Arc<LiveResidentWorld>,
    protected: Arc<AtomicBool>,
}

impl ResidentWorld for ProtectionGatedResidentWorld {
    fn dimension_loaded(&self, dimension: &str) -> bool {
        self.inner.dimension_loaded(dimension)
    }

    fn block(
        &self,
        dimension: &str,
        pos: [i32; 3],
    ) -> Option<crate::play::resident_work::ResidentBlock> {
        self.inner.block(dimension, pos)
    }

    fn standable(&self, dimension: &str, pos: [i32; 3]) -> Option<bool> {
        self.inner.standable(dimension, pos)
    }

    fn route_open(&self, dimension: &str, from: [i32; 3], to: [i32; 3]) -> Option<bool> {
        self.inner.route_open(dimension, from, to)
    }

    fn line_of_sight(&self, dimension: &str, from: Vec3, to: Vec3) -> Option<bool> {
        self.inner.line_of_sight(dimension, from, to)
    }

    fn foreign_zone_overlaps(
        &self,
        _plugin_id: &str,
        _dimension: &str,
        _min: [i32; 3],
        _max: [i32; 3],
    ) -> bool {
        self.protected.load(Ordering::Acquire)
    }

    fn state_for(&self, block_path: &str) -> Option<u32> {
        self.inner.state_for(block_path)
    }

    fn preview_break(
        &self,
        dimension: &str,
        pos: [i32; 3],
        expected_state: u32,
        tool: Option<&str>,
    ) -> Result<ResidentWorldEdit, ScriptOperationFailure> {
        self.inner
            .preview_break(dimension, pos, expected_state, tool)
    }

    fn commit_world_edits<'a>(
        &'a self,
        plugin_id: &'a str,
        dimension: &'a str,
        edits: &'a [ResidentWorldEdit],
        receipt: Vec<u8>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<u64, ScriptOperationFailure>> + Send + 'a>>
    {
        self.inner
            .commit_world_edits(plugin_id, dimension, edits, receipt)
    }

    fn preview_place(
        &self,
        dimension: &str,
        pos: [i32; 3],
        state: u32,
    ) -> Result<ResidentWorldEdit, ScriptOperationFailure> {
        self.inner.preview_place(dimension, pos, state)
    }
}

struct Fixture {
    storage_root: tempfile::TempDir,
    #[allow(dead_code)]
    world_root: tempfile::TempDir,
    blocks: Arc<mc_world::BlockRegistry>,
    world: WorldHandle,
    sessions: Arc<SessionRegistry>,
    runtime: InventoryRuntime,
}

impl Fixture {
    fn new(wall: bool) -> Self {
        Self::with_options(wall, None, false, false, None)
    }

    fn with_replacement(wall: bool, replacement: Option<&str>) -> Self {
        Self::with_options(wall, replacement, false, false, None)
    }

    fn with_replant_replacement(wall: bool, replacement: &str) -> Self {
        Self::with_options(wall, Some(replacement), false, true, None)
    }

    fn with_regional_edge_crop(wall: bool) -> Self {
        Self::with_options(wall, None, true, false, None)
    }

    fn with_protection_gate(protected: Arc<AtomicBool>) -> Self {
        Self::with_options(false, None, false, false, Some(protected))
    }

    fn with_options(
        wall: bool,
        replacement: Option<&str>,
        regional_edge_crop: bool,
        only_placements: bool,
        protected: Option<Arc<AtomicBool>>,
    ) -> Self {
        let blocks = Arc::new(
            mc_world::BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap(),
        );
        let replacement = replacement.map(|path| state_of(&blocks, path));
        let world_root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(
            world_root
                .path()
                .join("dimensions/minecraft/overworld/region"),
        )
        .unwrap();
        let world: WorldHandle = Arc::new(tokio::sync::Mutex::new(
            WorldStorage::open(world_root.path(), Arc::clone(&blocks)).unwrap(),
        ));
        {
            let mut storage = world.try_lock().expect("test world is free");
            let air = state_of(&blocks, "air");
            let biome = Identifier::parse("minecraft:plains").unwrap();
            let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, air, biome);
            let stone = state_of(&blocks, "stone");
            let grass = state_of(&blocks, "grass_block");
            let farmland = state_of(&blocks, "farmland");
            let wheat = state_with_props(&blocks, "wheat", &[("age", "7")]);
            let log = state_of(&blocks, "oak_log");
            let leaves = state_of(&blocks, "oak_leaves");
            let ore = state_of(&blocks, "iron_ore");
            let water = state_of(&blocks, "water");
            let crafting_table = state_of(&blocks, "crafting_table");
            for x in 0..16_u8 {
                for z in 0..16_u8 {
                    for y in SURFACE_Y - 5..SURFACE_Y {
                        chunk.set_block(x, y, z, stone);
                    }
                    chunk.set_block(x, SURFACE_Y, z, grass);
                }
            }
            // A ripe crop on farmland.
            chunk.set_block(2, SURFACE_Y, 2, farmland);
            chunk.set_block(2, SURFACE_Y + 1, 2, wheat);
            if regional_edge_crop {
                chunk.set_block(0, SURFACE_Y, 2, farmland);
                chunk.set_block(0, SURFACE_Y + 1, 2, wheat);
            }
            // A bounded natural tree: rooted trunk plus a small leaf canopy.
            chunk.set_block(6, SURFACE_Y + 1, 6, log);
            chunk.set_block(6, SURFACE_Y + 2, 6, log);
            for x in 5..=7 {
                for z in 5..=7 {
                    chunk.set_block(x, SURFACE_Y + 3, z, leaves);
                }
            }
            // Ore inside the stone band.
            chunk.set_block(4, SURFACE_Y - 2, 4, ore);
            // A single water column.
            chunk.set_block(8, SURFACE_Y, 8, water);
            // A loaded workstation for the craft-work contract.
            chunk.set_block(9, SURFACE_Y + 1, 9, crafting_table);
            if wall {
                // A closed passage: solid blocks where the squad must walk.
                let wall_block = state_of(&blocks, "stone");
                for z in 0..16_u8 {
                    chunk.set_block(10, SURFACE_Y + 1, z, wall_block);
                    chunk.set_block(10, SURFACE_Y + 2, z, wall_block);
                }
            }
            storage
                .insert_generated_chunk(ChunkPos { x: 0, z: 0 }, chunk)
                .unwrap();
        }
        let (read, mutation) = {
            let storage = world.try_lock().expect("test world is free");
            (storage.read_view(), storage.mutation_view())
        };
        let sessions = Arc::new(SessionRegistry::new());
        let items = Arc::new(solaris_required_items());
        let (journal, pending) = crate::play::world_journal::WorldChunkJournal::open_for_test(
            world_root.path(),
            Arc::clone(&blocks),
            Arc::clone(&items),
        )
        .unwrap();
        assert!(pending.is_empty());
        let first_decision = journal.reserve_decision_ids(1).unwrap()[0];
        journal
            .record_reserved_snapshot_groups(1, vec![(first_decision, Vec::new())])
            .unwrap();
        sessions.install_world_chunk_journal(journal);
        let (simulation, mut owner) = crate::play::simulation_channel();
        let _driver = {
            let sessions = Arc::clone(&sessions);
            let world = Arc::clone(&world);
            let simulation_read = read.clone();
            let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
            tokio::spawn(async move {
                while owner.wait_for_command().await {
                    owner
                        .process_commands_with_world_views(
                            &sessions,
                            Some(&world),
                            crate::play::SimulationWorldAccess {
                                read: Some(&simulation_read),
                                mutation: Some(&mutation),
                                cpu: Some(&resources),
                                light: None,
                            },
                            None,
                            1,
                        )
                        .await;
                }
            })
        };
        let item_facts = Arc::new(solaris_required_item_facts());
        let adapter = Arc::new(LiveResidentWorld::new(
            read,
            Arc::clone(&blocks),
            None,
            simulation,
            Arc::clone(&items),
            Arc::clone(&item_facts),
        ));
        let resident_world: Arc<dyn ResidentWorld> = match (replacement, protected) {
            (Some(replacement), _) => Arc::new(ReplacingResidentWorld {
                inner: Arc::clone(&adapter),
                world: Arc::clone(&world),
                replacement,
                only_placements,
            }),
            (None, Some(protected)) => Arc::new(ProtectionGatedResidentWorld {
                inner: adapter,
                protected,
            }),
            (None, None) => adapter,
        };
        let runtime = InventoryRuntime::new(
            Some(world_root.path()),
            &crate::server::ShutdownHandle::default(),
            Arc::clone(&sessions),
            items,
            item_facts,
        )
        .with_resident_world(resident_world);
        Self {
            storage_root: tempfile::tempdir().unwrap(),
            world_root,
            blocks,
            world,
            sessions,
            runtime,
        }
    }

    fn storage(&self) -> PluginStorage {
        PluginStorage::open(self.storage_root.path()).unwrap()
    }

    /// Materialise one resident through C3's real identity path and return its
    /// durable handle plus entity identity.
    async fn resident(
        &self,
        storage: &mut PluginStorage,
        slot: u32,
        position: Vec3,
    ) -> (String, Uuid) {
        let generation =
            resident_generation_id(WORLD_IDENTITY, "site-7", slot).expect("generation id");
        let snapshot = self
            .runtime
            .materialize_resident(storage, OWNER, &generation, position)
            .await
            .expect("resident materialises");
        let handle = mc_script::resident_handle_for_generation(OWNER, &generation).expect("handle");
        assert_eq!(handle, snapshot.handle);
        (handle, Uuid::parse_str(&snapshot.entity_uuid).unwrap())
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

    async fn block(&self, x: i32, y: i32, z: i32) -> Option<BlockStateId> {
        let mut storage = self.world.lock().await;
        storage.get_block(BlockPos { x, y, z }).unwrap()
    }

    /// Seed enough real farmland cells to force a receipt-bearing replant
    /// operation to stop at the simulation's conditional-edit boundary.
    async fn prepare_dense_replant_field(&self) {
        let farmland = state_of(&self.blocks, "farmland");
        let air = state_of(&self.blocks, "air");
        let mut storage = self.world.lock().await;
        for y in [40, 42, 44] {
            for x in 0..16 {
                for z in 0..16 {
                    storage
                        .set_block_at(BlockPos { x, y, z }, farmland)
                        .expect("dense farmland stays in the loaded fixture chunk");
                    storage
                        .set_block_at(BlockPos { x, y: y + 1, z }, air)
                        .expect("dense crop target stays in the loaded fixture chunk");
                }
            }
        }
    }
    async fn goal(&self, uuid: Uuid) -> mc_entity::GoalState {
        self.sessions
            .resident_entity_snapshots(&[uuid])
            .await
            .into_iter()
            .next()
            .flatten()
            .expect("resident stays live")
            .goal
    }

    async fn position(&self, uuid: Uuid) -> Vec3 {
        self.sessions
            .resident_entity_snapshots(&[uuid])
            .await
            .into_iter()
            .next()
            .flatten()
            .expect("resident stays live")
            .position
    }

    async fn health(&self, uuid: Uuid) -> f32 {
        self.sessions
            .resident_entity_snapshots(&[uuid])
            .await
            .into_iter()
            .next()
            .flatten()
            .expect("target stays live")
            .health
    }
}

fn state_of(blocks: &mc_world::BlockRegistry, path: &str) -> BlockStateId {
    let name = codec::Identifier::parse(format!("minecraft:{path}")).unwrap();
    blocks.block(&name).expect("required block").default
}

fn state_with_props(
    blocks: &mc_world::BlockRegistry,
    path: &str,
    props: &[(&str, &str)],
) -> BlockStateId {
    let name = codec::Identifier::parse(format!("minecraft:{path}")).unwrap();
    let props = props
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect::<Vec<_>>();
    blocks
        .by_name_and_props(&name, &props)
        .unwrap_or_else(|| state_of(blocks, path))
}

/// Seed canonical gear through the durable order ledger, the same store the
/// executor reads and writes.
fn seed_gear(
    storage: &mut PluginStorage,
    handle: &str,
    entity_uuid: Uuid,
    carry: &[(&str, u32)],
) -> u64 {
    seed_gear_in(storage, handle, entity_uuid, carry, false)
}

fn seed_gear_in(
    storage: &mut PluginStorage,
    handle: &str,
    entity_uuid: Uuid,
    carry: &[(&str, u32)],
    equipment: bool,
) -> u64 {
    // Gear is one field of the resident's durable order record, so seeding it
    // merges into the record the resident already has: acquiring a bow must not
    // silently drop the standing order it is serving.
    let mut record = storage
        .resident_orders()
        .record(handle)
        .cloned()
        .unwrap_or_else(|| {
            crate::script::storage::resident_orders::DurableResidentOrderRecord::empty(
                handle.to_owned(),
                OWNER.to_owned(),
                entity_uuid.to_string(),
                crate::script::storage::resident_orders::DurableAssignment::Civilian,
            )
        });
    for (index, (item, count)) in carry.iter().enumerate() {
        let stack = Some(DurableResidentStack::new((*item).to_owned(), *count));
        if equipment {
            record.equipment[index] = stack;
        } else {
            record.carry[index] = stack;
        }
    }
    storage
        .append_resident_order_change(DurableResidentOrderChange::Record {
            record: Box::new(record),
        })
        .expect("seeded gear is durable")
}

/// Equip the seeded bow through the durable order ledger. A record write
/// re-stamps the nested order revision, so the write's own revision is the
/// fence the next order must send.
fn equip_bow(storage: &mut PluginStorage, handle: &str) -> u64 {
    let mut record = storage
        .resident_orders()
        .record(handle)
        .cloned()
        .expect("resident record");
    let held = record.carry[0].take().expect("bow stack");
    record.equipment[0] = Some(held);
    storage
        .append_resident_order_change(DurableResidentOrderChange::Record {
            record: Box::new(record),
        })
        .unwrap();
    storage
        .resident_orders()
        .record(handle)
        .expect("resident record")
        .order_revision()
}

fn arrow_count(storage: &PluginStorage, handle: &str) -> u32 {
    gear(storage, handle)
        .iter()
        .filter(|(item, _)| item == "minecraft:arrow")
        .map(|(_, count)| *count)
        .sum()
}

fn gear(storage: &PluginStorage, handle: &str) -> Vec<(String, u32)> {
    storage
        .resident_orders()
        .record(handle)
        .map(|record| {
            record
                .equipment
                .iter()
                .chain(&record.carry)
                .flatten()
                .map(|stack| (stack.item_id.clone(), stack.count))
                .collect()
        })
        .unwrap_or_default()
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

fn demobilize_request(
    operation_id: &str,
    handle: &str,
    expected_revision: u64,
) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::ResidentOrder {
            operation: ScriptResidentOrderOperation::Demobilize {
                operation_id: operation_id.to_owned(),
                handle: handle.to_owned(),
                expected_revision,
            },
        },
    )
    .expect("valid demobilise request")
}

fn area(min: [i32; 3], max: [i32; 3]) -> ScriptWorkArea {
    ScriptWorkArea::new(
        "minecraft:overworld".to_owned(),
        ScriptBlockPosition::new(min[0], min[1], min[2]),
        ScriptBlockPosition::new(max[0], max[1], max[2]),
    )
}

fn crafting_station() -> ScriptWorkArea {
    area([9, SURFACE_Y + 1, 9], [9, SURFACE_Y + 1, 9])
}

fn work_of(outcome: &ScriptOperationOutcome) -> mc_script::ScriptWorkAssignment {
    let ScriptOperationPayload::ResidentOrder { result } = outcome.payload() else {
        panic!(
            "expected a resident order payload, got {:?}",
            outcome.payload()
        );
    };
    match &**result {
        ScriptResidentOrderResult::Work { assignment } => (**assignment).clone(),
        other => panic!("expected a work result, got {other:?}"),
    }
}

fn order_of(
    outcome: &ScriptOperationOutcome,
) -> (
    u64,
    Vec<mc_script::ScriptOrderMemberOutcome>,
    Vec<mc_script::ScriptCombatEvent>,
) {
    let ScriptOperationPayload::ResidentOrder { result } = outcome.payload() else {
        panic!(
            "expected a resident order payload, got {:?}",
            outcome.payload()
        );
    };
    match &**result {
        ScriptResidentOrderResult::Order {
            order_revision,
            members,
            combat,
        } => (*order_revision, members.clone(), combat.clone()),
        other => panic!("expected an order result, got {other:?}"),
    }
}

fn refused_members(outcome: &ScriptOperationOutcome) -> Vec<mc_script::ScriptOrderMemberOutcome> {
    let ScriptOperationPayload::ResidentOrder { result } = outcome.payload() else {
        panic!("expected a refusal payload, got {:?}", outcome.payload());
    };
    match &**result {
        ScriptResidentOrderResult::Order { members, .. } => members.clone(),
        other => panic!("expected an order refusal, got {other:?}"),
    }
}

fn formation() -> ScriptFormation {
    ScriptFormation::new(ScriptFormationKind::Line, 4)
}

/// (A08) A job without its tool commits nothing and reports the typed reason;
/// with the tool it breaks the real block and reports only committed drops.
#[tokio::test]
async fn harvest_requires_its_tool_and_commits_real_drops() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let harvest = ScriptResidentWorkOrder::Harvest {
        area: area([2, 64, 2], [2, 64, 2]),
        tool: "minecraft:iron_hoe".to_owned(),
    };

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request("harvest-1", &handle, harvest.clone(), 4, 0),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::MissingTool));
    assert_eq!(assignment.work_units_done, 0);
    assert!(assignment.changes.is_empty());
    assert_eq!(
        fixture.block(2, 64, 2).await,
        Some(state_with_props(&fixture.blocks, "wheat", &[("age", "7")])),
        "no tool means the crop is untouched"
    );

    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]);
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request("harvest-2", &handle, harvest, 4, revision),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, None, "{assignment:?}");
    assert!(assignment.work_units_done >= 1);
    assert!(
        assignment
            .changes
            .iter()
            .any(|change| change.item_id == "minecraft:wheat" && change.delta > 0),
        "the harvested crop is reported: {:?}",
        assignment.changes
    );
    assert_eq!(
        fixture.block(2, 64, 2).await,
        Some(state_of(&fixture.blocks, "air")),
        "the crop really left the world"
    );
    assert!(
        gear(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:iron_hoe" && *count == 1),
        "the tool stays in the canonical slot"
    );
}

/// (CP-013) A post-commit station event resumes the same paused assignment
/// exactly once; another region cannot wake it, and completion cannot loop.
#[tokio::test]
async fn world_event_resumes_the_same_paused_craft_once() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 17, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let air = state_of(&fixture.blocks, "air");
    let crafting_table = state_of(&fixture.blocks, "crafting_table");
    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos {
                x: 9,
                y: SURFACE_Y + 1,
                z: 9,
            },
            air,
        )
        .expect("fixture station stays loaded");
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:oak_log", 1)]);
    let paused = fixture
        .execute(
            &mut storage,
            &work_request(
                "paused-craft",
                &handle,
                ScriptResidentWorkOrder::Craft {
                    recipe: "minecraft:oak_planks".to_owned(),
                    count: 1,
                    station: crafting_station(),
                },
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(
        work_of(&paused).reason,
        Some(ScriptWorkPauseReason::MissingStation)
    );
    drop(storage);
    let mut storage = fixture.storage();
    assert_eq!(
        storage
            .resident_orders()
            .record(&handle)
            .and_then(|record| record.work.as_ref())
            .map(|work| work.state),
        Some(ScriptWorkState::Paused),
        "the reopened Store retains the exact paused job"
    );

    let unrelated = fixture
        .runtime
        .resume_paused_work_for_chunks(&mut storage, "minecraft:overworld", &[[8, 8]], |_| true)
        .await
        .expect("unrelated world event is bounded");
    assert!(unrelated.is_empty());
    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos {
                x: 9,

                y: SURFACE_Y + 1,
                z: 9,
            },
            crafting_table,
        )
        .expect("fixture station restoration commits");

    let inactive_owner = fixture
        .runtime
        .resume_paused_work_for_chunks(&mut storage, "minecraft:overworld", &[[0, 0]], |_| false)
        .await
        .expect("inactive owner leaves durable work paused");
    assert!(inactive_owner.is_empty());
    assert_eq!(
        storage
            .resident_orders()
            .record(&handle)
            .and_then(|record| record.work.as_ref())
            .map(|work| work.state),
        Some(ScriptWorkState::Paused)
    );

    let resumed = fixture
        .runtime
        .resume_paused_work_for_chunks(&mut storage, "minecraft:overworld", &[[0, 0]], |_| true)
        .await
        .expect("matching world event resumes work");
    assert_eq!(resumed.len(), 1);
    let assignment = work_of(&resumed[0].outcome);
    assert_eq!(assignment.state, ScriptWorkState::Committed);
    assert_eq!(assignment.reason, None);
    assert_eq!(assignment.work_units_done, 1);
    assert!(
        carry(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:oak_planks" && *count == 4)
    );

    let repeated = fixture
        .runtime
        .resume_paused_work_for_chunks(&mut storage, "minecraft:overworld", &[[0, 0]], |_| true)
        .await
        .expect("repeated event does not reschedule committed work");
    assert!(repeated.is_empty());
    assert_eq!(
        storage
            .resident_orders()
            .record(&handle)
            .and_then(|record| record.work.as_ref())
            .map(|work| work.done),
        Some(1)
    );
}

/// (CP-013) A work target absent from the resident world stays unloaded until
/// the matching chunk is published again; the reload resumes that same record.
#[tokio::test]
async fn chunk_load_event_resumes_the_same_unloaded_craft_once() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 21, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let station = area([24, SURFACE_Y + 1, 8], [24, SURFACE_Y + 1, 8]);
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:oak_log", 1)]);
    let paused = fixture
        .execute(
            &mut storage,
            &work_request(
                "pause-unloaded-craft",
                &handle,
                ScriptResidentWorkOrder::Craft {
                    recipe: "minecraft:oak_planks".to_owned(),
                    count: 1,
                    station,
                },
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(
        work_of(&paused).reason,
        Some(ScriptWorkPauseReason::Unloaded)
    );

    let air = state_of(&fixture.blocks, "air");
    let crafting_table = state_of(&fixture.blocks, "crafting_table");
    let mut loaded = Chunk::empty(
        ChunkPos { x: 1, z: 0 },
        air,
        Identifier::parse("minecraft:plains").expect("valid fixture biome"),
    );
    loaded.set_block(8, SURFACE_Y + 1, 8, crafting_table);
    fixture
        .world
        .lock()
        .await
        .commit_chunk_snapshot(ChunkPos { x: 1, z: 0 }, loaded)
        .expect("fixture publishes the reloaded chunk");

    let resumed = fixture
        .runtime
        .resume_paused_work_for_chunks(&mut storage, "minecraft:overworld", &[[1, 0]], |_| true)
        .await
        .expect("matching chunk load resumes craft");
    assert_eq!(resumed.len(), 1);
    assert_eq!(
        work_of(&resumed[0].outcome).state,
        ScriptWorkState::Committed
    );
    assert!(
        fixture
            .runtime
            .resume_paused_work_for_chunks(
                &mut storage,
                "minecraft:overworld",
                &[[1, 0]],
                |_| true,
            )
            .await
            .expect("completed work cannot resume twice")
            .is_empty()
    );
}

/// (CP-013) A resident inventory event wakes only that resident's blocked
/// prerequisite and preserves the original assignment watermark.
#[tokio::test]
async fn inventory_event_resumes_the_same_paused_harvest_once() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 18, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let harvest = ScriptResidentWorkOrder::Harvest {
        area: area([2, 64, 2], [2, 64, 2]),
        tool: "minecraft:iron_hoe".to_owned(),
    };
    let paused = fixture
        .execute(
            &mut storage,
            &work_request("paused-harvest", &handle, harvest, 1, 0),
        )
        .await;
    assert_eq!(
        work_of(&paused).reason,
        Some(ScriptWorkPauseReason::MissingTool)
    );

    seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]);
    let resumed = fixture
        .runtime
        .resume_paused_work_for_inventory_change(
            &mut storage,
            std::slice::from_ref(&handle),
            &[],
            |_| true,
        )
        .await
        .expect("resident inventory event resumes work");
    assert_eq!(resumed.len(), 1);
    assert_eq!(
        work_of(&resumed[0].outcome).state,
        ScriptWorkState::Committed
    );
    assert_eq!(
        fixture.block(2, 64, 2).await,
        Some(state_of(&fixture.blocks, "air"))
    );
    assert!(
        fixture
            .runtime
            .resume_paused_work_for_inventory_change(
                &mut storage,
                std::slice::from_ref(&handle),
                &[],
                |_| true,
            )
            .await
            .expect("completed work cannot loop")
            .is_empty()
    );
}
/// (CP-013) A changed protection zone selects only the paused work region it
/// overlaps. Removing that protection resumes the original job once.
#[tokio::test]
async fn zone_change_resumes_the_same_protected_harvest_once() {
    let protected = Arc::new(AtomicBool::new(true));
    let fixture = Fixture::with_protection_gate(Arc::clone(&protected));
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 22, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]);
    let paused = fixture
        .execute(
            &mut storage,
            &work_request(
                "pause-protected-harvest",
                &handle,
                ScriptResidentWorkOrder::Harvest {
                    area: area([2, 64, 2], [2, 64, 2]),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(
        work_of(&paused).reason,
        Some(ScriptWorkPauseReason::Protected)
    );
    let released_zone = ScriptAxisAlignedZone::try_new(
        "released-field",
        "minecraft:overworld",
        ScriptPosition::try_new(1.5, 63.0, 1.5).expect("valid zone minimum"),
        ScriptPosition::try_new(2.5, 65.0, 2.5).expect("valid zone maximum"),
    )
    .expect("valid released zone");
    let other_zone = ScriptAxisAlignedZone::try_new(
        "other-field",
        "minecraft:overworld",
        ScriptPosition::try_new(128.0, 63.0, 128.0).expect("valid zone minimum"),
        ScriptPosition::try_new(129.0, 65.0, 129.0).expect("valid zone maximum"),
    )
    .expect("valid unrelated zone");
    assert!(
        storage
            .resident_orders()
            .paused_records_for_zone(&other_zone)
            .is_empty(),
        "a non-overlapping zone cannot wake protected work"
    );

    protected.store(false, Ordering::Release);
    let resumed = fixture
        .runtime
        .resume_paused_work_for_zone(&mut storage, &released_zone, |_| true)
        .await
        .expect("removed protection resumes work");
    assert_eq!(resumed.len(), 1);
    assert_eq!(
        work_of(&resumed[0].outcome).state,
        ScriptWorkState::Committed
    );
    assert_eq!(
        fixture.block(2, 64, 2).await,
        Some(state_of(&fixture.blocks, "air"))
    );
    assert!(
        fixture
            .runtime
            .resume_paused_work_for_zone(&mut storage, &released_zone, |_| true)
            .await
            .expect("completed protected work cannot loop")
            .is_empty()
    );
}

/// (CP-013) The storage actor sends the real native resumption receipt only
/// after its queued world event observes the restored station.
#[tokio::test]
async fn storage_actor_delivers_one_native_resume_receipt() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 19, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let air = state_of(&fixture.blocks, "air");
    let crafting_table = state_of(&fixture.blocks, "crafting_table");
    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos {
                x: 9,
                y: SURFACE_Y + 1,
                z: 9,
            },
            air,
        )
        .expect("fixture station stays loaded");
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:oak_log", 1)]);
    let paused = fixture
        .execute(
            &mut storage,
            &work_request(
                "actor-paused-craft",
                &handle,
                ScriptResidentWorkOrder::Craft {
                    recipe: "minecraft:oak_planks".to_owned(),
                    count: 1,
                    station: crafting_station(),
                },
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(
        work_of(&paused).reason,
        Some(ScriptWorkPauseReason::MissingStation)
    );
    let receipt = storage
        .operation_receipt(OWNER, "actor-paused-craft")
        .cloned()
        .expect("paused assignment receipt is durable");
    storage
        .acknowledge_operation(&receipt)
        .expect("fixture consumes the original receipt before actor startup");
    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos {
                x: 9,
                y: SURFACE_Y + 1,
                z: 9,
            },
            crafting_table,
        )
        .expect("fixture station restoration commits");

    let (boundary, mut endpoint) = script_boundary_pair(
        NonZeroUsize::new(8).expect("event capacity"),
        NonZeroUsize::new(8).expect("command capacity"),
    );
    let manifest = ScriptPluginManifest::new(OWNER, OWNER, "0.1.0", COMPONENT_PLUGIN_API_VERSION)
        .validate()
        .expect("test owner manifest is valid");
    endpoint
        .register_plugin_routes(&manifest)
        .expect("test owner registration is live");
    let actor = PluginStorageHandle::start(
        storage,
        fixture.runtime.clone(),
        ScriptEventSink::new(boundary),
        ShutdownHandle::default(),
    );
    actor
        .wake_resident_work("minecraft:overworld", vec![[0, 0]])
        .await;
    let event = match tokio::time::timeout(Duration::from_secs(1), endpoint.recv_event()).await {
        Ok(Some(event)) => event,
        Ok(None) => panic!("storage actor closed before native resume delivery"),
        Err(_) => panic!(
            "native resume event is not delivered; storage actor failed: {}",
            actor.failed()
        ),
    };
    let ScriptEventKind::OperationResult {
        operation_id,
        outcome,
        ..
    } = event.kind()
    else {
        panic!(
            "expected native operation result, got {}",
            event.event_name()
        );
    };
    assert!(
        operation_id
            .as_deref()
            .is_some_and(|id| id.starts_with("native-resume-"))
    );
    assert_eq!(work_of(outcome).state, ScriptWorkState::Committed);
    assert_eq!(
        fixture.block(9, SURFACE_Y + 1, 9).await,
        Some(crafting_table)
    );
    drop(actor);
}

/// (R1) The harvested crop ends up in the worker's own cargo, and the receipt
/// states exactly what it holds: a receipt delta with no owner was the defect.
#[tokio::test]
async fn harvest_deposits_the_real_crop_into_the_worker_cargo() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "harvest-cargo",
                &handle,
                ScriptResidentWorkOrder::Harvest {
                    area: area([2, 64, 2], [2, 64, 2]),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                4,
                revision,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), None, "{outcome:?}");
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, None, "{assignment:?}");
    assert!(assignment.work_units_done >= 1);
    assert_eq!(
        fixture.block(2, 64, 2).await,
        Some(state_of(&fixture.blocks, "air")),
        "the crop really left the world"
    );

    let carried: Vec<(String, u32)> = carry(&storage, &handle);
    let wheat = carried
        .iter()
        .find(|(item, _)| item == "minecraft:wheat")
        .map(|(_, count)| *count)
        .unwrap_or(0);
    assert!(
        wheat >= 1,
        "the worker owns the crop it harvested: {carried:?}"
    );
    for change in assignment.changes.iter().filter(|change| change.delta > 0) {
        let held = carried
            .iter()
            .filter(|(item, _)| *item == change.item_id)
            .map(|(_, count)| i64::from(*count))
            .sum::<i64>();
        assert!(
            held >= change.delta,
            "the receipt claims {} of {} but the cargo holds {held}",
            change.delta,
            change.item_id
        );
    }

    // The observable read path agrees with the record: a plugin asks for the
    // resident's carry and gets these stacks.
    let snapshot = fixture
        .runtime
        .execute_owned_inventory(&mut storage, OWNER, &carry_request(&handle, None))
        .await
        .expect("the carry read reaches the durable boundary");
    assert_eq!(snapshot.failure(), None, "{snapshot:?}");
    let counted = carried_from(&snapshot);
    assert!(
        counted
            .iter()
            .any(|(item, count)| item == "minecraft:wheat" && *count == wheat),
        "the carry snapshot shows the harvested crop: {counted:?}"
    );
}

/// (CP-007) A receipt-bearing resident break at a regional edge still uses the
/// journaled regional decision instead of falling through to a direct refusal.
#[tokio::test]
async fn harvest_at_regional_edge_commits_the_crop_and_cargo_together() {
    let fixture = Fixture::with_regional_edge_crop(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(0.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]);
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "harvest-regional-edge",
                &handle,
                ScriptResidentWorkOrder::Harvest {
                    area: area([0, 64, 2], [0, 64, 2]),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(
        fixture.block(0, 64, 2).await,
        Some(state_of(&fixture.blocks, "air"))
    );
    assert!(
        carry(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:wheat" && *count >= 1),
        "the edge decision records its crop in worker cargo"
    );
}

/// (CP-007) A storage projection failure after the world decision never loses
/// the previewed crop: reopening projects the exact journal receipt once.
#[tokio::test]
async fn harvest_recovers_cargo_and_progress_after_storage_projection_fails() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]);
    let request = work_request(
        "harvest-projection-recovery",
        &handle,
        ScriptResidentWorkOrder::Harvest {
            area: area([2, 64, 2], [2, 64, 2]),
            tool: "minecraft:iron_hoe".to_owned(),
        },
        1,
        revision,
    );

    storage.inject_fault_for_test(StorageFaultPoint::Write);
    assert!(matches!(
        fixture
            .runtime
            .execute_resident_order_operation(&mut storage, OWNER, &request)
            .await,
        Err(super::PluginStorageMutationError::DurabilityUnknown(_))
    ));
    assert_eq!(
        fixture.block(2, 64, 2).await,
        Some(state_of(&fixture.blocks, "air")),
        "the accepted world decision keeps the broken crop"
    );

    drop(storage);
    let mut reopened = fixture.storage();
    fixture
        .runtime
        .recover(&mut reopened)
        .expect("the accepted world receipt projects at recovery");
    let recovered = fixture.execute(&mut reopened, &request).await;
    let assignment = work_of(&recovered);
    assert_eq!(assignment.work_units_done, 1);
    let wheat = carry(&reopened, &handle)
        .iter()
        .filter(|(item, _)| item == "minecraft:wheat")
        .map(|(_, count)| *count)
        .sum::<u32>();
    assert_eq!(wheat, 1, "recovery must not duplicate the canonical crop");
}

/// (CP-007) Replacing a crop after preview consumes neither the replacement
/// block nor the previewed loot; the worker record keeps its old progress.
#[tokio::test]
async fn replaced_crop_after_preview_does_not_create_cargo_or_work_progress() {
    let fixture = Fixture::with_replacement(false, Some("stone"));
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]);
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "harvest-replaced-after-preview",
                &handle,
                ScriptResidentWorkOrder::Harvest {
                    area: area([2, 64, 2], [2, 64, 2]),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;

    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::StaleRevision)
    );
    assert_eq!(
        fixture.block(2, 64, 2).await,
        Some(state_of(&fixture.blocks, "stone")),
        "the replacement survives the stale conditional decision"
    );
    assert!(
        !carry(&storage, &handle)
            .iter()
            .any(|(item, _)| item == "minecraft:wheat"),
        "a stale preview cannot mint crop cargo"
    );
    let record = storage
        .resident_orders()
        .record(&handle)
        .expect("seeded resident record remains");
    assert_eq!(record.revision, revision);
    assert!(
        record.work.is_none(),
        "the stale call records no work progress"
    );
}

/// (CP-008) Harvesting and replanting use the real crop state and leave the
/// worker with the canonical produce and the exact remaining seed reserve.
#[tokio::test]
async fn harvest_then_replant_commits_crop_seed_and_work_together() {
    let fixture = Fixture::with_regional_edge_crop(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(0.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:iron_hoe", 1), ("minecraft:wheat_seeds", 2)],
    );
    let harvest = fixture
        .execute(
            &mut storage,
            &work_request(
                "harvest-before-replant",
                &handle,
                ScriptResidentWorkOrder::Harvest {
                    area: area([0, 64, 2], [0, 64, 2]),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(harvest.failure(), None, "{harvest:?}");
    assert_eq!(
        fixture.block(0, 64, 2).await,
        Some(state_of(&fixture.blocks, "air"))
    );

    let revision = storage
        .resident_orders()
        .record(&handle)
        .expect("harvest record")
        .revision;
    let seeds_before_replant = carry(&storage, &handle)
        .iter()
        .find(|(item, _)| item == "minecraft:wheat_seeds")
        .map(|(_, count)| *count)
        .expect("harvest retains the canonical seed drop");
    let replant = fixture
        .execute(
            &mut storage,
            &work_request(
                "replant-after-harvest",
                &handle,
                ScriptResidentWorkOrder::Replant {
                    area: area([0, SURFACE_Y, 2], [0, SURFACE_Y, 2]),
                    seed: "minecraft:wheat_seeds".to_owned(),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&replant);
    assert_eq!(replant.failure(), None, "{replant:?}");
    assert_eq!(assignment.state, ScriptWorkState::Committed);
    assert_eq!(assignment.work_units_done, 1);
    assert_eq!(
        fixture.block(0, 64, 2).await,
        Some(state_of(&fixture.blocks, "wheat")),
        "replant places the registered default crop state"
    );
    let carried = carry(&storage, &handle);
    assert_eq!(
        carried
            .iter()
            .find(|(item, _)| item == "minecraft:wheat_seeds")
            .map(|(_, count)| *count),
        Some(seeds_before_replant - 1),
        "exactly one seed left the worker's reserve"
    );
    assert!(
        carried
            .iter()
            .any(|(item, count)| item == "minecraft:wheat" && *count >= 1),
        "the first cycle's canonical harvest remains worker-owned"
    );
}

/// (CP-008) A field with no seed reserve advances neither the replant
/// assignment nor a speculative world decision.
#[tokio::test]
async fn replant_without_seed_leaves_the_field_unchanged() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]);
    let replant = fixture
        .execute(
            &mut storage,
            &work_request(
                "replant-without-seed",
                &handle,
                ScriptResidentWorkOrder::Replant {
                    area: area([2, SURFACE_Y, 2], [2, SURFACE_Y, 2]),
                    seed: "minecraft:wheat_seeds".to_owned(),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&replant);
    assert_eq!(replant.failure(), None, "{replant:?}");
    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::MissingInput));
    assert_eq!(assignment.work_units_done, 0);
    assert_eq!(
        fixture.block(2, 64, 2).await,
        Some(state_with_props(&fixture.blocks, "wheat", &[("age", "7")])),
        "a missing seed cannot change the field"
    );
}

/// (CP-008) A seed reserve alone cannot plant a crop: replant requires its
/// configured tool before either the field or inventory record can change.
#[tokio::test]
async fn replant_without_tool_leaves_the_field_and_seed_unchanged() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:wheat_seeds", 1)]);
    let replant = fixture
        .execute(
            &mut storage,
            &work_request(
                "replant-without-tool",
                &handle,
                ScriptResidentWorkOrder::Replant {
                    area: area([2, SURFACE_Y, 2], [2, SURFACE_Y, 2]),
                    seed: "minecraft:wheat_seeds".to_owned(),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&replant);
    assert_eq!(replant.failure(), None, "{replant:?}");
    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::MissingTool));
    assert_eq!(assignment.work_units_done, 0);
    assert_eq!(
        fixture.block(2, 64, 2).await,
        Some(state_with_props(&fixture.blocks, "wheat", &[("age", "7")])),
        "a missing tool cannot change the field"
    );
    assert_eq!(
        carry(&storage, &handle),
        vec![("minecraft:wheat_seeds".to_owned(), 1)],
        "a missing tool cannot spend the seed reserve"
    );
}

/// (CP-008) A player edit after placement preview consumes neither seed nor
/// replant progress; the changed field wins the conditional decision.
#[tokio::test]
async fn changed_field_after_replant_preview_keeps_seed_and_work_unchanged() {
    let fixture = Fixture::with_replant_replacement(false, "stone");
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:iron_hoe", 1), ("minecraft:wheat_seeds", 1)],
    );
    let harvest = fixture
        .execute(
            &mut storage,
            &work_request(
                "harvest-before-changed-replant",
                &handle,
                ScriptResidentWorkOrder::Harvest {
                    area: area([2, 64, 2], [2, 64, 2]),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(harvest.failure(), None, "{harvest:?}");
    let revision = storage
        .resident_orders()
        .record(&handle)
        .expect("harvest record")
        .revision;
    let seeds_before_replant = carry(&storage, &handle)
        .iter()
        .find(|(item, _)| item == "minecraft:wheat_seeds")
        .map(|(_, count)| *count)
        .expect("harvest retains the canonical seed drop");
    let wheat_before_replant = carry(&storage, &handle)
        .iter()
        .filter(|(item, _)| item == "minecraft:wheat")
        .map(|(_, count)| *count)
        .sum::<u32>();
    let replant = fixture
        .execute(
            &mut storage,
            &work_request(
                "replant-after-player-field-change",
                &handle,
                ScriptResidentWorkOrder::Replant {
                    area: area([2, SURFACE_Y, 2], [2, SURFACE_Y, 2]),
                    seed: "minecraft:wheat_seeds".to_owned(),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(
        replant.failure(),
        Some(ScriptOperationFailure::StaleRevision)
    );
    assert_eq!(
        fixture.block(2, 64, 2).await,
        Some(state_of(&fixture.blocks, "stone")),
        "the player-changed field survives the rejected placement"
    );
    assert_eq!(
        carry(&storage, &handle)
            .iter()
            .find(|(item, _)| item == "minecraft:wheat_seeds")
            .map(|(_, count)| *count),
        Some(seeds_before_replant),
        "a rejected placement preserves the durable seed reserve"
    );
    assert_eq!(
        carry(&storage, &handle)
            .iter()
            .filter(|(item, _)| item == "minecraft:wheat")
            .map(|(_, count)| *count)
            .sum::<u32>(),
        wheat_before_replant,
        "a rejected placement cannot duplicate harvested produce"
    );
    let record = storage
        .resident_orders()
        .record(&handle)
        .expect("harvest record remains");
    assert_eq!(record.revision, revision);
    assert!(
        matches!(
            record.work.as_deref(),
            Some(work) if matches!(&work.work, ScriptResidentWorkOrder::Harvest { .. })
        ),
        "the rejected replant cannot replace prior committed work progress"
    );
}

/// (CP-008) A replant receipt never exceeds the simulation's conditional-edit
/// cap: the durable work watermark resumes the one remaining real field cell.
#[tokio::test]
async fn replant_splits_at_the_simulation_edit_limit() {
    let fixture = Fixture::new(false);
    fixture.prepare_dense_replant_field().await;
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(0.5, 64.0, 0.5))
        .await;
    let equipment = [
        ("minecraft:iron_hoe", 1),
        ("minecraft:wheat_seeds", 64),
        ("minecraft:wheat_seeds", 64),
        ("minecraft:wheat_seeds", 64),
        ("minecraft:wheat_seeds", 64),
        ("minecraft:wheat_seeds", 64),
    ];
    let carry = [
        ("minecraft:wheat_seeds", 64),
        ("minecraft:wheat_seeds", 64),
        ("minecraft:wheat_seeds", 64),
        ("minecraft:wheat_seeds", 64),
    ];
    let revision = seed_gear_in(&mut storage, &handle, uuid, &equipment, true);
    let revision = seed_gear_in(&mut storage, &handle, uuid, &carry, false).max(revision);
    let work = ScriptResidentWorkOrder::Replant {
        area: area([0, 40, 0], [15, 44, 15]),
        seed: "minecraft:wheat_seeds".to_owned(),
        tool: "minecraft:iron_hoe".to_owned(),
    };
    let first = fixture
        .execute(
            &mut storage,
            &work_request(
                "replant-edit-limit-first",
                &handle,
                work.clone(),
                513,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&first);
    assert_eq!(first.failure(), None, "{first:?}");
    assert_eq!(assignment.state, ScriptWorkState::Running);
    assert_eq!(assignment.reason, None);
    assert_eq!(assignment.work_units_done, 512);

    let revision = storage
        .resident_orders()
        .record(&handle)
        .expect("first portion record")
        .revision;
    let resumed = fixture
        .execute(
            &mut storage,
            &work_request("replant-edit-limit-resume", &handle, work, 513, revision),
        )
        .await;
    let assignment = work_of(&resumed);
    assert_eq!(resumed.failure(), None, "{resumed:?}");
    assert_eq!(assignment.state, ScriptWorkState::Committed);
    assert_eq!(assignment.reason, None);
    assert_eq!(assignment.work_units_done, 513);
}

/// (R1) A worker who cannot hold the loot leaves the world alone: no block is
/// broken to produce items nobody can own.
#[tokio::test]
async fn a_full_worker_reports_no_storage_and_leaves_the_crop_standing() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let fill: Vec<(&str, u32)> = (0..MAX_RESIDENT_CARRY_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    let mut revision = seed_gear_in(&mut storage, &handle, uuid, &fill, false);
    let equipment: Vec<(&str, u32)> = (0..MAX_RESIDENT_EQUIPMENT_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    revision = seed_gear_in(&mut storage, &handle, uuid, &equipment, true).max(revision);
    // The hoe takes the only slot a full worker would have had to spare.
    revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]).max(revision);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "harvest-full",
                &handle,
                ScriptResidentWorkOrder::Harvest {
                    area: area([2, 64, 2], [2, 64, 2]),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                4,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::NoStorage));
    assert_eq!(assignment.work_units_done, 0);
    assert!(
        assignment.changes.iter().all(|change| change.delta <= 0),
        "nothing was produced: {:?}",
        assignment.changes
    );
    assert_ne!(
        fixture.block(2, 64, 2).await,
        Some(state_of(&fixture.blocks, "air")),
        "the crop stays because the worker could not hold it"
    );
}

/// (R1) The same cargo path carries mined ore: one producer, one owner.
#[tokio::test]
async fn mined_ore_reaches_the_worker_cargo() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:iron_pickaxe", 1)],
    );

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "mine-cargo",
                &handle,
                ScriptResidentWorkOrder::Mine {
                    area: area([4, SURFACE_Y - 2, 4], [4, SURFACE_Y - 2, 4]),
                    tool: "minecraft:iron_pickaxe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, None, "{assignment:?}");
    assert_eq!(assignment.work_units_done, 1);
    let carried = carry(&storage, &handle);
    assert!(
        carried
            .iter()
            .any(|(item, count)| item == "minecraft:raw_iron" && *count >= 1),
        "the mined ore is the worker's: {carried:?} changes {:?}",
        assignment.changes
    );
    assert_eq!(
        fixture.block(4, SURFACE_Y - 2, 4).await,
        Some(state_of(&fixture.blocks, "air")),
        "the ore really left the world"
    );
}

/// (CP-010) Mining must not destroy an ore whose canonical loot cannot enter
/// the worker's actual inventory.
#[tokio::test]
async fn mine_with_full_cargo_leaves_ore_untouched() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 5.5))
        .await;
    let fill: Vec<(&str, u32)> = (0..MAX_RESIDENT_CARRY_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    let mut revision = seed_gear_in(&mut storage, &handle, uuid, &fill, false);
    let equipment: Vec<(&str, u32)> = (0..MAX_RESIDENT_EQUIPMENT_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    revision = seed_gear_in(&mut storage, &handle, uuid, &equipment, true).max(revision);
    revision = seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:iron_pickaxe", 1)],
    )
    .max(revision);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "mine-full",
                &handle,
                ScriptResidentWorkOrder::Mine {
                    area: area([4, SURFACE_Y - 2, 4], [4, SURFACE_Y - 2, 4]),
                    tool: "minecraft:iron_pickaxe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);

    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::NoStorage));
    assert_eq!(assignment.work_units_done, 0);
    assert_eq!(
        fixture.block(4, SURFACE_Y - 2, 4).await,
        Some(state_of(&fixture.blocks, "iron_ore")),
        "a full worker cannot convert an ore to unowned loot"
    );
}

/// (CP-010) An unsuitable tool cannot consume an ore block without producing
/// its canonical loot for the worker.
#[tokio::test]
async fn mine_with_unsuitable_tool_preserves_ore() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]);
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "mine-with-hoe",
                &handle,
                ScriptResidentWorkOrder::Mine {
                    area: area([4, SURFACE_Y - 2, 4], [4, SURFACE_Y - 2, 4]),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::MissingTool));
    assert_eq!(assignment.work_units_done, 0);
    assert_eq!(
        fixture.block(4, SURFACE_Y - 2, 4).await,
        Some(state_of(&fixture.blocks, "iron_ore")),
        "a non-dropping tool must leave the ore in the world"
    );
    assert!(
        !carry(&storage, &handle)
            .iter()
            .any(|(item, _)| item == "minecraft:raw_iron"),
        "a refused mine does not mint ore cargo"
    );
}

/// (CP-010) A tool-tier refusal after one accepted ore preserves the next ore
/// and exposes the reason without rolling back the accepted durable decision.
#[tokio::test]
async fn mine_pauses_for_a_later_ore_that_needs_a_better_tool() {
    let fixture = Fixture::new(false);
    {
        let mut world = fixture.world.lock().await;
        world
            .set_block_at(
                BlockPos {
                    x: 4,
                    y: SURFACE_Y - 2,
                    z: 4,
                },
                state_of(&fixture.blocks, "coal_ore"),
            )
            .expect("the first ore remains in the loaded fixture chunk");
        world
            .set_block_at(
                BlockPos {
                    x: 5,
                    y: SURFACE_Y - 2,
                    z: 4,
                },
                state_of(&fixture.blocks, "diamond_ore"),
            )
            .expect("the later ore remains in the loaded fixture chunk");
    }
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:stone_pickaxe", 1)],
    );
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "mine-coal-before-diamond",
                &handle,
                ScriptResidentWorkOrder::Mine {
                    area: area([4, SURFACE_Y - 2, 4], [5, SURFACE_Y - 2, 4]),
                    tool: "minecraft:stone_pickaxe".to_owned(),
                },
                2,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::MissingTool));
    assert_eq!(assignment.work_units_done, 1);
    assert_eq!(
        fixture.block(4, SURFACE_Y - 2, 4).await,
        Some(state_of(&fixture.blocks, "air")),
        "the canonical coal decision remains committed"
    );
    assert_eq!(
        fixture.block(5, SURFACE_Y - 2, 4).await,
        Some(state_of(&fixture.blocks, "diamond_ore")),
        "the tier-gated ore remains available for a suitable tool"
    );
    assert!(
        carry(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:coal" && *count >= 1),
        "only the accepted coal reaches the worker"
    );
}

/// (CP-007) Tree cutting uses the same receipt-bearing break boundary as crop
/// harvest and mining.
#[tokio::test]
async fn cut_tree_commits_the_log_and_worker_cargo_together() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(6.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_axe", 1)]);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "cut-cargo",
                &handle,
                ScriptResidentWorkOrder::CutTree {
                    area: area([6, SURFACE_Y + 1, 6], [6, SURFACE_Y + 2, 6]),
                    tool: "minecraft:iron_axe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, None, "{assignment:?}");
    assert_eq!(assignment.work_units_done, 1);
    assert!(
        carry(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:oak_log" && *count == 1),
        "the worker receives the one committed log"
    );
    assert_eq!(
        fixture.block(6, SURFACE_Y + 1, 6).await,
        Some(state_of(&fixture.blocks, "air")),
        "the matching world log was committed in the receipt decision"
    );
}

/// (CP-009) A grounded house column beside a real tree remains untouched.
#[tokio::test]
async fn cut_tree_keeps_adjacent_grounded_house_logs() {
    let fixture = Fixture::new(false);

    let house_log = state_of(&fixture.blocks, "oak_log");
    {
        let mut world = fixture.world.lock().await;
        for y in SURFACE_Y + 1..=SURFACE_Y + 2 {
            world
                .set_block_at(BlockPos { x: 9, y, z: 6 }, house_log)
                .expect("the house column stays in the loaded fixture chunk");
        }
    }
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(6.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_axe", 1)]);
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "cut-tree-not-house",
                &handle,
                ScriptResidentWorkOrder::CutTree {
                    area: area([6, SURFACE_Y + 1, 6], [9, SURFACE_Y + 2, 6]),
                    tool: "minecraft:iron_axe".to_owned(),
                },
                3,
                revision,
            ),
        )
        .await;

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(
        fixture.block(9, SURFACE_Y + 1, 6).await,
        Some(house_log),
        "a grounded log column without a canopy is a house, not a tree"
    );
    assert_eq!(
        fixture.block(9, SURFACE_Y + 2, 6).await,
        Some(house_log),
        "the worker must not progress into the adjacent house column"
    );
    assert_eq!(
        carry(&storage, &handle)
            .iter()
            .filter(|(item, _)| item == "minecraft:oak_log")
            .map(|(_, count)| *count)
            .sum::<u32>(),
        2,
        "only the two natural-tree trunk logs reach the worker"
    );
}
/// (CP-011) Fishing pays a durable owner only from water currently present in
/// the named work area; its deterministic cod result is not a vanilla-loot claim.
#[tokio::test]
async fn fish_uses_real_water_and_credits_resident_cargo() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(8.5, 64.0, 8.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:fishing_rod", 1)]);
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "fish-water-column",
                &handle,
                ScriptResidentWorkOrder::Fish {
                    area: area([8, SURFACE_Y, 8], [8, SURFACE_Y, 8]),
                    tool: "minecraft:fishing_rod".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(assignment.state, ScriptWorkState::Committed);
    assert_eq!(assignment.reason, None);
    assert_eq!(assignment.work_units_done, 1);
    assert!(
        carry(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:cod" && *count == 1),
        "the catch belongs to the worker"
    );
}

/// (CP-011) Tending spends feed only while a live, locally tracked animal is
/// available; it produces no virtual livestock goods.
#[tokio::test]
async fn tend_livestock_consumes_feed_for_a_visible_animal() {
    let fixture = Fixture::new(false);
    let mut cow = SpawnEntity::new(0, "minecraft:cow", Vec3::new(6.5, 64.0, 6.5));
    cow.animal = Some(mc_entity::AnimalBreedingState::adult());
    fixture
        .sessions
        .spawn_tracked_entity_for_test(cow, false)
        .expect("the cow joins the local session index");
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(6.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:wheat", 1)]);
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "tend-visible-cow",
                &handle,
                ScriptResidentWorkOrder::TendLivestock {
                    area: area([6, 64, 6], [6, 64, 6]),
                    feed: "minecraft:wheat".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(assignment.state, ScriptWorkState::Committed);
    assert_eq!(assignment.reason, None);
    assert_eq!(assignment.work_units_done, 1);
    assert!(
        !carry(&storage, &handle)
            .iter()
            .any(|(item, _)| item == "minecraft:wheat"),
        "the real tending action consumes resident-owned feed"
    );
    assert!(
        assignment.changes.iter().all(|change| change.delta <= 0),
        "tending does not mint livestock output: {:?}",
        assignment.changes
    );
}

/// (CP-011) A resident cannot spend wheat on a chicken or a baby cow: the
/// shared vanilla food tags and breeding state gate the durable cost.
#[tokio::test]
async fn tend_livestock_refuses_incompatible_or_immature_animals() {
    for (case, type_name, animal) in [
        (
            "incompatible-chicken",
            "minecraft:chicken",
            mc_entity::AnimalBreedingState::adult(),
        ),
        (
            "immature-cow",
            "minecraft:cow",
            mc_entity::AnimalBreedingState::baby(),
        ),
    ] {
        let fixture = Fixture::new(false);
        let mut entity = SpawnEntity::new(0, type_name, Vec3::new(6.5, 64.0, 6.5));
        entity.animal = Some(animal);
        fixture
            .sessions
            .spawn_tracked_entity_for_test(entity, false)
            .expect("the animal joins the local session index");
        let mut storage = fixture.storage();
        let (handle, uuid) = fixture
            .resident(&mut storage, 3, Vec3::new(6.5, 64.0, 5.5))
            .await;
        let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:wheat", 1)]);
        let operation_id = format!("tend-{case}");
        let outcome = fixture
            .execute(
                &mut storage,
                &work_request(
                    &operation_id,
                    &handle,
                    ScriptResidentWorkOrder::TendLivestock {
                        area: area([6, 64, 6], [6, 64, 6]),
                        feed: "minecraft:wheat".to_owned(),
                    },
                    1,
                    revision,
                ),
            )
            .await;
        let assignment = work_of(&outcome);

        assert_eq!(outcome.failure(), None, "{case}: {outcome:?}");
        assert_eq!(assignment.state, ScriptWorkState::Paused, "{case}");
        assert_eq!(
            assignment.reason,
            Some(ScriptWorkPauseReason::MissingInput),
            "{case}"
        );
        assert_eq!(assignment.work_units_done, 0, "{case}");
        assert!(
            carry(&storage, &handle)
                .iter()
                .any(|(item, count)| item == "minecraft:wheat" && *count == 1),
            "{case}: an ineligible animal cannot consume feed"
        );
    }
}

/// (CP-011) Water and livestock work both preserve their input when their
/// corresponding live source is absent.
#[tokio::test]
async fn fish_and_livestock_without_sources_preserve_inputs() {
    let fishing = Fixture::new(false);
    let mut fishing_storage = fishing.storage();
    let (fisher, fisher_uuid) = fishing
        .resident(&mut fishing_storage, 3, Vec3::new(7.5, 64.0, 7.5))
        .await;
    let fishing_revision = seed_gear(
        &mut fishing_storage,
        &fisher,
        fisher_uuid,
        &[("minecraft:fishing_rod", 1)],
    );
    let fishing_outcome = fishing
        .execute(
            &mut fishing_storage,
            &work_request(
                "fish-without-water",
                &fisher,
                ScriptResidentWorkOrder::Fish {
                    area: area([7, SURFACE_Y, 7], [7, SURFACE_Y, 7]),
                    tool: "minecraft:fishing_rod".to_owned(),
                },
                1,
                fishing_revision,
            ),
        )
        .await;
    let fishing_work = work_of(&fishing_outcome);
    assert_eq!(fishing_work.state, ScriptWorkState::Paused);
    assert_eq!(
        fishing_work.reason,
        Some(ScriptWorkPauseReason::MissingInput)
    );
    assert!(
        carry(&fishing_storage, &fisher)
            .iter()
            .any(|(item, count)| item == "minecraft:fishing_rod" && *count == 1),
        "no water leaves the rod untouched"
    );

    let livestock = Fixture::new(false);
    let mut livestock_storage = livestock.storage();
    let (tender, tender_uuid) = livestock
        .resident(&mut livestock_storage, 3, Vec3::new(6.5, 64.0, 5.5))
        .await;
    let livestock_revision = seed_gear(
        &mut livestock_storage,
        &tender,
        tender_uuid,
        &[("minecraft:wheat", 1)],
    );
    let livestock_outcome = livestock
        .execute(
            &mut livestock_storage,
            &work_request(
                "tend-without-livestock",
                &tender,
                ScriptResidentWorkOrder::TendLivestock {
                    area: area([6, 64, 6], [6, 64, 6]),
                    feed: "minecraft:wheat".to_owned(),
                },
                1,
                livestock_revision,
            ),
        )
        .await;
    let livestock_work = work_of(&livestock_outcome);
    assert_eq!(livestock_work.state, ScriptWorkState::Paused);
    assert_eq!(
        livestock_work.reason,
        Some(ScriptWorkPauseReason::MissingInput)
    );
    assert!(
        carry(&livestock_storage, &tender)
            .iter()
            .any(|(item, count)| item == "minecraft:wheat" && *count == 1),
        "no livestock leaves the feed with its owner"
    );
}

fn carry_request(handle: &str, expected_revision: Option<u64>) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::Query {
                endpoint: ScriptInventoryEndpoint::ResidentCarry {
                    handle: handle.to_owned(),
                },
                expected_revision,
            },
        },
    )
    .expect("a carry query is a valid request")
}

/// Every non-empty stack one carry snapshot reports.
fn carried_from(outcome: &ScriptOperationOutcome) -> Vec<(String, u32)> {
    let ScriptOperationPayload::OwnedInventory { result } = outcome.payload() else {
        panic!("expected an inventory payload, got {:?}", outcome.payload());
    };
    let mc_script::ScriptOwnedInventoryResult::Snapshot { inventory } = &**result else {
        panic!("expected an inventory snapshot, got {result:?}");
    };
    inventory
        .slots
        .iter()
        .filter_map(|slot| {
            slot.item
                .as_ref()
                .map(|item| (item.resource_id.clone(), item.count))
        })
        .collect()
}

/// The stacks in the worker's own carry slots, equipment excluded.
fn carry(storage: &PluginStorage, handle: &str) -> Vec<(String, u32)> {
    storage
        .resident_orders()
        .record(handle)
        .map(|record| {
            record
                .carry
                .iter()
                .flatten()
                .map(|stack| (stack.item_id.clone(), stack.count))
                .collect()
        })
        .unwrap_or_default()
}

/// (A08) A craft without inputs commits nothing; with inputs it consumes real
/// items and produces the recipe result.
#[tokio::test]
async fn craft_requires_inputs_and_consumes_them_exactly_once() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let craft = ScriptResidentWorkOrder::Craft {
        recipe: "minecraft:oak_planks".to_owned(),
        count: 2,
        station: crafting_station(),
    };

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request("craft-1", &handle, craft.clone(), 2, 0),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::MissingInput));
    assert!(assignment.changes.is_empty());

    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:oak_log", 2)]);
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request("craft-2", &handle, craft.clone(), 2, revision),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(
        assignment.state,
        ScriptWorkState::Committed,
        "{assignment:?}"
    );
    assert_eq!(assignment.work_units_done, 2);
    let changes = assignment
        .changes
        .iter()
        .map(|change| (change.item_id.as_str(), change.delta))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(changes.get("minecraft:oak_log"), Some(&-2));
    assert_eq!(changes.get("minecraft:oak_planks"), Some(&8));

    // The same operation id with a different payload conflicts instead of
    // crafting again.
    let conflicting = work_request(
        "craft-2",
        &handle,
        ScriptResidentWorkOrder::Craft {
            recipe: "minecraft:oak_planks".to_owned(),
            count: 1,
            station: crafting_station(),
        },
        1,
        assignment.revision,
    );
    let outcome = fixture.execute(&mut storage, &conflicting).await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::OperationConflict)
    );
    let planks = gear(&storage, &handle)
        .into_iter()
        .filter(|(item, _)| item == "minecraft:oak_planks")
        .map(|(_, count)| count)
        .sum::<u32>();
    assert_eq!(planks, 8, "no second craft ran");
}

/// (CP-012) A full inventory keeps every ingredient and leaves no partial output
/// when a recipe can fill only part of an existing output stack.
#[tokio::test]
async fn craft_without_output_capacity_preserves_ingredients() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let mut carry_fill: Vec<(&str, u32)> = (0..MAX_RESIDENT_CARRY_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    carry_fill[0] = ("minecraft:oak_planks", 62);
    carry_fill[1] = ("minecraft:oak_log", 64);
    let revision = seed_gear_in(&mut storage, &handle, uuid, &carry_fill, false);
    let equipment_fill: Vec<(&str, u32)> = (0..MAX_RESIDENT_EQUIPMENT_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    let revision = seed_gear_in(&mut storage, &handle, uuid, &equipment_fill, true).max(revision);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "craft-full-output",
                &handle,
                ScriptResidentWorkOrder::Craft {
                    recipe: "minecraft:oak_planks".to_owned(),
                    count: 1,
                    station: crafting_station(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);

    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::NoStorage));
    assert_eq!(assignment.work_units_done, 0);
    let contents = gear(&storage, &handle);
    assert_eq!(
        contents
            .iter()
            .filter(|(item, _)| item == "minecraft:oak_log")
            .map(|(_, count)| count)
            .sum::<u32>(),
        64,
        "the failed output placement restores the exact input stack"
    );
    assert_eq!(
        contents
            .iter()
            .filter(|(item, _)| item == "minecraft:oak_planks")
            .map(|(_, count)| count)
            .sum::<u32>(),
        62,
        "a failed partial merge leaves no phantom output"
    );
}

/// (CP-012) A craft pauses before consuming material when its named station
/// is absent, rather than treating an arbitrary loaded cell as a workshop.
#[tokio::test]
async fn craft_requires_the_named_crafting_table() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:oak_log", 1)]);
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "craft-without-table",
                &handle,
                ScriptResidentWorkOrder::Craft {
                    recipe: "minecraft:oak_planks".to_owned(),
                    count: 1,
                    station: area([7, SURFACE_Y, 7], [7, SURFACE_Y, 7]),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(
        assignment.reason,
        Some(ScriptWorkPauseReason::MissingStation)
    );
    assert_eq!(assignment.work_units_done, 0);
    assert!(
        gear(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:oak_log" && *count == 1),
        "a missing table leaves the craft input with its resident owner"
    );
}

/// (A08) Haul moves real items between the resident's canonical endpoints and
/// reports `missing_input` when the source is empty.
#[tokio::test]
async fn haul_moves_items_between_canonical_resident_endpoints() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    // The frozen DTO canonicalises the endpoints into enum order, so the
    // durable work is always `equipment -> carry`.
    let haul = ScriptResidentWorkOrder::Haul {
        source: mc_script::ScriptInventoryEndpoint::ResidentEquipment {
            handle: handle.clone(),
        },
        destination: mc_script::ScriptInventoryEndpoint::ResidentCarry {
            handle: handle.clone(),
        },
        item: None,
    };
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request("haul-1", &handle, haul.clone(), 4, 0),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::MissingInput));
    assert!(assignment.changes.is_empty());

    let revision = seed_gear_in(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:oak_log", 3)],
        true,
    );
    assert_eq!(
        gear(&storage, &handle),
        vec![("minecraft:oak_log".to_owned(), 3)],
        "seed landed in the canonical slots"
    );
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request("haul-2", &handle, haul, 4, revision),
        )
        .await;
    let assignment = work_of(&outcome);
    assert!(assignment.work_units_done >= 3, "{assignment:?}");
    assert_eq!(assignment.reason, None);
    let changes = assignment
        .changes
        .iter()
        .map(|change| (change.item_id.as_str(), change.delta))
        .collect::<BTreeMap<_, _>>();
    let moved = u32::try_from(*changes.get("minecraft:oak_log").expect("haul delta")).unwrap();
    assert_eq!(u64::from(moved), assignment.work_units_done);
    let record = storage
        .resident_orders()
        .record(&handle)
        .expect("the moved items are durable");
    assert_eq!(record.equipment[0], None);
    assert_eq!(
        record.carry[0].as_ref().map(|stack| stack.count),
        Some(moved)
    );
}

/// (CP-003) A haul that names an item moves only that item: the stack the order
/// did not ask for stays in the source, and the receipt reports the item that
/// really moved.
#[tokio::test]
async fn a_named_item_haul_leaves_every_other_stack_where_it_is() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    // Two stacks are in the canonical source, and the order names the second
    // one, so storage order alone would move the wrong item.
    let revision = seed_gear_in(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:oak_log", 3), ("minecraft:wheat", 5)],
        true,
    );
    let haul = ScriptResidentWorkOrder::Haul {
        source: mc_script::ScriptInventoryEndpoint::ResidentEquipment {
            handle: handle.clone(),
        },
        destination: mc_script::ScriptInventoryEndpoint::ResidentCarry {
            handle: handle.clone(),
        },
        item: Some("minecraft:wheat".to_owned()),
    };
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request("haul-named", &handle, haul, 8, revision),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, None, "{assignment:?}");
    assert_eq!(assignment.work_units_done, 5, "the whole wheat stack moved");
    assert_eq!(
        assignment
            .changes
            .iter()
            .map(|change| (change.item_id.as_str(), change.delta))
            .collect::<Vec<_>>(),
        vec![("minecraft:wheat", 5)],
        "the receipt names the item the order asked for"
    );
    // Read each endpoint on its own: the concatenated view cannot tell which
    // side a stack sits on.
    let record = storage
        .resident_orders()
        .record(&handle)
        .expect("the move is durable");
    assert_eq!(
        record.equipment[0]
            .as_ref()
            .map(|stack| stack.item_id.as_str()),
        Some("minecraft:oak_log"),
        "the stack the order did not ask for stayed in the source"
    );
    assert_eq!(
        record.carry[0]
            .as_ref()
            .map(|stack| (stack.item_id.as_str(), stack.count)),
        Some(("minecraft:wheat", 5)),
        "the named item landed in the destination"
    );
    assert_eq!(record.equipment[1], None, "no third stack appeared");
}

/// (CP-003) An order that names an item the source does not hold pauses as
/// `missing_input` and moves nothing, even when the source is not empty.
#[tokio::test]
async fn a_named_item_haul_of_an_absent_item_moves_nothing() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let revision = seed_gear_in(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:oak_log", 3)],
        true,
    );
    let haul = ScriptResidentWorkOrder::Haul {
        source: mc_script::ScriptInventoryEndpoint::ResidentEquipment {
            handle: handle.clone(),
        },
        destination: mc_script::ScriptInventoryEndpoint::ResidentCarry {
            handle: handle.clone(),
        },
        item: Some("minecraft:wheat".to_owned()),
    };
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request("haul-absent", &handle, haul, 4, revision),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::MissingInput));
    assert_eq!(assignment.work_units_done, 0);
    assert!(assignment.changes.is_empty());
    assert_eq!(
        gear(&storage, &handle),
        vec![("minecraft:oak_log".to_owned(), 3)],
        "the source is exactly as it was"
    );
}

/// (A09) A squad ordered through open terrain reforms into distinct standable
/// slots; the same order across a closed passage reports `blocked_route` with no
/// teleport and no shared coordinate.
#[tokio::test]
async fn squad_reforms_through_an_open_passage_and_refuses_a_closed_one() {
    for wall in [false, true] {
        let fixture = Fixture::new(wall);
        let mut storage = fixture.storage();
        let mut members = Vec::new();
        for slot in 3..6 {
            let member = fixture
                .resident(
                    &mut storage,
                    slot,
                    Vec3::new(2.5 + f64::from(slot), 64.0, 4.5),
                )
                .await;
            members.push(member);
        }
        let handles = members
            .iter()
            .map(|(handle, _)| handle.clone())
            .collect::<Vec<_>>();
        let request = order_request(
            if wall { "move-closed" } else { "move-open" },
            &handles,
            &vec![0; handles.len()],
            ScriptResidentOrder::Move {
                dimension: "minecraft:overworld".to_owned(),
                anchor: ScriptBlockPosition::new(13, 64, 8),
                heading_degrees: 0,
                formation: formation(),
            },
        );
        let outcome = fixture.execute(&mut storage, &request).await;
        assert_eq!(outcome.failure(), None, "batch admitted: {outcome:?}");
        let (_, outcomes, _) = order_of(&outcome);
        assert_eq!(outcomes.len(), 3);
        if wall {
            assert!(
                outcomes
                    .iter()
                    .all(|member| member.state == ScriptOrderMemberState::BlockedRoute),
                "closed passage: {outcomes:?}"
            );
            assert!(
                outcomes
                    .iter()
                    .all(|member| member.formation_slot.is_none())
            );
            for (_, uuid) in &members {
                // No teleport and no permanent shared coordinate.
                let position = fixture.position(*uuid).await;
                assert!(
                    position.x < 10.0,
                    "the member stayed near side: {position:?}"
                );
                assert_eq!(
                    fixture.goal(*uuid).await,
                    mc_entity::GoalState::Idle,
                    "no goal was pushed"
                );
            }
        } else {
            assert!(
                outcomes
                    .iter()
                    .all(|member| member.state == ScriptOrderMemberState::Applied),
                "open passage: {outcomes:?}"
            );
            let slots = outcomes
                .iter()
                .filter_map(|member| member.formation_slot)
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(slots.len(), 3, "every member has a distinct slot");
            let mut goals = Vec::new();
            for (_, uuid) in &members {
                let goal = fixture.goal(*uuid).await;
                assert!(
                    matches!(goal, mc_entity::GoalState::FollowPosition { .. }),
                    "a real movement goal was pushed: {goal:?}"
                );
                goals.push(goal);
            }
        }
    }
}

/// (A10) Combat is committed through the real damage path: an archer without
/// arrows deals nothing, a wall stops the shot, an allied resident is never
/// hit and a supplied archer really damages the hostile.
#[tokio::test]
async fn archer_ammo_line_of_sight_and_ally_policy_gate_committed_damage() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (archer_handle, archer) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    let (ally_handle, ally_uuid) = fixture
        .resident(&mut storage, 4, Vec3::new(5.5, 64.0, 5.5))
        .await;
    // The ally is a real resident entity whose spawn path is the regional
    // owner, not a session spawn, so register it in the same chunk index the
    // server maintains. Without that registration no local perception can see
    // it and the ally policy would be untestable.
    let ally_id = fixture
        .sessions
        .resident_entity_snapshots(&[ally_uuid])
        .await
        .into_iter()
        .next()
        .flatten()
        .expect("the ally stays live")
        .id;
    assert!(
        fixture.sessions.track_entity_chunk_for_test(ally_id),
        "the ally is registered for perception"
    );
    let zombie = fixture
        .sessions
        .spawn_tracked_entity_for_test(
            SpawnEntity::new(54, "minecraft:zombie", Vec3::new(6.5, 64.0, 4.5)),
            true,
        )
        .expect("zombie spawns");
    let zombie_snapshot = fixture
        .sessions
        .snapshot_entity_for_test(zombie)
        .expect("zombie is tracked");
    let zombie_uuid = zombie_snapshot.uuid;
    assert!(zombie_snapshot.health > 0.0);

    // A hold order perceives over the same local index and issues the
    // server-issued reference the attack order must name.
    let holders = vec![archer_handle.clone()];
    let hold = ScriptResidentOrder::Hold {
        anchor: ScriptBlockPosition::new(4, 64, 4),
        heading_degrees: 0,
        formation: formation(),
        engagement_radius: 16,
    };
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request("hold-1", &holders, &[0], hold.clone()),
        )
        .await;
    let (_, members, _) = order_of(&outcome);
    let perceived = members[0].targets.clone();
    assert!(
        perceived
            .iter()
            .any(|target| target.category == ScriptHostileCategory::Hostile),
        "the hostile is perceived: {perceived:?}"
    );
    assert!(
        !perceived
            .iter()
            .any(|target| { target.position == ScriptBlockPosition::new(5, 64, 5) }),
        "a proximity order offers hostile mobs only, never the allied resident: {perceived:?}"
    );
    let hostile_ref = perceived
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("hostile reference")
        .target_ref
        .clone();

    let attack_order = |targets: Vec<ScriptOrderTargetRef>, policy: ScriptEngagementPolicy| {
        ScriptResidentOrder::Attack { targets, policy }
    };
    let reference = |value: String| ScriptOrderTargetRef::new(value, 1, 0);
    let policy_with_ally = ScriptEngagementPolicy::new(
        1,
        vec![ally_handle.clone()],
        vec![
            ScriptHostileCategory::Hostile,
            ScriptHostileCategory::OwnedResident,
        ],
    );
    // A bow with no arrows: the archer engages nothing, no damage, no event.
    // The bow must be equipped, or the resident would simply punch.
    seed_gear(
        &mut storage,
        &archer_handle,
        archer,
        &[("minecraft:bow", 1)],
    );
    let bow_revision = equip_bow(&mut storage, &archer_handle);
    let health_before = fixture.health(zombie_uuid).await;
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "attack-no-ammo",
                &holders,
                &[bow_revision],
                attack_order(
                    vec![reference(hostile_ref.clone())],
                    policy_with_ally.clone(),
                ),
            ),
        )
        .await;
    let (_, _, combat) = order_of(&outcome);
    assert!(combat.is_empty(), "no ammo means no committed damage");
    assert_eq!(
        fixture.health(zombie_uuid).await,
        health_before,
        "health is unchanged by a refused engagement"
    );

    // With arrows the shot commits through the engine damage path.
    let ammo_revision = seed_gear(
        &mut storage,
        &archer_handle,
        archer,
        &[("minecraft:arrow", 8)],
    );
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "attack-arrows",
                &holders,
                &[ammo_revision],
                attack_order(
                    vec![reference(hostile_ref.clone())],
                    policy_with_ally.clone(),
                ),
            ),
        )
        .await;
    let (_, _, combat) = order_of(&outcome);
    assert_eq!(combat.len(), 1, "one committed volley: {combat:?}");
    assert_eq!(combat[0].attacker_handle, archer_handle);
    assert!(combat[0].damage_milli > 0);
    assert!(
        fixture.health(zombie_uuid).await < health_before,
        "the hostile really lost health"
    );
    assert_eq!(
        arrow_count(&storage, &archer_handle),
        7,
        "exactly one arrow was consumed"
    );
    let after_arrows = members_revision(&outcome);

    // The ally policy gate. The ally is a real target on the damage path: the
    // control volley, whose policy names no ally, hurts it through the same
    // issued reference; the same reference is refused once the policy covers
    // the ally.
    let neutral_policy = |allies: Vec<String>| {
        ScriptEngagementPolicy::new(1, allies, vec![ScriptHostileCategory::NeutralAnimal])
    };
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "attack-observe-ally",
                &holders,
                &[after_arrows],
                attack_order(
                    vec![reference(hostile_ref.clone())],
                    neutral_policy(Vec::new()),
                ),
            ),
        )
        .await;
    let (_, members, combat) = order_of(&outcome);
    assert!(
        combat.is_empty(),
        "a policy permitting neutral animals only never shoots the hostile: {combat:?}"
    );
    let ally_ref = members[0]
        .targets
        .iter()
        .find(|target| target.position == ScriptBlockPosition::new(5, 64, 5))
        .expect("the adjacent resident is issued when no ally excludes it")
        .target_ref
        .clone();

    let after_observe = members_revision(&outcome);
    let ally_health = fixture.health(ally_uuid).await;
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "attack-ally-control",
                &holders,
                &[after_observe],
                attack_order(
                    vec![reference(ally_ref.clone())],
                    neutral_policy(Vec::new()),
                ),
            ),
        )
        .await;
    let (_, _, combat) = order_of(&outcome);
    assert_eq!(
        combat.len(),
        1,
        "the control volley hits the resident: {combat:?}"
    );
    assert!(
        fixture.health(ally_uuid).await < ally_health,
        "the issued reference really addresses the resident"
    );
    let after_control = members_revision(&outcome);

    let ally_health = fixture.health(ally_uuid).await;
    let arrows_before = arrow_count(&storage, &archer_handle);
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "attack-ally-gated",
                &holders,
                &[after_control],
                attack_order(
                    vec![reference(ally_ref)],
                    ScriptEngagementPolicy::new(
                        1,
                        vec![ally_handle.clone()],
                        vec![
                            ScriptHostileCategory::Hostile,
                            ScriptHostileCategory::NeutralAnimal,
                        ],
                    ),
                ),
            ),
        )
        .await;
    let (_, members, combat) = order_of(&outcome);
    assert!(
        members[0]
            .targets
            .iter()
            .any(|target| target.category == ScriptHostileCategory::Hostile),
        "the hostile is still perceived: {:?}",
        members[0].targets
    );
    assert!(
        !members[0]
            .targets
            .iter()
            .any(|target| target.position == ScriptBlockPosition::new(5, 64, 5)),
        "an ally named by its durable handle is never issued as a target: {:?}",
        members[0].targets
    );
    assert!(
        combat.is_empty(),
        "an allied resident is never hit: {combat:?}"
    );
    assert_eq!(
        fixture.health(ally_uuid).await,
        ally_health,
        "the allied resident keeps its health"
    );
    assert_eq!(
        arrow_count(&storage, &archer_handle),
        arrows_before,
        "a refused volley consumes no ammunition"
    );

    // A wall between the archer and the hostile stops the shot.
    let walled = Fixture::new(true);
    let mut walled_storage = walled.storage();
    let (walled_handle, walled_archer) = walled
        .resident(&mut walled_storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    seed_gear(
        &mut walled_storage,
        &walled_handle,
        walled_archer,
        &[("minecraft:bow", 1), ("minecraft:arrow", 4)],
    );
    let walled_revision = equip_bow(&mut walled_storage, &walled_handle);
    let walled_holders = vec![walled_handle.clone()];
    let walled_zombie = walled
        .sessions
        .spawn_tracked_entity_for_test(
            SpawnEntity::new(54, "minecraft:zombie", Vec3::new(12.5, 64.0, 4.5)),
            true,
        )
        .expect("the hostile behind the wall spawns");
    let walled_uuid = walled
        .sessions
        .snapshot_entity_for_test(walled_zombie)
        .expect("the hostile behind the wall is tracked")
        .uuid;
    let outcome = walled
        .execute(
            &mut walled_storage,
            &order_request("wall-hold", &walled_holders, &[walled_revision], hold),
        )
        .await;
    let (_, members, _) = order_of(&outcome);
    let walled_ref = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("the hostile is perceived through the wall")
        .target_ref
        .clone();
    let walled_health = walled.health(walled_uuid).await;
    let walled_order_revision = members_revision(&outcome);
    let walled_order_revision = if walled_order_revision == 0 {
        1
    } else {
        walled_order_revision
    };
    let walled_policy =
        ScriptEngagementPolicy::new(1, Vec::new(), vec![ScriptHostileCategory::Hostile]);
    let outcome = walled
        .execute(
            &mut walled_storage,
            &order_request(
                "wall-attack",
                &walled_holders,
                &[walled_order_revision],
                attack_order(vec![reference(walled_ref)], walled_policy),
            ),
        )
        .await;
    let (_, _, combat) = order_of(&outcome);
    assert!(
        combat.is_empty(),
        "a wall blocks the committed shot: {combat:?}"
    );
    assert_eq!(
        walled.health(walled_uuid).await,
        walled_health,
        "the hostile behind the wall keeps its health"
    );
    assert_eq!(
        arrow_count(&walled_storage, &walled_handle),
        4,
        "a blocked shot consumes no ammunition"
    );
}

/// (A11) Retreat interrupts the attack chase and patrol restores the route
/// after the threat is gone.
#[tokio::test]
async fn retreat_interrupts_attack_and_patrol_walks_its_waypoints() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    let handles = vec![handle.clone()];
    fixture
        .sessions
        .spawn_tracked_entity_for_test(
            SpawnEntity::new(54, "minecraft:zombie", Vec3::new(6.5, 64.0, 4.5)),
            true,
        )
        .expect("zombie spawns");

    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "stage-1",
                &handles,
                &[0],
                ScriptResidentOrder::Hold {
                    anchor: ScriptBlockPosition::new(4, 64, 4),
                    heading_degrees: 0,
                    formation: formation(),
                    engagement_radius: 16,
                },
            ),
        )
        .await;
    let (_, members, _) = order_of(&outcome);
    let hostile_ref = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("hostile perceived")
        .target_ref
        .clone();
    let order_revision = members_revision(&outcome);

    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "stage-2",
                &handles,
                &[order_revision],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(hostile_ref.clone(), 1, 0)],
                    policy: ScriptEngagementPolicy::new(
                        1,
                        Vec::new(),
                        vec![ScriptHostileCategory::Hostile],
                    ),
                },
            ),
        )
        .await;
    let (_, _, _) = order_of(&outcome);
    let attack_revision = members_revision(&outcome);
    assert!(
        matches!(
            fixture.goal(uuid).await,
            mc_entity::GoalState::FollowTarget { .. }
        ),
        "attack chases the hostile"
    );

    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "stage-3",
                &handles,
                &[attack_revision],
                ScriptResidentOrder::Retreat {
                    anchor: ScriptBlockPosition::new(2, 64, 4),
                    formation: formation(),
                },
            ),
        )
        .await;
    let (_, _, _) = order_of(&outcome);
    let retreat_revision = members_revision(&outcome);
    assert!(
        matches!(
            fixture.goal(uuid).await,
            mc_entity::GoalState::FollowPosition { .. }
        ),
        "retreat replaces the chase"
    );
    assert_eq!(
        fixture.position(uuid).await.x,
        4.5,
        "retreat never teleports the member"
    );

    let waypoints = vec![
        ScriptBlockPosition::new(3, 64, 3),
        ScriptBlockPosition::new(3, 64, 5),
        ScriptBlockPosition::new(5, 64, 5),
    ];
    let patrol = ScriptResidentOrder::Patrol {
        waypoints: waypoints.clone(),
        formation: formation(),
        engagement_radius: 8,
    };
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request("stage-4", &handles, &[retreat_revision], patrol.clone()),
        )
        .await;
    let (_, _, _) = order_of(&outcome);
    let first_leg = fixture.goal(uuid).await;
    assert!(
        matches!(first_leg, mc_entity::GoalState::FollowPosition { .. }),
        "patrol walks a waypoint: {first_leg:?}"
    );
    let record = storage
        .resident_orders()
        .record(&handle)
        .cloned()
        .expect("patrol record");
    let first_index = record.order.as_ref().expect("patrol order").route_index;
    let patrol_revision = members_revision(&outcome);
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request("stage-5", &handles, &[patrol_revision], patrol.clone()),
        )
        .await;
    let (_, _, _) = order_of(&outcome);
    let record = storage
        .resident_orders()
        .record(&handle)
        .cloned()
        .expect("patrol record");
    assert_ne!(
        record.order.as_ref().expect("patrol order").route_index,
        first_index,
        "the route advances to the next waypoint"
    );
    let advanced_revision = members_revision(&outcome);

    // A threat interrupts the route: the member chases instead of walking.
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "stage-6",
                &handles,
                &[advanced_revision],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(hostile_ref, 1, 0)],
                    policy: ScriptEngagementPolicy::new(
                        1,
                        Vec::new(),
                        vec![ScriptHostileCategory::Hostile],
                    ),
                },
            ),
        )
        .await;
    let (_, _, _) = order_of(&outcome);
    let threat_leg = fixture.goal(uuid).await;
    assert!(
        matches!(threat_leg, mc_entity::GoalState::FollowTarget { .. }),
        "the threat interrupts the patrol route: {threat_leg:?}"
    );

    // Once the threat is gone, the same patrol order resumes the route.
    let threat_revision = members_revision(&outcome);
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request("stage-7", &handles, &[threat_revision], patrol),
        )
        .await;
    let (_, _, _) = order_of(&outcome);
    let resumed_leg = fixture.goal(uuid).await;
    assert!(
        matches!(resumed_leg, mc_entity::GoalState::FollowPosition { .. }),
        "the patrol returns to its route: {resumed_leg:?}"
    );
    let record = storage
        .resident_orders()
        .record(&handle)
        .cloned()
        .expect("patrol record");
    assert!(
        matches!(
            record.order.as_ref().expect("patrol order").order,
            ScriptResidentOrder::Patrol { .. }
        ),
        "the resumed order is the patrol route"
    );
}

/// (A11) A member that dies between prepare and commit invalidates the whole
/// batch: no member's order changes.
#[tokio::test]
async fn a_member_dying_between_prepare_and_commit_changes_no_order() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (first_handle, _first) = fixture
        .resident(&mut storage, 3, Vec3::new(3.5, 64.0, 3.5))
        .await;
    let (second_handle, second) = fixture
        .resident(&mut storage, 4, Vec3::new(4.5, 64.0, 3.5))
        .await;
    let handles = vec![first_handle.clone(), second_handle.clone()];

    let move_order = ScriptResidentOrder::Move {
        dimension: "minecraft:overworld".to_owned(),
        anchor: ScriptBlockPosition::new(3, 64, 6),
        heading_degrees: 0,
        formation: formation(),
    };
    // An accepted order first, so the stale batch has an old order to preserve.
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request("first", &handles, &[0, 0], move_order.clone()),
        )
        .await;
    assert_eq!(outcome.failure(), None, "{outcome:?}");
    let revisions = [members_revision(&outcome), members_revision(&outcome)];
    let before = storage
        .resident_orders()
        .record(&first_handle)
        .cloned()
        .expect("first record");
    let before_second = storage
        .resident_orders()
        .record(&second_handle)
        .cloned()
        .expect("second record");

    let records = crate::script::storage::resident_order_execution::resident_order_records(
        &storage, OWNER, &handles, &revisions,
    )
    .expect("records load");
    let request = order_request("second", &handles, &revisions, move_order);
    let prepared = fixture
        .runtime
        .prepare_resident_order_batch(&mut storage, &request, &records)
        .await
        .expect("prepared");
    let Some(mut admission) = prepared else {
        panic!("the batch prepared against live members");
    };

    // The member dies between prepare and commit.
    let entity = fixture
        .sessions
        .resident_entity_snapshots(&[second])
        .await
        .into_iter()
        .next()
        .flatten()
        .expect("second is live")
        .id;
    assert!(fixture.sessions.remove_tracked_entity_for_test(entity));

    assert!(
        !crate::script::storage::resident_order_execution::commit_resident_order_batch(
            &fixture.sessions,
            &records,
            &mut admission,
        )
        .await,
        "the drifted batch is refused wholesale"
    );
    assert_eq!(
        storage
            .resident_orders()
            .record(&first_handle)
            .cloned()
            .expect("first record"),
        before,
        "a surviving member keeps its old order"
    );
    assert_eq!(
        storage
            .resident_orders()
            .record(&second_handle)
            .cloned()
            .expect("second record"),
        before_second,
    );
}

/// (A12) A committed admission replays exactly once; the same operation id with
/// a new payload conflicts.
#[tokio::test]
async fn a_committed_admission_replays_exactly_once() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, _uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    let handles = vec![handle.clone()];
    let order = ScriptResidentOrder::Move {
        dimension: "minecraft:overworld".to_owned(),
        anchor: ScriptBlockPosition::new(4, 64, 8),
        heading_degrees: 0,
        formation: formation(),
    };
    let request = order_request("replay", &handles, &[0], order.clone());
    let outcome = fixture.execute(&mut storage, &request).await;
    assert_eq!(outcome.failure(), None, "{outcome:?}");
    let revision = members_revision(&outcome);
    let record = storage
        .resident_orders()
        .record(&handle)
        .cloned()
        .expect("record");
    assert!(storage.resident_orders().pending_admissions().is_empty());

    // A crash between the durable group commit and its engine application leaves
    // a committed admission with a pending member.
    let admission_id = storage
        .force_pending_admission_for_test(OWNER, "replay", std::slice::from_ref(&record))
        .expect("pending admission is durable");
    assert_eq!(
        storage.resident_orders().pending_admissions(),
        vec![(admission_id, handles.clone())]
    );

    // Re-open: the pending member is applied once and never again.
    let mut reopened = PluginStorage::open(fixture.storage_root.path()).unwrap();
    assert_eq!(
        reopened.resident_orders().pending_admissions(),
        vec![(admission_id, handles.clone())]
    );
    fixture.runtime.recover_resident_orders(&mut reopened).await;
    assert!(
        reopened.resident_orders().pending_admissions().is_empty(),
        "the accepted batch is applied exactly once"
    );
    fixture.runtime.recover_resident_orders(&mut reopened).await;
    assert!(
        reopened.resident_orders().pending_admissions().is_empty(),
        "a second replay has nothing left to apply"
    );

    // A repeated operation id with a new payload conflicts without any effect.
    let conflicting = order_request(
        "replay",
        &handles,
        &[revision],
        ScriptResidentOrder::Move {
            dimension: "minecraft:overworld".to_owned(),
            anchor: ScriptBlockPosition::new(2, 64, 2),
            heading_degrees: 0,
            formation: formation(),
        },
    );
    let outcome = fixture.execute(&mut reopened, &conflicting).await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::OperationConflict)
    );
    assert_eq!(
        reopened
            .resident_orders()
            .record(&handle)
            .expect("record")
            .order
            .as_ref()
            .expect("order")
            .order,
        order,
        "the conflicting call changed nothing"
    );
}

/// (f) Demobilisation without a reachable warehouse keeps the resident handle,
/// its order record and every item.
#[tokio::test]
async fn demobilisation_without_a_warehouse_keeps_handle_and_gear() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:iron_sword", 1), ("minecraft:arrow", 8)],
    );
    let before = gear(&storage, &handle);
    let revision = storage
        .resident_orders()
        .record(&handle)
        .expect("record")
        .revision;

    let outcome = fixture
        .execute(
            &mut storage,
            &demobilize_request("demob-1", &handle, revision),
        )
        .await;
    let ScriptOperationPayload::ResidentOrder { result } = outcome.payload() else {
        panic!("expected a resident order payload");
    };
    let ScriptResidentOrderResult::Demobilized { resident } = &**result else {
        panic!("expected a demobilisation result, got {result:?}");
    };
    assert_eq!(
        resident.state,
        mc_script::ScriptDemobilizeState::Demobilizing
    );
    assert_eq!(resident.reason, Some(ScriptWorkPauseReason::NoStorage));
    assert!(resident.returned.is_empty());
    let record = storage
        .resident_orders()
        .record(&handle)
        .cloned()
        .expect("record");
    assert_eq!(record.entity_uuid, uuid.to_string(), "same resident handle");
    assert_eq!(gear(&storage, &handle), before, "no item was lost");

    // Everything is durable: the reopened journal still shows the same state.
    let reopened = PluginStorage::open(fixture.storage_root.path()).unwrap();
    let record = reopened
        .resident_orders()
        .record(&handle)
        .cloned()
        .expect("record survives the reopen");
    assert_eq!(
        record.assignment,
        crate::script::storage::resident_orders::DurableAssignment::Demobilizing
    );
    assert_eq!(gear(&reopened, &handle), before);
}

fn members_revision(outcome: &ScriptOperationOutcome) -> u64 {
    match outcome.payload() {
        ScriptOperationPayload::ResidentOrder { result } => match &**result {
            ScriptResidentOrderResult::Order { order_revision, .. } => *order_revision,
            other => panic!("expected an order result, got {other:?}"),
        },
        other => panic!("expected a resident order payload, got {other:?}"),
    }
}

/// The focused refusal shape the batch tests rely on.
#[test]
fn refusals_carry_per_member_reasons() {
    let outcome =
        crate::script::storage::resident_order_execution::resident_order_batch_refusal(&[
            "resident-a".to_owned(),
            "resident-b".to_owned(),
        ]);
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));
    let members = refused_members(&outcome);
    assert_eq!(members.len(), 2);
    assert!(
        members
            .iter()
            .all(|member| member.state == ScriptOrderMemberState::StaleRevision)
    );
}

/// Keep the unused import honest: the canonical item stack type is exercised by
/// the seeded gear.
#[test]
fn canonical_item_stack_is_wired_to_the_registry() {
    let items = solaris_required_items();
    let name = codec::Identifier::parse("minecraft:oak_log").unwrap();
    let item_id = items.id_of(&name).expect("oak log item");
    let stack = ItemStack::new(item_id, 3);
    assert_eq!(stack.count, 3);
}
