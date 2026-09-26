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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

use crate::play::resident_work::{LiveResidentWorld, ResidentWorld, ResidentWorldEdit};
use crate::play::{ArrowPhysicsFact, EntityPhysicsStep, ResidentGoal, SessionRegistry};
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

    fn crop_is_mature(&self, state: u32) -> bool {
        self.inner.crop_is_mature(state)
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
    standable_queries: Arc<AtomicUsize>,
    route_queries: Arc<AtomicUsize>,
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
        self.standable_queries.fetch_add(1, Ordering::Relaxed);
        self.inner.standable(dimension, pos)
    }

    fn route_open(&self, dimension: &str, from: [i32; 3], to: [i32; 3]) -> Option<bool> {
        self.route_queries.fetch_add(1, Ordering::Relaxed);
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

    fn crop_is_mature(&self, state: u32) -> bool {
        self.inner.crop_is_mature(state)
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
        Self::with_options(wall, None, false, false, None, None)
    }

    fn with_replacement(wall: bool, replacement: Option<&str>) -> Self {
        Self::with_options(wall, replacement, false, false, None, None)
    }

    fn with_replant_replacement(wall: bool, replacement: &str) -> Self {
        Self::with_options(wall, Some(replacement), false, true, None, None)
    }

    fn with_regional_edge_crop(wall: bool) -> Self {
        Self::with_options(wall, None, true, false, None, None)
    }

    fn with_protection_gate(protected: Arc<AtomicBool>) -> Self {
        Self::with_options(false, None, false, false, Some(protected), None)
    }

    fn with_query_counts(
        wall: bool,
        standable: Arc<AtomicUsize>,
        routes: Arc<AtomicUsize>,
    ) -> Self {
        Self::with_options(
            wall,
            None,
            false,
            false,
            Some(Arc::new(AtomicBool::new(false))),
            Some((standable, routes)),
        )
    }

    fn with_options(
        wall: bool,
        replacement: Option<&str>,
        regional_edge_crop: bool,
        only_placements: bool,
        protected: Option<Arc<AtomicBool>>,
        query_counts: Option<(Arc<AtomicUsize>, Arc<AtomicUsize>)>,
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
            // A narrow, loaded stair exposes the ore from a real reachable
            // stance; all other stone-band cells remain sealed.
            chunk.set_block(4, SURFACE_Y, 7, air);
            chunk.set_block(4, SURFACE_Y - 1, 6, air);
            chunk.set_block(4, SURFACE_Y, 6, air);
            chunk.set_block(4, SURFACE_Y - 2, 5, air);
            chunk.set_block(4, SURFACE_Y - 1, 5, air);
            chunk.set_block(5, SURFACE_Y - 2, 5, air);
            chunk.set_block(5, SURFACE_Y - 1, 5, air);
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
            (None, Some(protected)) => {
                let (standable_queries, route_queries) = query_counts.unwrap_or_else(|| {
                    (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)))
                });
                Arc::new(ProtectionGatedResidentWorld {
                    inner: adapter,
                    protected,
                    standable_queries,
                    route_queries,
                })
            }
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
    fn enable_arrows(&self) {
        self.sessions.configure_arrow_kill_rewards(
            None,
            None,
            Some(77),
            Arc::new(solaris_required_items()),
            Arc::new(solaris_required_item_facts()),
            Arc::new(mc_data::loot::LootTables::default()),
        );
    }

    fn advance_resident_arrows(&self, tick: u64, center: Vec3) -> usize {
        let arrows = self.sessions.resident_arrows_for_test(center);
        let steps = arrows
            .iter()
            .map(|arrow| EntityPhysicsStep {
                id: arrow.id,
                position: Vec3::new(
                    arrow.position.x + arrow.velocity.x,
                    arrow.position.y + arrow.velocity.y,
                    arrow.position.z + arrow.velocity.z,
                ),
                velocity: arrow.velocity,
                on_ground: false,
                horizontal_collision: false,
            })
            .collect::<Vec<_>>();
        let facts = arrows
            .iter()
            .map(|arrow| ArrowPhysicsFact {
                arrow_id: arrow.id,
                block_hit: None,
                embedded_in_block: false,
                current_block_state: mc_world::BlockStateId(0),
                should_fall: false,
                fall_velocity_scale: Vec3::new(0.1, 0.1, 0.1),
                in_water: false,
                in_water_or_rain: false,
            })
            .collect::<Vec<_>>();
        self.sessions
            .apply_entity_physics_with_arrow_facts_and_dispatch(tick, &steps, &facts);
        arrows.len()
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

    /// Materialise a resident at the fixture's only physical mine entrance.
    async fn mining_resident(&self, storage: &mut PluginStorage, slot: u32) -> (String, Uuid) {
        self.resident(storage, slot, Vec3::new(4.5, 64.0, 8.5))
            .await
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

fn capture_request(
    operation_id: &str,
    handle: &str,
    custodian: &str,
    expected_revision: u64,
) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::ResidentOrder {
            operation: ScriptResidentOrderOperation::Capture {
                operation_id: operation_id.to_owned(),
                handle: handle.to_owned(),
                custodian: custodian.to_owned(),
                expected_revision,
            },
        },
    )
    .expect("valid capture request")
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

/// (CP-013) A growth update retries the same harvest that paused only because
/// its crop was immature; a later unchanged world event cannot harvest twice.
#[tokio::test]
async fn world_event_resumes_the_same_matured_harvest_once() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 23, Vec3::new(2.5, 64.0, 5.5))
        .await;
    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos { x: 2, y: 64, z: 2 },
            state_with_props(&fixture.blocks, "wheat", &[("age", "6")]),
        )
        .expect("fixture field accepts an immature crop");
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]);
    let paused = fixture
        .execute(
            &mut storage,
            &work_request(
                "pause-immature-harvest",
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
        Some(ScriptWorkPauseReason::MissingInput)
    );
    drop(storage);
    let mut storage = fixture.storage();

    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos { x: 2, y: 64, z: 2 },
            state_with_props(&fixture.blocks, "wheat", &[("age", "7")]),
        )
        .expect("fixture field accepts the growth update");
    let resumed = fixture
        .runtime
        .resume_paused_work_for_chunks(&mut storage, "minecraft:overworld", &[[0, 0]], |_| true)
        .await
        .expect("mature crop resumes the paused harvest");
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
            .resume_paused_work_for_chunks(
                &mut storage,
                "minecraft:overworld",
                &[[0, 0]],
                |_| true,
            )
            .await
            .expect("completed harvest cannot loop")
            .is_empty()
    );
}

/// (CP-013) A missing craft ingredient waits for the addressed inventory
/// transfer, not every changed block in the same active work region.
#[tokio::test]
async fn world_event_does_not_retry_inventory_missing_craft() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 24, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let paused = fixture
        .execute(
            &mut storage,
            &work_request(
                "pause-missing-craft-input",
                &handle,
                ScriptResidentWorkOrder::Craft {
                    recipe: "minecraft:oak_planks".to_owned(),
                    count: 1,
                    station: crafting_station(),
                },
                1,
                0,
            ),
        )
        .await;
    assert_eq!(
        work_of(&paused).reason,
        Some(ScriptWorkPauseReason::MissingInput)
    );
    let paused_revision = storage
        .resident_orders()
        .record(&handle)
        .expect("paused craft is durable")
        .revision;
    assert!(
        fixture
            .runtime
            .resume_paused_work_for_chunks(
                &mut storage,
                "minecraft:overworld",
                &[[0, 0]],
                |_| true,
            )
            .await
            .expect("world wake stays bounded to source waits")
            .is_empty()
    );
    assert_eq!(
        storage
            .resident_orders()
            .record(&handle)
            .expect("world wake preserves the paused craft")
            .revision,
        paused_revision,
        "an unrelated world wake must not create a replacement receipt"
    );

    seed_gear(&mut storage, &handle, uuid, &[("minecraft:oak_log", 1)]);
    let resumed = fixture
        .runtime
        .resume_paused_work_for_inventory_change(
            &mut storage,
            std::slice::from_ref(&handle),
            &[],
            |_| true,
        )
        .await
        .expect("addressed inventory event resumes the craft");
    assert_eq!(resumed.len(), 1);
    assert_eq!(
        work_of(&resumed[0].outcome).state,
        ScriptWorkState::Committed
    );
}

/// (CP-013) Cancelling a source wait removes its world wake classification
/// before the record reaches durable replay.
#[tokio::test]
async fn cancelling_a_world_input_wait_reopens_cleanly() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 25, Vec3::new(2.5, 64.0, 5.5))
        .await;
    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos { x: 2, y: 64, z: 2 },
            state_with_props(&fixture.blocks, "wheat", &[("age", "6")]),
        )
        .expect("fixture field accepts an immature crop");
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]);
    let paused = fixture
        .execute(
            &mut storage,
            &work_request(
                "pause-before-cancel",
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
        Some(ScriptWorkPauseReason::MissingInput)
    );
    let paused_revision = storage
        .resident_orders()
        .record(&handle)
        .expect("paused work is durable")
        .revision;
    let cancel = ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::ResidentOrder {
            operation: ScriptResidentOrderOperation::CancelWork {
                operation_id: "cancel-world-wait".to_owned(),
                handle: handle.clone(),
                expected_revision: paused_revision,
            },
        },
    )
    .expect("valid cancel request");
    fixture.execute(&mut storage, &cancel).await;

    drop(storage);
    let reopened = fixture.storage();
    let record = reopened
        .resident_orders()
        .record(&handle)
        .expect("cancelled work survives storage replay");
    let work = record.work.as_ref().expect("cancelled work record");
    assert_eq!(work.state, ScriptWorkState::Cancelled);
    assert_eq!(work.reason, None);
    assert!(!work.world_input_wait);
    assert_eq!(work.revision, record.revision);
    assert_ne!(work.revision, 0);
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

/// (CP-007) A mining receipt recovers its exact ore cargo and work progress
/// after storage projection fails following the accepted world decision.
#[tokio::test]
async fn mine_recovers_cargo_and_progress_after_storage_projection_fails() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture.mining_resident(&mut storage, 3).await;
    let revision = seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:iron_pickaxe", 1)],
    );
    let request = work_request(
        "mine-projection-recovery",
        &handle,
        ScriptResidentWorkOrder::Mine {
            area: area([4, SURFACE_Y - 2, 4], [4, SURFACE_Y - 2, 4]),
            tool: "minecraft:iron_pickaxe".to_owned(),
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
        fixture.block(4, SURFACE_Y - 2, 4).await,
        Some(state_of(&fixture.blocks, "air")),
        "the accepted world decision keeps the broken ore"
    );

    drop(storage);
    let mut reopened = fixture.storage();
    fixture
        .runtime
        .recover(&mut reopened)
        .expect("the accepted mine receipt projects at recovery");
    let recovered = fixture.execute(&mut reopened, &request).await;
    assert_eq!(work_of(&recovered).work_units_done, 1);
    let raw_iron = carry(&reopened, &handle)
        .iter()
        .filter(|(item, _)| item == "minecraft:raw_iron")
        .map(|(_, count)| *count)
        .sum::<u32>();
    assert_eq!(raw_iron, 1, "recovery must not duplicate canonical ore");
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

/// (CP-008) A replanted crop only starts its next cycle after the real world
/// reports maturity; its second harvest and replant retain worker-owned output.
#[tokio::test]
async fn a_matured_replanted_crop_completes_a_second_cycle() {
    let fixture = Fixture::with_regional_edge_crop(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(0.5, 64.0, 5.5))
        .await;
    let harvest_work = ScriptResidentWorkOrder::Harvest {
        area: area([0, 64, 2], [0, 64, 2]),
        tool: "minecraft:iron_hoe".to_owned(),
    };
    let replant_work = ScriptResidentWorkOrder::Replant {
        area: area([0, SURFACE_Y, 2], [0, SURFACE_Y, 2]),
        seed: "minecraft:wheat_seeds".to_owned(),
        tool: "minecraft:iron_hoe".to_owned(),
    };
    let mut revision = seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:iron_hoe", 1), ("minecraft:wheat_seeds", 3)],
    );
    for (operation_id, work) in [
        ("first-harvest", harvest_work.clone()),
        ("first-replant", replant_work.clone()),
    ] {
        let outcome = fixture
            .execute(
                &mut storage,
                &work_request(operation_id, &handle, work, 1, revision),
            )
            .await;
        assert_eq!(outcome.failure(), None, "{outcome:?}");
        revision = storage
            .resident_orders()
            .record(&handle)
            .expect("completed first-cycle record")
            .revision;
    }
    assert_eq!(
        fixture.block(0, 64, 2).await,
        Some(state_of(&fixture.blocks, "wheat")),
        "a newly replanted crop is not mature by receipt alone"
    );
    let premature = fixture
        .execute(
            &mut storage,
            &work_request(
                "premature-second-harvest",
                &handle,
                harvest_work.clone(),
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(
        work_of(&premature).reason,
        Some(ScriptWorkPauseReason::MissingInput),
        "an immature crop cannot become loot before a world growth update"
    );
    assert_eq!(work_of(&premature).work_units_done, 0);
    revision = storage
        .resident_orders()
        .record(&handle)
        .expect("paused premature harvest record")
        .revision;
    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos { x: 0, y: 64, z: 2 },
            state_with_props(&fixture.blocks, "wheat", &[("age", "7")]),
        )
        .expect("the loaded field accepts a real growth update");
    for (operation_id, work) in [
        ("second-harvest", harvest_work),
        ("second-replant", replant_work),
    ] {
        let outcome = fixture
            .execute(
                &mut storage,
                &work_request(operation_id, &handle, work, 1, revision),
            )
            .await;
        assert_eq!(outcome.failure(), None, "{outcome:?}");
        revision = storage
            .resident_orders()
            .record(&handle)
            .expect("completed second-cycle record")
            .revision;
    }
    assert_eq!(
        fixture.block(0, 64, 2).await,
        Some(state_of(&fixture.blocks, "wheat")),
        "the second cycle replants the world-grown crop"
    );
    assert!(
        carry(&storage, &handle)
            .iter()
            .filter(|(item, _)| item == "minecraft:wheat")
            .map(|(_, count)| *count)
            .sum::<u32>()
            >= 2,
        "both actual harvests remain worker-owned"
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
    let (handle, uuid) = fixture.mining_resident(&mut storage, 3).await;
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
    let (handle, uuid) = fixture.mining_resident(&mut storage, 3).await;
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
    let (handle, uuid) = fixture.mining_resident(&mut storage, 3).await;
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
    let (handle, uuid) = fixture.mining_resident(&mut storage, 3).await;
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

/// (CP-010) An ore block inside solid terrain remains unavailable until a
/// resident can reach one of its bounded mining stances.
#[tokio::test]
async fn mine_does_not_extract_a_hidden_ore_without_a_route() {
    let fixture = Fixture::new(false);
    let hidden = [10, SURFACE_Y - 2, 10];
    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos {
                x: hidden[0],
                y: hidden[1],
                z: hidden[2],
            },
            state_of(&fixture.blocks, "iron_ore"),
        )
        .expect("hidden ore remains inside the loaded stone band");
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture.mining_resident(&mut storage, 3).await;
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
                "mine-hidden-ore",
                &handle,
                ScriptResidentWorkOrder::Mine {
                    area: area(hidden, hidden),
                    tool: "minecraft:iron_pickaxe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).state, ScriptWorkState::Paused);
    assert_eq!(
        work_of(&outcome).reason,
        Some(ScriptWorkPauseReason::BlockedRoute)
    );
    assert_eq!(work_of(&outcome).work_units_done, 0);
    assert_eq!(
        fixture.block(hidden[0], hidden[1], hidden[2]).await,
        Some(state_of(&fixture.blocks, "iron_ore"))
    );
    assert!(
        !carry(&storage, &handle)
            .iter()
            .any(|(item, _)| item == "minecraft:raw_iron"),
        "a sealed ore cannot become worker cargo"
    );
}

/// (CP-010) Protected mine cells pause before a physical preview can alter the
/// world or the worker's cargo.
#[tokio::test]
async fn mine_in_a_protected_zone_leaves_the_ore_untouched() {
    let protected = Arc::new(AtomicBool::new(true));
    let fixture = Fixture::with_protection_gate(protected);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture.mining_resident(&mut storage, 3).await;
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
                "mine-protected-ore",
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

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).state, ScriptWorkState::Paused);
    assert_eq!(
        work_of(&outcome).reason,
        Some(ScriptWorkPauseReason::Protected)
    );
    assert_eq!(work_of(&outcome).work_units_done, 0);
    assert_eq!(
        fixture.block(4, SURFACE_Y - 2, 4).await,
        Some(state_of(&fixture.blocks, "iron_ore"))
    );
}

/// (CP-010) A source changed after an accessible mine preview commits neither
/// canonical ore loot nor progress.
#[tokio::test]
async fn replaced_ore_after_preview_does_not_create_cargo_or_work_progress() {
    let fixture = Fixture::with_replacement(false, Some("stone"));
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture.mining_resident(&mut storage, 3).await;
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
                "mine-replaced-after-preview",
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

    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::StaleRevision)
    );
    assert_eq!(
        fixture.block(4, SURFACE_Y - 2, 4).await,
        Some(state_of(&fixture.blocks, "stone"))
    );
    assert!(
        !carry(&storage, &handle)
            .iter()
            .any(|(item, _)| item == "minecraft:raw_iron"),
        "a stale mine preview cannot mint ore cargo"
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

/// (CP-010) An unloaded work cell has no route or loot fallback and leaves the
/// resident work record at zero progress.
#[tokio::test]
async fn mine_in_an_unloaded_area_pauses_without_cargo() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture.mining_resident(&mut storage, 3).await;
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
                "mine-unloaded-ore",
                &handle,
                ScriptResidentWorkOrder::Mine {
                    area: area([32, SURFACE_Y - 2, 32], [32, SURFACE_Y - 2, 32]),
                    tool: "minecraft:iron_pickaxe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).state, ScriptWorkState::Paused);
    assert_eq!(
        work_of(&outcome).reason,
        Some(ScriptWorkPauseReason::Unloaded)
    );
    assert_eq!(work_of(&outcome).work_units_done, 0);
    assert!(
        !carry(&storage, &handle)
            .iter()
            .any(|(item, _)| item == "minecraft:raw_iron")
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

/// (CP-009) Every trunk cell in one bounded taller tree shares the route to
/// its rooted standing position, so one receipt cannot strand the upper log.
#[tokio::test]
async fn cut_tree_reaches_the_full_tall_trunk_from_its_rooted_route() {
    let fixture = Fixture::new(false);
    let log = state_of(&fixture.blocks, "oak_log");
    let leaves = state_of(&fixture.blocks, "oak_leaves");
    {
        let mut world = fixture.world.lock().await;
        world
            .set_block_at(
                BlockPos {
                    x: 6,
                    y: SURFACE_Y + 3,
                    z: 6,
                },
                log,
            )
            .expect("the fixture canopy column accepts the taller trunk");
        for x in 5..=7 {
            for z in 5..=7 {
                world
                    .set_block_at(
                        BlockPos {
                            x,
                            y: SURFACE_Y + 4,
                            z,
                        },
                        leaves,
                    )
                    .expect("the fixture accepts the lifted canopy");
            }
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
                "cut-tall-tree",
                &handle,
                ScriptResidentWorkOrder::CutTree {
                    area: area([6, SURFACE_Y + 1, 6], [6, SURFACE_Y + 3, 6]),
                    tool: "minecraft:iron_axe".to_owned(),
                },
                3,
                revision,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).work_units_done, 3);
    assert_eq!(
        carry(&storage, &handle)
            .iter()
            .filter(|(item, _)| item == "minecraft:oak_log")
            .map(|(_, count)| *count)
            .sum::<u32>(),
        3
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

/// (CP-009) A real tree behind a blocked route remains in the world; a work
/// receipt does not teleport the logger into a reachable-looking area.
#[tokio::test]
async fn cut_tree_with_no_standable_route_leaves_the_trunk_untouched() {
    let fixture = Fixture::new(false);
    let stone = state_of(&fixture.blocks, "stone");
    {
        let mut world = fixture.world.lock().await;
        for (x, z) in [(5, 6), (7, 6), (6, 5), (6, 7)] {
            world
                .set_block_at(
                    BlockPos {
                        x,
                        y: SURFACE_Y + 1,
                        z,
                    },
                    stone,
                )
                .expect("the loaded fixture can close every adjacent route");
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
                "cut-tree-blocked-route",
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
    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::BlockedRoute));
    assert_eq!(assignment.work_units_done, 0);
    assert_eq!(
        fixture.block(6, SURFACE_Y + 1, 6).await,
        Some(state_of(&fixture.blocks, "oak_log")),
        "the unreachable trunk remains intact"
    );
    assert!(
        !carry(&storage, &handle)
            .iter()
            .any(|(item, _)| item == "minecraft:oak_log"),
        "an unreachable tree cannot mint log cargo"
    );
}

/// (CP-009) A logger without its real axe cannot consume an otherwise valid
/// natural trunk.
#[tokio::test]
async fn cut_tree_without_an_axe_leaves_the_tree_untouched() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, _uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(6.5, 64.0, 5.5))
        .await;
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "cut-tree-without-axe",
                &handle,
                ScriptResidentWorkOrder::CutTree {
                    area: area([6, SURFACE_Y + 1, 6], [6, SURFACE_Y + 2, 6]),
                    tool: "minecraft:iron_axe".to_owned(),
                },
                1,
                0,
            ),
        )
        .await;
    assert_eq!(
        work_of(&outcome).reason,
        Some(ScriptWorkPauseReason::MissingTool)
    );
    assert_eq!(
        fixture.block(6, SURFACE_Y + 1, 6).await,
        Some(state_of(&fixture.blocks, "oak_log"))
    );
}

/// (CP-009) A full logger cannot turn a real tree into unowned drops.
#[tokio::test]
async fn cut_tree_with_full_cargo_leaves_the_tree_untouched() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(6.5, 64.0, 5.5))
        .await;
    let fill: Vec<(&str, u32)> = (0..MAX_RESIDENT_CARRY_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    let mut revision = seed_gear_in(&mut storage, &handle, uuid, &fill, false);
    let equipment: Vec<(&str, u32)> = (0..MAX_RESIDENT_EQUIPMENT_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    revision = seed_gear_in(&mut storage, &handle, uuid, &equipment, true).max(revision);
    revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_axe", 1)]).max(revision);
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "cut-tree-full",
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
    assert_eq!(
        work_of(&outcome).reason,
        Some(ScriptWorkPauseReason::NoStorage)
    );
    assert_eq!(
        fixture.block(6, SURFACE_Y + 1, 6).await,
        Some(state_of(&fixture.blocks, "oak_log"))
    );
}

/// (CP-009) Removing the matching permission fence resumes the same logger
/// once; an unchanged protected tree remains unmodified.
#[tokio::test]
async fn released_tree_permission_resumes_the_paused_logger_once() {
    let protected = Arc::new(AtomicBool::new(true));
    let fixture = Fixture::with_protection_gate(Arc::clone(&protected));
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(6.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_axe", 1)]);
    let paused = fixture
        .execute(
            &mut storage,
            &work_request(
                "pause-protected-tree",
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
    assert_eq!(
        work_of(&paused).reason,
        Some(ScriptWorkPauseReason::Protected)
    );
    assert_eq!(
        fixture.block(6, SURFACE_Y + 1, 6).await,
        Some(state_of(&fixture.blocks, "oak_log"))
    );
    let released_zone = ScriptAxisAlignedZone::try_new(
        "released-tree",
        "minecraft:overworld",
        ScriptPosition::try_new(5.5, 63.0, 5.5).expect("valid zone minimum"),
        ScriptPosition::try_new(7.5, 66.0, 7.5).expect("valid zone maximum"),
    )
    .expect("valid released zone");
    protected.store(false, Ordering::Release);
    let resumed = fixture
        .runtime
        .resume_paused_work_for_zone(&mut storage, &released_zone, |_| true)
        .await
        .expect("released tree permission resumes work");
    assert_eq!(resumed.len(), 1);
    assert_eq!(
        work_of(&resumed[0].outcome).state,
        ScriptWorkState::Committed
    );
    assert_eq!(
        fixture.block(6, SURFACE_Y + 1, 6).await,
        Some(state_of(&fixture.blocks, "air"))
    );
    assert!(
        fixture
            .runtime
            .resume_paused_work_for_zone(&mut storage, &released_zone, |_| true)
            .await
            .expect("the completed logger cannot resume twice")
            .is_empty()
    );
}
/// (CP-011) Fishing pays a durable owner only from water currently present in
/// the named work area; its deterministic cod result is not a vanilla-loot claim.
#[tokio::test]
async fn fish_uses_real_water_and_credits_resident_cargo() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(7.5, 64.0, 8.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:fishing_rod", 1)]);
    let request = work_request(
        "fish-water-column",
        &handle,
        ScriptResidentWorkOrder::Fish {
            area: area([8, SURFACE_Y, 8], [8, SURFACE_Y, 8]),
            tool: "minecraft:fishing_rod".to_owned(),
        },
        1,
        revision,
    );
    let outcome = fixture.execute(&mut storage, &request).await;
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
    drop(storage);
    let mut reopened = fixture.storage();
    let replay = fixture.execute(&mut reopened, &request).await;
    assert_eq!(work_of(&replay).work_units_done, 1);
    assert_eq!(
        carry(&reopened, &handle)
            .iter()
            .filter(|(item, _)| item == "minecraft:cod")
            .map(|(_, count)| *count)
            .sum::<u32>(),
        1,
        "restart and replay preserve the one committed catch"
    );
}

/// (CP-011) A changed water source cannot produce another catch after the
/// previous durable result.
#[tokio::test]
async fn fish_stops_when_its_water_source_is_removed() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(7.5, 64.0, 8.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:fishing_rod", 1)]);
    let work = ScriptResidentWorkOrder::Fish {
        area: area([8, SURFACE_Y, 8], [8, SURFACE_Y, 8]),
        tool: "minecraft:fishing_rod".to_owned(),
    };
    let first = fixture
        .execute(
            &mut storage,
            &work_request(
                "fish-before-water-removal",
                &handle,
                work.clone(),
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(work_of(&first).work_units_done, 1);
    let revision = storage
        .resident_orders()
        .record(&handle)
        .expect("first fish work record")
        .revision;
    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos {
                x: 8,
                y: SURFACE_Y,
                z: 8,
            },
            state_of(&fixture.blocks, "air"),
        )
        .expect("fixture water cell remains loaded");

    let removed = fixture
        .execute(
            &mut storage,
            &work_request("fish-after-water-removal", &handle, work, 2, revision),
        )
        .await;
    assert_eq!(removed.failure(), None, "{removed:?}");
    assert_eq!(work_of(&removed).state, ScriptWorkState::Paused);
    assert_eq!(
        work_of(&removed).reason,
        Some(ScriptWorkPauseReason::MissingInput)
    );
    assert_eq!(work_of(&removed).work_units_done, 0);
    assert_eq!(
        carry(&storage, &handle)
            .iter()
            .filter(|(item, _)| item == "minecraft:cod")
            .map(|(_, count)| *count)
            .sum::<u32>(),
        1
    );
}

/// (CP-011) A fisher with no cargo space retains its real water source and
/// receives neither a virtual catch nor progress.
#[tokio::test]
async fn fish_with_full_cargo_pauses_without_a_catch() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(7.5, 64.0, 8.5))
        .await;
    let fill: Vec<(&str, u32)> = (0..MAX_RESIDENT_CARRY_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    let mut revision = seed_gear_in(&mut storage, &handle, uuid, &fill, false);
    let equipment: Vec<(&str, u32)> = (0..MAX_RESIDENT_EQUIPMENT_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    revision = seed_gear_in(&mut storage, &handle, uuid, &equipment, true).max(revision);
    revision =
        seed_gear(&mut storage, &handle, uuid, &[("minecraft:fishing_rod", 1)]).max(revision);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "fish-full-cargo",
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

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).state, ScriptWorkState::Paused);
    assert_eq!(
        work_of(&outcome).reason,
        Some(ScriptWorkPauseReason::NoStorage)
    );
    assert_eq!(work_of(&outcome).work_units_done, 0);
    assert!(
        !carry(&storage, &handle)
            .iter()
            .any(|(item, _)| item == "minecraft:cod")
    );
}

/// (CP-011) Visible water behind a closed route is not a remote fishing source.
#[tokio::test]
async fn fish_with_no_route_leaves_the_water_and_cargo_untouched() {
    let fixture = Fixture::new(true);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(12.5, 64.0, 8.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:fishing_rod", 1)]);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "fish-blocked-water",
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

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).state, ScriptWorkState::Paused);
    assert_eq!(
        work_of(&outcome).reason,
        Some(ScriptWorkPauseReason::BlockedRoute)
    );
    assert_eq!(work_of(&outcome).work_units_done, 0);
    assert_eq!(
        fixture.block(8, SURFACE_Y, 8).await,
        Some(state_of(&fixture.blocks, "water"))
    );
    assert!(
        !carry(&storage, &handle)
            .iter()
            .any(|(item, _)| item == "minecraft:cod")
    );
}

/// (CP-011) An unloaded first shore candidate does not hide a later loaded,
/// reachable shore at a chunk boundary.
#[tokio::test]
async fn fish_uses_a_loaded_boundary_shore_after_an_unloaded_stance() {
    let fixture = Fixture::new(false);
    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos {
                x: 0,
                y: SURFACE_Y,
                z: 8,
            },
            state_of(&fixture.blocks, "water"),
        )
        .expect("boundary water remains in the loaded fixture chunk");
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(0.5, 64.0, 7.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:fishing_rod", 1)]);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "fish-boundary-shore",
                &handle,
                ScriptResidentWorkOrder::Fish {
                    area: area([0, SURFACE_Y, 8], [0, SURFACE_Y, 8]),
                    tool: "minecraft:fishing_rod".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).state, ScriptWorkState::Committed);
    assert_eq!(work_of(&outcome).work_units_done, 1);
}

/// (CP-011) An unloaded first animal stance does not mask a later loaded,
/// reachable stance at the same chunk boundary.
#[tokio::test]
async fn tend_livestock_uses_a_loaded_boundary_stance_after_an_unloaded_one() {
    let fixture = Fixture::new(false);
    let mut cow = SpawnEntity::new(0, "minecraft:cow", Vec3::new(0.5, 64.0, 8.5));
    cow.animal = Some(mc_entity::AnimalBreedingState::adult());
    fixture
        .sessions
        .spawn_tracked_entity_for_test(cow, false)
        .expect("cow joins the local session index");
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(0.5, 64.0, 7.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:wheat", 1)]);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "tend-boundary-cow",
                &handle,
                ScriptResidentWorkOrder::TendLivestock {
                    area: area([0, 64, 8], [0, 64, 8]),
                    feed: "minecraft:wheat".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).state, ScriptWorkState::Committed);
    assert_eq!(work_of(&outcome).work_units_done, 1);
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

/// (CP-011) A live compatible animal does not turn an absent feed reserve into
/// a completed tending unit.
#[tokio::test]
async fn tend_livestock_without_feed_pauses_before_work() {
    let fixture = Fixture::new(false);
    let mut cow = SpawnEntity::new(0, "minecraft:cow", Vec3::new(6.5, 64.0, 6.5));
    cow.animal = Some(mc_entity::AnimalBreedingState::adult());
    fixture
        .sessions
        .spawn_tracked_entity_for_test(cow, false)
        .expect("cow joins the local session index");
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(6.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[]);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "tend-without-feed",
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

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).state, ScriptWorkState::Paused);
    assert_eq!(
        work_of(&outcome).reason,
        Some(ScriptWorkPauseReason::MissingInput)
    );
    assert_eq!(work_of(&outcome).work_units_done, 0);
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

/// (CP-011) A mature, compatible animal behind a closed route cannot consume
/// the resident's feed.
#[tokio::test]
async fn tend_livestock_with_no_route_preserves_feed() {
    let fixture = Fixture::new(true);
    let mut cow = SpawnEntity::new(0, "minecraft:cow", Vec3::new(6.5, 64.0, 6.5));
    cow.animal = Some(mc_entity::AnimalBreedingState::adult());
    fixture
        .sessions
        .spawn_tracked_entity_for_test(cow, false)
        .expect("cow joins the local session index");
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(12.5, 64.0, 6.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:wheat", 1)]);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "tend-blocked-cow",
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

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).state, ScriptWorkState::Paused);
    assert_eq!(
        work_of(&outcome).reason,
        Some(ScriptWorkPauseReason::BlockedRoute)
    );
    assert_eq!(work_of(&outcome).work_units_done, 0);
    assert!(
        carry(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:wheat" && *count == 1)
    );
}

/// (CP-011) A removed animal is not retained as an invisible livestock source.
#[tokio::test]
async fn dead_livestock_preserves_feed_and_work_progress() {
    let fixture = Fixture::new(false);
    let mut cow = SpawnEntity::new(0, "minecraft:cow", Vec3::new(6.5, 64.0, 6.5));
    cow.animal = Some(mc_entity::AnimalBreedingState::adult());
    let cow_id = fixture
        .sessions
        .spawn_tracked_entity_for_test(cow, false)
        .expect("cow joins the local session index");
    assert!(
        fixture.sessions.remove_tracked_entity_for_test(cow_id),
        "the test removes the live source before work begins"
    );
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(6.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:wheat", 1)]);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "tend-dead-cow",
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

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).state, ScriptWorkState::Paused);
    assert_eq!(
        work_of(&outcome).reason,
        Some(ScriptWorkPauseReason::MissingInput)
    );
    assert_eq!(work_of(&outcome).work_units_done, 0);
    assert!(
        carry(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:wheat" && *count == 1)
    );
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

/// (CP-012) A recipe uses its own stateless station capability: campfire food
/// does not run at a crafting table, and the mismatch preserves the real input.
#[tokio::test]
async fn craft_requires_the_recipe_matching_station_for_campfire_food() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:cod", 1)]);
    let work = ScriptResidentWorkOrder::Craft {
        recipe: "minecraft:cooked_cod_from_campfire_cooking".to_owned(),
        count: 1,
        station: crafting_station(),
    };

    let missing_station = fixture
        .execute(
            &mut storage,
            &work_request("cook-cod-wrong-station", &handle, work.clone(), 1, revision),
        )
        .await;
    let assignment = work_of(&missing_station);
    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(
        assignment.reason,
        Some(ScriptWorkPauseReason::MissingStation)
    );
    assert_eq!(assignment.work_units_done, 0);
    assert!(
        carry(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:cod" && *count == 1),
        "a table cannot consume campfire food"
    );

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
            state_with_props(
                &fixture.blocks,
                "campfire",
                &[
                    ("facing", "north"),
                    ("lit", "false"),
                    ("signal_fire", "false"),
                    ("waterlogged", "false"),
                ],
            ),
        )
        .expect("fixture station remains loaded");
    let unlit = fixture
        .execute(
            &mut storage,
            &work_request(
                "cook-cod-unlit-campfire",
                &handle,
                work.clone(),
                1,
                assignment.revision,
            ),
        )
        .await;
    let assignment = work_of(&unlit);
    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(
        assignment.reason,
        Some(ScriptWorkPauseReason::MissingStation)
    );
    assert_eq!(assignment.work_units_done, 0);

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
            state_with_props(
                &fixture.blocks,
                "campfire",
                &[
                    ("facing", "north"),
                    ("lit", "true"),
                    ("signal_fire", "false"),
                    ("waterlogged", "false"),
                ],
            ),
        )
        .expect("fixture station remains loaded");
    let cooked = fixture
        .execute(
            &mut storage,
            &work_request("cook-cod-campfire", &handle, work, 1, assignment.revision),
        )
        .await;
    let assignment = work_of(&cooked);
    assert_eq!(
        assignment.state,
        ScriptWorkState::Committed,
        "{assignment:?}"
    );
    assert_eq!(assignment.work_units_done, 1);
    assert_eq!(
        assignment
            .changes
            .iter()
            .map(|change| (change.item_id.as_str(), change.delta))
            .collect::<BTreeMap<_, _>>(),
        BTreeMap::from([("minecraft:cod", -1), ("minecraft:cooked_cod", 1)])
    );
}

/// (CP-012) A real container input returns its recipe remainder beside the
/// result; all output entries belong to the same durable craft receipt.
#[tokio::test]
async fn craft_returns_recipe_container_remainders_with_its_output() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[
            ("minecraft:milk_bucket", 1),
            ("minecraft:milk_bucket", 1),
            ("minecraft:milk_bucket", 1),
            ("minecraft:sugar", 2),
            ("minecraft:egg", 1),
            ("minecraft:wheat", 3),
        ],
    );

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "craft-cake-remainders",
                &handle,
                ScriptResidentWorkOrder::Craft {
                    recipe: "minecraft:cake".to_owned(),
                    count: 1,
                    station: crafting_station(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(
        assignment.state,
        ScriptWorkState::Committed,
        "{assignment:?}"
    );
    assert_eq!(assignment.work_units_done, 1);
    assert_eq!(
        assignment
            .changes
            .iter()
            .map(|change| (change.item_id.as_str(), change.delta))
            .collect::<BTreeMap<_, _>>(),
        BTreeMap::from([
            ("minecraft:bucket", 3),
            ("minecraft:cake", 1),
            ("minecraft:egg", -1),
            ("minecraft:milk_bucket", -3),
            ("minecraft:sugar", -2),
            ("minecraft:wheat", -3),
        ])
    );
    assert!(
        carry(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:bucket" && *count == 3),
        "the three milk buckets become three owned empty buckets"
    );
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

/// (CP-012) A failed output cannot simplify a component-bearing input during
/// rollback: the resident keeps its exact named stack and reports no delta.
#[tokio::test]
async fn craft_with_full_output_keeps_component_bearing_input_unchanged() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let carry_fill: Vec<(&str, u32)> = (0..MAX_RESIDENT_CARRY_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    let revision = seed_gear_in(&mut storage, &handle, uuid, &carry_fill, false);
    let equipment_fill: Vec<(&str, u32)> = (0..MAX_RESIDENT_EQUIPMENT_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    let revision = seed_gear_in(&mut storage, &handle, uuid, &equipment_fill, true).max(revision);
    let mut record = storage
        .resident_orders()
        .record(&handle)
        .cloned()
        .expect("seeded resident record");
    record.carry[0]
        .as_mut()
        .expect("first carry stack")
        .custom_name = Some("owned stone".to_owned());
    let revision = storage
        .append_resident_order_change(DurableResidentOrderChange::Record {
            record: Box::new(record),
        })
        .expect("named input is durable")
        .max(revision);
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
            state_of(&fixture.blocks, "stonecutter"),
        )
        .expect("fixture station remains loaded");

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "craft-named-stone-without-output-capacity",
                &handle,
                ScriptResidentWorkOrder::Craft {
                    recipe: "minecraft:stone_slab_from_stone_stonecutting".to_owned(),
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
    assert!(assignment.changes.is_empty());
    let input = storage
        .resident_orders()
        .record(&handle)
        .and_then(|record| record.carry[0].as_ref())
        .expect("named source stays in its original slot");
    assert_eq!(input.item_id, "minecraft:stone");
    assert_eq!(input.count, 64);
    assert_eq!(input.custom_name.as_deref(), Some("owned stone"));
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
/// slots. A closed passage or one unplaceable slot blocks the formation without
/// teleportation; clearing that slot and reissuing the order reforms the squad.
#[tokio::test]
async fn squad_reforms_through_an_open_passage_and_refuses_a_closed_one() {
    for (wall, blocked_slot) in [(false, false), (true, false), (false, true)] {
        let standable_queries = Arc::new(AtomicUsize::new(0));
        let route_queries = Arc::new(AtomicUsize::new(0));
        let fixture = Fixture::with_query_counts(
            wall,
            Arc::clone(&standable_queries),
            Arc::clone(&route_queries),
        );
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
        if blocked_slot {
            fixture
                .world
                .lock()
                .await
                .set_block_at(
                    BlockPos { x: 13, y: 64, z: 8 },
                    state_of(&fixture.blocks, "stone"),
                )
                .expect("block one formation slot");
        }
        standable_queries.store(0, Ordering::Relaxed);
        route_queries.store(0, Ordering::Relaxed);
        let handles = members
            .iter()
            .map(|(handle, _)| handle.clone())
            .collect::<Vec<_>>();
        let request = order_request(
            if wall {
                "move-closed"
            } else if blocked_slot {
                "move-blocked-slot"
            } else {
                "move-open"
            },
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
        let (first_revision, outcomes, _) = order_of(&outcome);
        assert_eq!(outcomes.len(), 3);
        let placement_queries = standable_queries.load(Ordering::Relaxed);
        let member_routes = route_queries.load(Ordering::Relaxed);
        if blocked_slot {
            assert!(
                placement_queries <= handles.len() * 2,
                "one bounded formation scan, not one scan per member"
            );
            assert_eq!(member_routes, 0, "unplaceable squad does not query paths");
        } else {
            assert_eq!(
                placement_queries,
                handles.len(),
                "one flat-ground placement query per squad member"
            );
            assert_eq!(member_routes, 3, "one route decision per member");
        }
        eprintln!(
            "squad wall={wall} blocked_slot={blocked_slot} size=3 standable_queries={placement_queries} route_queries={member_routes}"
        );
        let repeated = fixture.execute(&mut storage, &request).await;
        assert_eq!(members_revision(&repeated), first_revision);
        assert_eq!(standable_queries.load(Ordering::Relaxed), placement_queries);
        assert_eq!(route_queries.load(Ordering::Relaxed), member_routes);
        if wall || blocked_slot {
            assert!(
                outcomes
                    .iter()
                    .all(|member| member.state == ScriptOrderMemberState::BlockedRoute),
                "unreachable formation: {outcomes:?}"
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
            if blocked_slot {
                fixture
                    .world
                    .lock()
                    .await
                    .set_block_at(
                        BlockPos { x: 13, y: 64, z: 8 },
                        state_of(&fixture.blocks, "air"),
                    )
                    .expect("clear the obstructed slot");
                standable_queries.store(0, Ordering::Relaxed);
                route_queries.store(0, Ordering::Relaxed);
                let cleared = fixture
                    .execute(
                        &mut storage,
                        &order_request(
                            "move-cleared-slot",
                            &handles,
                            &[first_revision; 3],
                            ScriptResidentOrder::Move {
                                dimension: "minecraft:overworld".to_owned(),
                                anchor: ScriptBlockPosition::new(13, 64, 8),
                                heading_degrees: 0,
                                formation: formation(),
                            },
                        ),
                    )
                    .await;
                let (_, clear_members, _) = order_of(&cleared);
                assert!(
                    clear_members
                        .iter()
                        .all(|member| member.state == ScriptOrderMemberState::Applied)
                );
                assert_eq!(standable_queries.load(Ordering::Relaxed), 3);
                assert_eq!(route_queries.load(Ordering::Relaxed), 3);
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
            let records = handles
                .iter()
                .map(|handle| {
                    storage
                        .resident_orders()
                        .record(handle)
                        .cloned()
                        .expect("accepted member record")
                })
                .collect::<Vec<_>>();
            storage
                .force_pending_admission_for_test(OWNER, "reform-replay", &records)
                .expect("one group replay is durable");
            fixture
                .sessions
                .apply_resident_goals(
                    members
                        .iter()
                        .map(|(_, uuid)| ResidentGoal {
                            uuid: *uuid,
                            goal: mc_entity::GoalState::Idle,
                        })
                        .collect(),
                )
                .await;
            let mut reopened =
                PluginStorage::open(fixture.storage_root.path()).expect("reopen squad");
            fixture
                .runtime
                .recover_resident_orders(&mut reopened)
                .await
                .unwrap();
            for ((_, uuid), expected) in members.iter().zip(&goals) {
                assert_eq!(
                    fixture.goal(*uuid).await,
                    expected.clone(),
                    "replay preserves each member's distinct formation destination"
                );
            }
            let mut storage = reopened;
            standable_queries.store(0, Ordering::Relaxed);
            route_queries.store(0, Ordering::Relaxed);
            let moving_roster = handles[..2].to_vec();
            let second = fixture
                .execute(
                    &mut storage,
                    &order_request(
                        "move-smaller-roster",
                        &moving_roster,
                        &[first_revision; 2],
                        ScriptResidentOrder::Move {
                            dimension: "minecraft:overworld".to_owned(),
                            anchor: ScriptBlockPosition::new(12, 64, 11),
                            heading_degrees: 0,
                            formation: formation(),
                        },
                    ),
                )
                .await;
            let (second_revision, next_members, _) = order_of(&second);
            assert!(
                next_members
                    .iter()
                    .all(|member| member.state == ScriptOrderMemberState::Applied)
            );
            let reformed_queries = standable_queries.load(Ordering::Relaxed);
            assert_eq!(route_queries.load(Ordering::Relaxed), 2);
            assert_eq!(reformed_queries, moving_roster.len());
            eprintln!(
                "squad wall=false size=2 standable_queries={reformed_queries} route_queries=2"
            );
            assert_eq!(next_members.len(), 2);
            assert_ne!(
                fixture.goal(members[0].1).await,
                fixture.goal(members[1].1).await
            );
            assert_eq!(
                fixture.goal(members[2].1).await,
                goals[2],
                "a member omitted from a new roster retains its previous order"
            );
            let moving_goals = vec![
                ResidentGoal {
                    uuid: members[0].1,
                    goal: fixture.goal(members[0].1).await,
                },
                ResidentGoal {
                    uuid: members[1].1,
                    goal: fixture.goal(members[1].1).await,
                },
            ];
            let cancel = ScriptOperationRequest::try_new(
                "cancel-smaller-roster",
                ScriptOperation::ResidentOrder {
                    operation: ScriptResidentOrderOperation::CancelOrder {
                        operation_id: "cancel-smaller-roster".to_owned(),
                        handles: moving_roster,
                        expected_order_revisions: vec![second_revision; 2],
                    },
                },
            )
            .expect("fenced group cancellation");
            let cancelled = fixture.execute(&mut storage, &cancel).await;
            assert_eq!(cancelled.failure(), None);
            for (_, uuid) in members.iter().take(2) {
                assert_eq!(fixture.goal(*uuid).await, mc_entity::GoalState::Idle);
            }
            assert_eq!(fixture.goal(members[2].1).await, goals[2]);
            let cancelled_records = handles[..2]
                .iter()
                .map(|handle| {
                    storage
                        .resident_orders()
                        .record(handle)
                        .cloned()
                        .expect("cancelled member remains durable")
                })
                .collect::<Vec<_>>();
            assert!(
                cancelled_records
                    .iter()
                    .all(|record| record.order.is_none())
            );
            storage
                .force_pending_admission_for_test(OWNER, "cancel-replay", &cancelled_records)
                .expect("pending group cancellation is durable");
            fixture.sessions.apply_resident_goals(moving_goals).await;
            let mut reopened =
                PluginStorage::open(fixture.storage_root.path()).expect("reopen cancellation");
            fixture
                .runtime
                .recover_resident_orders(&mut reopened)
                .await
                .unwrap();
            assert!(reopened.resident_orders().pending_admissions().is_empty());
            for (_, uuid) in members.iter().take(2) {
                assert_eq!(
                    fixture.goal(*uuid).await,
                    mc_entity::GoalState::Idle,
                    "pending cancellation must replay Idle before acknowledgement"
                );
            }
            assert_eq!(fixture.goal(members[2].1).await, goals[2]);
        }
    }
}

/// (A10) An archer consumes one arrow only on launch; the shared projectile
/// kernel hits an authorized target later, while no ammo, a wall and ally policy
/// prevent launch or damage.
#[tokio::test]
async fn archer_ammo_line_of_sight_and_ally_policy_gate_committed_damage() {
    let fixture = Fixture::new(false);
    fixture.enable_arrows();
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
    let mut hostile_target = perceived
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("hostile reference")
        .clone();

    let attack_order = |targets: Vec<ScriptOrderTargetRef>, policy: ScriptEngagementPolicy| {
        ScriptResidentOrder::Attack { targets, policy }
    };
    let reference = |target: &mc_script::ScriptOrderTarget| {
        ScriptOrderTargetRef::new(
            target.target_ref.clone(),
            target.policy_revision,
            target.expires_revision,
        )
    };
    let policy_with_ally = {
        let mut policy = ScriptEngagementPolicy::new(
            0,
            vec![ally_handle.clone()],
            vec![
                ScriptHostileCategory::Hostile,
                ScriptHostileCategory::OwnedResident,
            ],
        );
        policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
        policy
    };
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
                attack_order(vec![reference(&hostile_target)], policy_with_ally.clone()),
            ),
        )
        .await;
    let (_, members, combat) = order_of(&outcome);
    assert!(combat.is_empty(), "no ammo means no committed damage");
    assert_eq!(
        fixture.health(zombie_uuid).await,
        health_before,
        "health is unchanged by a refused engagement"
    );
    hostile_target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("refreshed hostile target")
        .clone();

    // A crowded lane cannot stall the one-target arrow or make it hit a
    // bystander: the restricted shot resolves just its issued entity.
    let mut bystander = None;
    for ordinal in 0..12 {
        let cow = fixture
            .sessions
            .spawn_tracked_entity_for_test(
                SpawnEntity::new(
                    11,
                    "minecraft:cow",
                    Vec3::new(5.0 + f64::from(ordinal) * 0.08, 64.0, 4.5),
                ),
                false,
            )
            .expect("bystander spawns");
        bystander.get_or_insert(cow);
    }
    let bystander = bystander.expect("first bystander");
    let bystander_health = fixture
        .sessions
        .snapshot_entity_for_test(bystander)
        .expect("bystander is live")
        .health;
    // With arrows the resident launches a physical projectile; there is no
    // instant-damage receipt before its flight and impact.
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
                attack_order(vec![reference(&hostile_target)], policy_with_ally.clone()),
            ),
        )
        .await;
    let (_, members, combat) = order_of(&outcome);
    assert!(
        combat.is_empty(),
        "launch is not an impact receipt: {combat:?}"
    );
    assert_eq!(fixture.health(zombie_uuid).await, health_before);
    assert_eq!(
        fixture
            .sessions
            .resident_arrows_for_test(Vec3::new(4.5, 64.0, 4.5))
            .len(),
        1,
        "one physical arrow launched"
    );
    assert_eq!(
        fixture.advance_resident_arrows(1, Vec3::new(4.5, 64.0, 4.5)),
        1
    );
    assert!(
        fixture.health(zombie_uuid).await < health_before,
        "the projectile kernel hit the hostile"
    );
    assert_eq!(
        fixture
            .sessions
            .snapshot_entity_for_test(bystander)
            .expect("bystander remains live")
            .health,
        bystander_health,
        "the physical arrow ignored bystanders in its path"
    );
    assert_eq!(
        arrow_count(&storage, &archer_handle),
        7,
        "exactly one arrow was consumed"
    );
    let after_arrows = members_revision(&outcome);
    hostile_target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("refreshed hostile target")
        .clone();
    let health_after_first = fixture.health(zombie_uuid).await;
    let repeated = fixture
        .execute(
            &mut storage,
            &order_request(
                "attack-same-tick",
                &holders,
                &[after_arrows],
                attack_order(vec![reference(&hostile_target)], policy_with_ally.clone()),
            ),
        )
        .await;
    let (_, members, repeated_combat) = order_of(&repeated);
    assert!(
        repeated_combat.is_empty(),
        "same-tick repeated order cannot bypass attack cooldown"
    );
    assert_eq!(fixture.health(zombie_uuid).await, health_after_first);
    assert_eq!(arrow_count(&storage, &archer_handle), 7);
    let after_repeat = members_revision(&repeated);
    hostile_target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("refreshed hostile target")
        .clone();

    // The ally policy gate. The ally is a real target on the damage path: the
    // control volley, whose policy names no ally, hurts it through the same
    // issued reference; the same reference is refused once the policy covers
    // the ally.
    let neutral_policy = |allies: Vec<String>| {
        let mut policy =
            ScriptEngagementPolicy::new(1, allies, vec![ScriptHostileCategory::NeutralAnimal]);
        policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
        policy
    };
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "attack-observe-ally",
                &holders,
                &[after_repeat],
                attack_order(vec![reference(&hostile_target)], neutral_policy(Vec::new())),
            ),
        )
        .await;
    let (_, members, combat) = order_of(&outcome);
    assert!(
        combat.is_empty(),
        "a policy permitting neutral animals only never shoots the hostile: {combat:?}"
    );
    let mut ally_target = members[0]
        .targets
        .iter()
        .find(|target| target.position == ScriptBlockPosition::new(5, 64, 5))
        .expect("the adjacent resident is issued when no ally excludes it")
        .clone();

    let after_observe = members_revision(&outcome);
    let ally_health = fixture.health(ally_uuid).await;
    fixture.sessions.synchronize_entity_lifecycle_epoch(20);
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "attack-ally-control",
                &holders,
                &[after_observe],
                attack_order(vec![reference(&ally_target)], neutral_policy(Vec::new())),
            ),
        )
        .await;
    let (_, members, combat) = order_of(&outcome);
    assert!(
        combat.is_empty(),
        "the projectile has not landed: {combat:?}"
    );
    assert_eq!(fixture.health(ally_uuid).await, ally_health);
    assert_eq!(
        fixture.advance_resident_arrows(21, Vec3::new(4.5, 64.0, 4.5)),
        1
    );
    assert!(
        fixture.health(ally_uuid).await < ally_health,
        "the issued reference really addresses the resident"
    );
    let after_control = members_revision(&outcome);
    ally_target = members[0]
        .targets
        .iter()
        .find(|target| target.position == ScriptBlockPosition::new(5, 64, 5))
        .expect("the neutral resident target is refreshed")
        .clone();

    let ally_health = fixture.health(ally_uuid).await;
    let arrows_before = arrow_count(&storage, &archer_handle);
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "attack-ally-gated",
                &holders,
                &[after_control],
                attack_order(vec![reference(&ally_target)], {
                    let mut policy = ScriptEngagementPolicy::new(
                        1,
                        vec![ally_handle.clone()],
                        vec![
                            ScriptHostileCategory::Hostile,
                            ScriptHostileCategory::NeutralAnimal,
                        ],
                    );
                    policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
                    policy
                }),
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
    walled.enable_arrows();
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
    let walled_target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("the hostile is perceived through the wall")
        .clone();
    let walled_health = walled.health(walled_uuid).await;
    let walled_order_revision = members_revision(&outcome);
    let walled_order_revision = if walled_order_revision == 0 {
        1
    } else {
        walled_order_revision
    };
    let walled_policy = {
        let mut policy =
            ScriptEngagementPolicy::new(0, Vec::new(), vec![ScriptHostileCategory::Hostile]);
        policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
        policy
    };
    let outcome = walled
        .execute(
            &mut walled_storage,
            &order_request(
                "wall-attack",
                &walled_holders,
                &[walled_order_revision],
                attack_order(vec![reference(&walled_target)], walled_policy),
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

#[tokio::test]
async fn changing_equipped_bow_to_sword_uses_melee_without_spending_an_arrow() {
    let fixture = Fixture::new(false);
    fixture.enable_arrows();
    let mut storage = fixture.storage();
    let (handle, resident_uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    seed_gear_in(
        &mut storage,
        &handle,
        resident_uuid,
        &[("minecraft:bow", 1), ("minecraft:arrow", 4)],
        true,
    );
    let zombie_id = fixture
        .sessions
        .spawn_tracked_entity_for_test(
            SpawnEntity::new(54, "minecraft:zombie", Vec3::new(6.5, 64.0, 4.5)),
            true,
        )
        .expect("zombie is tracked");
    let zombie_uuid = fixture
        .sessions
        .snapshot_entity_for_test(zombie_id)
        .expect("zombie is live")
        .uuid;
    let handles = vec![handle.clone()];
    let revision = storage
        .resident_orders()
        .record(&handle)
        .unwrap()
        .order_revision();
    let hold = fixture
        .execute(
            &mut storage,
            &order_request(
                "weapon-hold",
                &handles,
                &[revision],
                ScriptResidentOrder::Hold {
                    anchor: ScriptBlockPosition::new(4, 64, 4),
                    heading_degrees: 0,
                    formation: formation(),
                    engagement_radius: 16,
                },
            ),
        )
        .await;
    let (_, members, _) = order_of(&hold);
    let target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("hold issued a hostile reference")
        .clone();
    let before = fixture.health(zombie_uuid).await;
    seed_gear_in(
        &mut storage,
        &handle,
        resident_uuid,
        &[("minecraft:iron_sword", 1)],
        true,
    );
    let revision = storage
        .resident_orders()
        .record(&handle)
        .unwrap()
        .order_revision();
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "weapon-sword-attack",
                &handles,
                &[revision],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(
                        target.target_ref,
                        target.policy_revision,
                        target.expires_revision,
                    )],
                    policy: {
                        let mut policy = ScriptEngagementPolicy::new(
                            0,
                            Vec::new(),
                            vec![ScriptHostileCategory::Hostile],
                        );
                        policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
                        policy
                    },
                },
            ),
        )
        .await;
    let (_, members, combat) = order_of(&outcome);
    assert_eq!(combat.len(), 1, "the new sword commits a melee hit");
    assert!(fixture.health(zombie_uuid).await < before);
    assert_eq!(arrow_count(&storage, &handle), 4);
    assert!(
        fixture
            .sessions
            .resident_arrows_for_test(Vec3::new(4.5, 64.0, 4.5))
            .is_empty(),
        "the previous bow is not used after the weapon changes"
    );
    let target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("live target reference refreshed")
        .clone();
    seed_gear_in(
        &mut storage,
        &handle,
        resident_uuid,
        &[("minecraft:bow", 1)],
        true,
    );
    assert!(fixture.sessions.remove_tracked_entity_for_test(zombie_id));
    fixture.sessions.synchronize_entity_lifecycle_epoch(20);
    let revision = storage
        .resident_orders()
        .record(&handle)
        .unwrap()
        .order_revision();
    let departed = fixture
        .execute(
            &mut storage,
            &order_request(
                "weapon-departed-target",
                &handles,
                &[revision],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(
                        target.target_ref,
                        target.policy_revision,
                        target.expires_revision,
                    )],
                    policy: {
                        let mut policy = ScriptEngagementPolicy::new(
                            0,
                            Vec::new(),
                            vec![ScriptHostileCategory::Hostile],
                        );
                        policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
                        policy
                    },
                },
            ),
        )
        .await;
    let (_, _, combat) = order_of(&departed);
    assert!(combat.is_empty(), "departed target cannot be hit");
    assert_eq!(arrow_count(&storage, &handle), 4);
    assert!(
        fixture
            .sessions
            .resident_arrows_for_test(Vec3::new(4.5, 64.0, 4.5))
            .is_empty(),
        "the old reference cannot launch a projectile after the target leaves"
    );
}

/// A permitted distant target is chased, then attacked by the native owner
/// once movement reaches melee range; a guest does not issue per-tick damage.
#[tokio::test]
async fn distant_guard_engages_after_chasing_without_a_second_plugin_order() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, resident_uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    let zombie_id = fixture
        .sessions
        .spawn_tracked_entity_for_test(
            SpawnEntity::new(54, "minecraft:zombie", Vec3::new(9.5, 64.0, 4.5)),
            true,
        )
        .expect("zombie spawns");
    let zombie_uuid = fixture
        .sessions
        .snapshot_entity_for_test(zombie_id)
        .expect("zombie is live")
        .uuid;
    let hold = fixture
        .execute(
            &mut storage,
            &order_request(
                "distant-hold",
                std::slice::from_ref(&handle),
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
    let (_, members, _) = order_of(&hold);
    let target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("guard perceives the hostile")
        .clone();
    let attack = fixture
        .execute(
            &mut storage,
            &order_request(
                "distant-attack",
                std::slice::from_ref(&handle),
                &[members_revision(&hold)],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(
                        target.target_ref,
                        target.policy_revision,
                        target.expires_revision,
                    )],
                    policy: {
                        let mut policy = ScriptEngagementPolicy::new(
                            0,
                            Vec::new(),
                            vec![ScriptHostileCategory::Hostile],
                        );
                        policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
                        policy
                    },
                },
            ),
        )
        .await;
    let attack_revision = members_revision(&attack);
    assert_eq!(
        order_of(&attack).1[0].state,
        ScriptOrderMemberState::Applied,
        "accepted combat must not report a blocked formation route"
    );
    assert!(
        order_of(&attack).2.is_empty(),
        "out-of-reach order deals no damage"
    );
    let health = fixture.health(zombie_uuid).await;
    assert!(matches!(
        fixture.goal(resident_uuid).await,
        mc_entity::GoalState::FollowTarget { .. }
    ));
    let resident = fixture
        .sessions
        .resident_entity_snapshots(&[resident_uuid])
        .await
        .remove(0)
        .expect("guard stays live");
    fixture.sessions.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: resident.id,
            position: Vec3::new(7.5, 64.0, 4.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
    );
    assert!(fixture.position(resident_uuid).await.x >= 7.5);
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 1)
        .await
        .expect("native combat tick");
    let damaged = fixture.health(zombie_uuid).await;
    assert!(damaged < health, "native follow-up hits after approaching");
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 1)
        .await
        .expect("duplicate tick");
    assert_eq!(
        fixture.health(zombie_uuid).await,
        damaged,
        "cooldown gates repeat"
    );
    assert_eq!(
        storage
            .resident_orders()
            .record(&handle)
            .expect("guard record")
            .order_revision(),
        attack_revision,
        "native follow-up does not invalidate the guest's order fence"
    );
    let mut reopened = PluginStorage::open(fixture.storage_root.path()).expect("reopen combat");
    fixture
        .runtime
        .continue_active_resident_combat(&mut reopened, 21)
        .await
        .expect("cooldown elapsed after reopen");
    assert!(
        fixture.health(zombie_uuid).await < damaged,
        "the accepted chase and ammo/cooldown state survive reopening"
    );
    assert_eq!(
        reopened
            .resident_orders()
            .record(&handle)
            .expect("reopened guard")
            .order_revision(),
        attack_revision
    );
    let cancel = ScriptOperationRequest::try_new(
        "cancel",
        ScriptOperation::ResidentOrder {
            operation: ScriptResidentOrderOperation::CancelOrder {
                operation_id: "distant-cancel".to_owned(),
                handles: vec![handle],
                expected_order_revisions: vec![attack_revision],
            },
        },
    )
    .expect("valid cancellation");
    let outcome = fixture.execute(&mut reopened, &cancel).await;
    assert_eq!(outcome.failure(), None, "accepted cancel: {outcome:?}");
    let health = fixture.health(zombie_uuid).await;
    fixture
        .runtime
        .continue_active_resident_combat(&mut reopened, 41)
        .await
        .expect("cancelled order no longer runs");
    assert_eq!(fixture.health(zombie_uuid).await, health);
}

/// A standing archer order spends its own canonical arrows on subsequent
/// native cooldown-ready shots, and stops launching when the stack runs out.
#[tokio::test]
async fn native_archer_follow_up_spends_each_arrow_once() {
    let fixture = Fixture::new(false);
    fixture.enable_arrows();
    let mut storage = fixture.storage();
    let (handle, archer) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    let zombie_id = fixture
        .sessions
        .spawn_tracked_entity_for_test(
            SpawnEntity::new(54, "minecraft:zombie", Vec3::new(10.5, 64.0, 4.5)),
            true,
        )
        .expect("zombie spawns");
    seed_gear(&mut storage, &handle, archer, &[("minecraft:bow", 1)]);
    let bow_revision = equip_bow(&mut storage, &handle);
    let hold = fixture
        .execute(
            &mut storage,
            &order_request(
                "native-bow-hold",
                std::slice::from_ref(&handle),
                &[bow_revision],
                ScriptResidentOrder::Hold {
                    anchor: ScriptBlockPosition::new(4, 64, 4),
                    heading_degrees: 0,
                    formation: formation(),
                    engagement_radius: 16,
                },
            ),
        )
        .await;
    let (_, members, _) = order_of(&hold);
    let target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("hostile reference");
    seed_gear(&mut storage, &handle, archer, &[("minecraft:arrow", 2)]);
    let revision = storage
        .resident_orders()
        .record(&handle)
        .expect("archer record")
        .order_revision();
    let attack = fixture
        .execute(
            &mut storage,
            &order_request(
                "native-bow-attack",
                std::slice::from_ref(&handle),
                &[revision],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(
                        target.target_ref.clone(),
                        target.policy_revision,
                        target.expires_revision,
                    )],
                    policy: {
                        let mut policy = ScriptEngagementPolicy::new(
                            0,
                            Vec::new(),
                            vec![ScriptHostileCategory::Hostile],
                        );
                        policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
                        policy
                    },
                },
            ),
        )
        .await;
    let (_, _, combat) = order_of(&attack);
    assert!(combat.is_empty(), "launch is not impact");
    assert_eq!(arrow_count(&storage, &handle), 1);
    assert_eq!(
        fixture
            .sessions
            .resident_arrows_for_test(Vec3::new(4.5, 64.0, 4.5))
            .len(),
        1
    );
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 20)
        .await
        .expect("next cooldown shot");
    assert_eq!(arrow_count(&storage, &handle), 0);
    assert_eq!(
        fixture
            .sessions
            .resident_arrows_for_test(Vec3::new(4.5, 64.0, 4.5))
            .len(),
        2
    );
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 40)
        .await
        .expect("empty quiver does not launch");
    assert_eq!(
        fixture
            .sessions
            .resident_arrows_for_test(Vec3::new(4.5, 64.0, 4.5))
            .len(),
        2,
        "no ammunition yields no third world arrow"
    );
    assert!(
        fixture
            .sessions
            .snapshot_entity_for_test(zombie_id)
            .is_some(),
        "the target remains live until the shared arrow kernel resolves impact"
    );
}

/// A retreat whose rally cannot be placed must still stop the interrupted
/// chase and keep reporting the blocked route, instead of leaving the old
/// attack goal installed.
#[tokio::test]
async fn retreat_with_an_unplaceable_rally_stops_the_chase_and_reports_blocked() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    let handles = vec![handle.clone()];
    let zombie_id = fixture
        .sessions
        .spawn_tracked_entity_for_test(
            SpawnEntity::new(54, "minecraft:zombie", Vec3::new(6.5, 64.0, 5.5)),
            true,
        )
        .expect("zombie spawns");
    let zombie_uuid = fixture
        .sessions
        .snapshot_entity_for_test(zombie_id)
        .expect("zombie is live")
        .uuid;

    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "blocked-retreat-1",
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
    let hostile_target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("hostile perceived")
        .clone();
    let order_revision = members_revision(&outcome);
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "blocked-retreat-2",
                &handles,
                &[order_revision],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(
                        hostile_target.target_ref.clone(),
                        hostile_target.policy_revision,
                        hostile_target.expires_revision,
                    )],
                    policy: {
                        let mut policy = ScriptEngagementPolicy::new(
                            0,
                            Vec::new(),
                            vec![ScriptHostileCategory::Hostile],
                        );
                        policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
                        policy
                    },
                },
            ),
        )
        .await;
    let attack_revision = members_revision(&outcome);
    assert!(
        matches!(
            fixture.goal(uuid).await,
            mc_entity::GoalState::FollowTarget { .. }
        ),
        "attack chases the hostile"
    );
    // The fixture's bounded tree occupies this cell, so its formation slot
    // cannot be placed.
    assert_eq!(
        fixture.block(6, SURFACE_Y + 1, 6).await,
        Some(state_of(&fixture.blocks, "oak_log")),
        "the blocked rally cell is occupied"
    );

    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "blocked-retreat-3",
                &handles,
                &[attack_revision],
                ScriptResidentOrder::Retreat {
                    anchor: ScriptBlockPosition::new(6, SURFACE_Y + 1, 6),
                    formation: formation(),
                },
            ),
        )
        .await;
    let (_, members, _) = order_of(&outcome);
    assert_eq!(
        members[0].state,
        ScriptOrderMemberState::BlockedRoute,
        "an unplaceable rally reports blocked_route"
    );
    assert_eq!(
        fixture.goal(uuid).await,
        mc_entity::GoalState::Idle,
        "the blocked retreat stops the interrupted chase"
    );
    let health_after_retreat = fixture.health(zombie_uuid).await;
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 100)
        .await
        .expect("retreat leaves no attack continuation");
    assert_eq!(
        fixture.health(zombie_uuid).await,
        health_after_retreat,
        "the prior chase cannot land another hit after a blocked retreat"
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
    let zombie_id = fixture
        .sessions
        .spawn_tracked_entity_for_test(
            SpawnEntity::new(54, "minecraft:zombie", Vec3::new(6.5, 64.0, 4.5)),
            true,
        )
        .expect("zombie spawns");
    let zombie_uuid = fixture
        .sessions
        .snapshot_entity_for_test(zombie_id)
        .expect("zombie is live")
        .uuid;

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
    let hostile_target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("hostile perceived")
        .clone();
    let order_revision = members_revision(&outcome);
    let hostile_health = fixture.health(zombie_uuid).await;

    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "stage-2",
                &handles,
                &[order_revision],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(
                        hostile_target.target_ref.clone(),
                        hostile_target.policy_revision,
                        hostile_target.expires_revision,
                    )],
                    policy: {
                        let mut policy = ScriptEngagementPolicy::new(
                            0,
                            Vec::new(),
                            vec![ScriptHostileCategory::Hostile],
                        );
                        policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
                        policy
                    },
                },
            ),
        )
        .await;
    let (_, _, combat) = order_of(&outcome);
    assert_eq!(
        combat.len(),
        1,
        "the melee action committed exactly one hit"
    );
    assert_eq!(combat[0].attacker_handle, handle);
    assert!(combat[0].damage_milli > 0);
    assert!(fixture.health(zombie_uuid).await < hostile_health);
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
    let health_after_retreat = fixture.health(zombie_uuid).await;
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 100)
        .await
        .expect("retreat leaves no attack continuation");
    assert_eq!(
        fixture.health(zombie_uuid).await,
        health_after_retreat,
        "the prior chase cannot land another hit after retreat"
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
    let (_, members, _) = order_of(&outcome);
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
    let hostile_target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("patrol refreshed hostile reference")
        .clone();

    // A threat interrupts the route: the member chases instead of walking.
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "stage-6",
                &handles,
                &[advanced_revision],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(
                        hostile_target.target_ref,
                        hostile_target.policy_revision,
                        hostile_target.expires_revision,
                    )],
                    policy: {
                        let mut policy = ScriptEngagementPolicy::new(
                            0,
                            Vec::new(),
                            vec![ScriptHostileCategory::Hostile],
                        );
                        policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
                        policy
                    },
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

#[tokio::test]
async fn injury_and_flank_route_then_capture_the_same_resident() {
    use crate::play::ResidentAttack;
    use crate::script::storage::resident_morale::MoralePhase;

    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    seed_gear(&mut storage, &handle, uuid, &[("minecraft:emerald", 1)]);
    let zombie_id = fixture
        .sessions
        .spawn_tracked_entity_for_test(
            SpawnEntity::new(54, "minecraft:zombie", Vec3::new(6.5, 64.0, 4.5)),
            true,
        )
        .expect("hostile spawns");
    let zombie_uuid = fixture
        .sessions
        .snapshot_entity_for_test(zombie_id)
        .expect("hostile exists")
        .uuid;
    let hold = fixture
        .execute(
            &mut storage,
            &order_request(
                "morale-hold",
                std::slice::from_ref(&handle),
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
    let target = order_of(&hold).1[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("hostile is perceived")
        .clone();
    let attack = fixture
        .execute(
            &mut storage,
            &order_request(
                "morale-attack",
                std::slice::from_ref(&handle),
                &[members_revision(&hold)],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(
                        target.target_ref,
                        target.policy_revision,
                        target.expires_revision,
                    )],
                    policy: {
                        let mut policy = ScriptEngagementPolicy::new(
                            0,
                            Vec::new(),
                            vec![ScriptHostileCategory::Hostile],
                        );
                        policy.rally = Some(ScriptBlockPosition::new(1, 64, 4));
                        policy
                    },
                },
            ),
        )
        .await;
    assert_eq!(attack.failure(), None);
    let public_order_revision = members_revision(&attack);
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 100)
        .await
        .unwrap();
    assert_eq!(
        storage
            .resident_orders()
            .record(&handle)
            .unwrap()
            .morale
            .as_ref()
            .unwrap()
            .phase,
        MoralePhase::Shaken,
        "the hostile is physically on the resident's flank"
    );
    for (tick, expected) in [(101, MoralePhase::Wavering), (102, MoralePhase::Routing)] {
        let snapshot = fixture.sessions.resident_entity_snapshots(&[uuid]).await[0]
            .clone()
            .expect("resident remains native and alive");
        let hit = fixture
            .sessions
            .commit_resident_damage(
                OWNER,
                vec![ResidentAttack {
                    uuid,
                    amount: 1.0,
                    expected: snapshot,
                }],
            )
            .await;
        assert!(
            hit[0].is_some(),
            "each injury must be a committed native hit"
        );
        fixture
            .runtime
            .continue_active_resident_combat(&mut storage, tick)
            .await
            .unwrap();
        assert_eq!(
            storage
                .resident_orders()
                .record(&handle)
                .unwrap()
                .morale
                .as_ref()
                .unwrap()
                .phase,
            expected
        );
    }
    assert_eq!(fixture.position(uuid).await, Vec3::new(4.5, 64.0, 4.5));
    assert!(matches!(
        fixture.goal(uuid).await,
        mc_entity::GoalState::FollowPosition { target, .. }
            if target == Vec3::new(1.5, 64.0, 4.5)
    ));
    let hostile_health = fixture.health(zombie_uuid).await;
    let mut reopened = PluginStorage::open(fixture.storage_root.path()).unwrap();
    fixture
        .runtime
        .recover_resident_orders(&mut reopened)
        .await
        .unwrap();
    fixture
        .runtime
        .continue_active_resident_combat(&mut reopened, 200)
        .await
        .unwrap();
    assert_eq!(fixture.health(zombie_uuid).await, hostile_health);
    assert_eq!(
        reopened
            .resident_orders()
            .record(&handle)
            .unwrap()
            .morale
            .as_ref()
            .unwrap()
            .phase,
        MoralePhase::Routing,
        "reload cannot silently restore the interrupted chase"
    );
    let (distant_guard, _) = fixture
        .resident(&mut reopened, 4, Vec3::new(11.5, 64.0, 4.5))
        .await;
    let distant_hold = fixture
        .execute(
            &mut reopened,
            &order_request(
                "distant-guard-hold",
                std::slice::from_ref(&distant_guard),
                &[0],
                ScriptResidentOrder::Hold {
                    anchor: ScriptBlockPosition::new(11, 64, 4),
                    heading_degrees: 0,
                    formation: formation(),
                    engagement_radius: 16,
                },
            ),
        )
        .await;
    assert_eq!(distant_hold.failure(), None);
    let revision = public_order_revision;
    let internal_revision = reopened.resident_orders().record(&handle).unwrap().revision;
    assert!(
        internal_revision > revision,
        "morale and gear progress must not fence off the public attack order"
    );
    let distant = fixture
        .execute(
            &mut reopened,
            &capture_request("capture-too-far", &handle, &distant_guard, revision),
        )
        .await;
    assert_eq!(
        distant.failure(),
        Some(ScriptOperationFailure::InvalidRequest)
    );
    assert_eq!(
        reopened.resident_orders().record(&handle).unwrap().revision,
        internal_revision
    );

    let (converted_guard, converted_uuid) = fixture
        .resident(&mut reopened, 6, Vec3::new(5.5, 64.0, 4.5))
        .await;
    let converted_hold = fixture
        .execute(
            &mut reopened,
            &order_request(
                "converted-guard-hold",
                std::slice::from_ref(&converted_guard),
                &[0],
                ScriptResidentOrder::Hold {
                    anchor: ScriptBlockPosition::new(5, 64, 4),
                    heading_degrees: 0,
                    formation: formation(),
                    engagement_radius: 16,
                },
            ),
        )
        .await;
    assert_eq!(converted_hold.failure(), None);
    let converted_id = fixture
        .sessions
        .resident_entity_snapshots(&[converted_uuid])
        .await[0]
        .as_ref()
        .expect("guard is originally a villager")
        .id;
    assert!(
        fixture
            .sessions
            .convert_resident_entity_for_test(converted_id)
    );
    let converted_capture = fixture
        .execute(
            &mut reopened,
            &capture_request(
                "capture-converted-guard",
                &handle,
                &converted_guard,
                revision,
            ),
        )
        .await;
    assert_eq!(
        converted_capture.failure(),
        Some(ScriptOperationFailure::NotFound),
        "a former villager converted to a zombie cannot claim custody"
    );

    let (guard, _) = fixture
        .resident(&mut reopened, 5, Vec3::new(5.5, 64.0, 5.5))
        .await;
    let nearby_hold = fixture
        .execute(
            &mut reopened,
            &order_request(
                "nearby-guard-hold",
                std::slice::from_ref(&guard),
                &[0],
                ScriptResidentOrder::Hold {
                    anchor: ScriptBlockPosition::new(5, 64, 5),
                    heading_degrees: 0,
                    formation: formation(),
                    engagement_radius: 16,
                },
            ),
        )
        .await;
    assert_eq!(nearby_hold.failure(), None);
    let before_gear = gear(&reopened, &handle);
    let capture = capture_request("capture-nearby", &handle, &guard, revision);
    let captured = fixture.execute(&mut reopened, &capture).await;
    let ScriptOperationPayload::ResidentOrder { result } = captured.payload() else {
        panic!("capture must return the original identity");
    };
    assert!(matches!(&**result,
        ScriptResidentOrderResult::Captured { handle: victim, custodian, .. }
            if victim == &handle && custodian == &guard
    ));
    let prisoner = reopened.resident_orders().record(&handle).unwrap();
    assert_eq!(
        prisoner.assignment,
        crate::script::storage::resident_orders::DurableAssignment::Prisoner
    );
    assert_eq!(prisoner.entity_uuid, uuid.to_string());
    assert_eq!(prisoner.custodian.as_deref(), Some(guard.as_str()));
    assert_eq!(
        prisoner.morale.as_ref().unwrap().phase,
        MoralePhase::Surrendered
    );
    assert_eq!(gear(&reopened, &handle), before_gear);
    assert_eq!(fixture.goal(uuid).await, mc_entity::GoalState::Idle);
    assert_eq!(fixture.position(uuid).await, Vec3::new(4.5, 64.0, 4.5));
    let repeat = fixture.execute(&mut reopened, &capture).await;
    assert_eq!(
        repeat, captured,
        "same native decision is replayed exactly once"
    );
    let mut restored = PluginStorage::open(fixture.storage_root.path()).unwrap();
    fixture
        .runtime
        .recover_resident_orders(&mut restored)
        .await
        .unwrap();
    assert_eq!(
        restored
            .resident_orders()
            .record(&handle)
            .unwrap()
            .entity_uuid,
        uuid.to_string()
    );
    assert_eq!(gear(&restored, &handle), before_gear);
    assert_eq!(
        fixture
            .execute(
                &mut restored,
                &demobilize_request("prisoner-demob", &handle, captured.revision().unwrap()),
            )
            .await
            .failure(),
        Some(ScriptOperationFailure::NotFound),
        "a captive cannot be enrolled or demobilized as a second army member"
    );
}

/// (CP-047 / WAR-07) A real ally death and a real officer death each advance one
/// morale category past the physical flank, and the routed squad receives its
/// rally as the native goal instead of the interrupted chase.
#[tokio::test]
async fn physical_ally_and_officer_losses_route_the_squad_to_its_rally() {
    use crate::play::ResidentAttack;
    use crate::script::storage::resident_morale::MoralePhase;

    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (guard, guard_uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    let (ally, ally_uuid) = fixture
        .resident(&mut storage, 4, Vec3::new(5.5, 64.0, 4.5))
        .await;
    let (officer, officer_uuid) = fixture
        .resident(&mut storage, 5, Vec3::new(5.5, 64.0, 5.5))
        .await;
    for (operation, member) in [("ally-post", &ally), ("officer-post", &officer)] {
        let post = fixture
            .execute(
                &mut storage,
                &order_request(
                    operation,
                    std::slice::from_ref(member),
                    &[0],
                    ScriptResidentOrder::Hold {
                        anchor: ScriptBlockPosition::new(5, 64, 5),
                        heading_degrees: 0,
                        formation: formation(),
                        engagement_radius: 16,
                    },
                ),
            )
            .await;
        assert_eq!(post.failure(), None, "{operation}: {post:?}");
    }
    fixture
        .sessions
        .spawn_tracked_entity_for_test(
            SpawnEntity::new(54, "minecraft:zombie", Vec3::new(6.5, 64.0, 4.5)),
            true,
        )
        .expect("hostile spawns");
    let hold = fixture
        .execute(
            &mut storage,
            &order_request(
                "loss-hold",
                std::slice::from_ref(&guard),
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
    let target = order_of(&hold).1[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("hostile is perceived")
        .clone();
    let attack = fixture
        .execute(
            &mut storage,
            &order_request(
                "loss-attack",
                std::slice::from_ref(&guard),
                &[members_revision(&hold)],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(
                        target.target_ref,
                        target.policy_revision,
                        target.expires_revision,
                    )],
                    policy: {
                        let mut policy = ScriptEngagementPolicy::new(
                            0,
                            vec![guard.clone(), ally.clone(), officer.clone()],
                            vec![ScriptHostileCategory::Hostile],
                        );
                        policy.officer = Some(officer.clone());
                        policy.rally = Some(ScriptBlockPosition::new(1, 64, 4));
                        policy
                    },
                },
            ),
        )
        .await;
    assert_eq!(attack.failure(), None, "{attack:?}");
    let phase = |storage: &PluginStorage| {
        storage
            .resident_orders()
            .record(&guard)
            .expect("the guard keeps its military record")
            .morale
            .as_ref()
            .expect("the attack order carries morale")
            .phase
    };
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 100)
        .await
        .unwrap();
    assert_eq!(
        phase(&storage),
        MoralePhase::Shaken,
        "the hostile stands on the guard's flank"
    );

    let expected = fixture
        .sessions
        .resident_entity_snapshots(&[ally_uuid])
        .await[0]
        .clone()
        .expect("the ally is native and alive");
    let hit = fixture
        .sessions
        .commit_resident_damage(
            OWNER,
            vec![ResidentAttack {
                uuid: ally_uuid,
                amount: 1_000.0,
                expected,
            }],
        )
        .await;
    assert!(
        hit[0].as_ref().is_some_and(|hit| hit.killed),
        "the ally must really die: {hit:?}"
    );
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 101)
        .await
        .unwrap();
    assert_eq!(
        phase(&storage),
        MoralePhase::Wavering,
        "one perceived ally loss is one category"
    );

    let expected = fixture
        .sessions
        .resident_entity_snapshots(&[officer_uuid])
        .await[0]
        .clone()
        .expect("the officer is native and alive");
    let hit = fixture
        .sessions
        .commit_resident_damage(
            OWNER,
            vec![ResidentAttack {
                uuid: officer_uuid,
                amount: 1_000.0,
                expected,
            }],
        )
        .await;
    assert!(
        hit[0].as_ref().is_some_and(|hit| hit.killed),
        "the officer must really die: {hit:?}"
    );
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 102)
        .await
        .unwrap();
    assert_eq!(
        phase(&storage),
        MoralePhase::Routing,
        "the officer's real death ends morale"
    );
    assert!(
        matches!(
            fixture.goal(guard_uuid).await,
            mc_entity::GoalState::FollowPosition { target, .. }
                if target == Vec3::new(1.5, 64.0, 4.5)
        ),
        "routing replaces the chase with the squad's rally"
    );
    let reopened = PluginStorage::open(fixture.storage_root.path()).expect("reopen morale");
    assert_eq!(
        reopened
            .resident_orders()
            .record(&guard)
            .unwrap()
            .morale
            .as_ref()
            .unwrap()
            .phase,
        MoralePhase::Routing,
        "the routed squad survives a restart"
    );
}

/// (CP-047 / WAR-07) A routed squad that physically walks to its rally with its
/// officer attending is restored: arrival, not elapsed time, ends the rout.
#[tokio::test]
async fn physical_rally_arrival_restores_the_routed_squad() {
    use crate::play::ResidentAttack;
    use crate::script::storage::resident_morale::MoralePhase;
    use mc_physics::{BlockMaterial, BlockMaterialIds, BlockSampler, EntityBody, PhysicsConfig};

    struct Terrain<'a> {
        chunks: &'a mc_world::WorldReadSnapshot,
        materials: &'a BlockMaterialIds,
    }
    impl BlockSampler for Terrain<'_> {
        fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
            self.chunks
                .get_cached_block(BlockPos { x, y, z })
                .map_or(BlockMaterial::Air, |state| self.materials.classify(state.0))
        }
    }
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (guard, guard_uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    let (officer, _) = fixture
        .resident(&mut storage, 5, Vec3::new(5.5, 64.0, 5.5))
        .await;
    let post = fixture
        .execute(
            &mut storage,
            &order_request(
                "rally-officer-post",
                std::slice::from_ref(&officer),
                &[0],
                ScriptResidentOrder::Hold {
                    anchor: ScriptBlockPosition::new(5, 64, 5),
                    heading_degrees: 0,
                    formation: formation(),
                    engagement_radius: 16,
                },
            ),
        )
        .await;
    assert_eq!(post.failure(), None, "{post:?}");
    fixture
        .sessions
        .spawn_tracked_entity_for_test(
            SpawnEntity::new(54, "minecraft:zombie", Vec3::new(6.5, 64.0, 4.5)),
            true,
        )
        .expect("hostile spawns");
    let hold = fixture
        .execute(
            &mut storage,
            &order_request(
                "rally-hold",
                std::slice::from_ref(&guard),
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
    let target = order_of(&hold).1[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("hostile is perceived")
        .clone();
    let attack = fixture
        .execute(
            &mut storage,
            &order_request(
                "rally-attack",
                std::slice::from_ref(&guard),
                &[members_revision(&hold)],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(
                        target.target_ref,
                        target.policy_revision,
                        target.expires_revision,
                    )],
                    policy: {
                        let mut policy = ScriptEngagementPolicy::new(
                            0,
                            vec![guard.clone(), officer.clone()],
                            vec![ScriptHostileCategory::Hostile],
                        );
                        policy.officer = Some(officer.clone());
                        policy.rally = Some(ScriptBlockPosition::new(1, 64, 4));
                        policy
                    },
                },
            ),
        )
        .await;
    assert_eq!(attack.failure(), None, "{attack:?}");
    let phase = |storage: &PluginStorage| {
        storage
            .resident_orders()
            .record(&guard)
            .expect("the guard keeps its military record")
            .morale
            .as_ref()
            .expect("the attack order carries morale")
            .phase
    };
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 100)
        .await
        .unwrap();
    assert_eq!(phase(&storage), MoralePhase::Shaken);
    let mut before_routing = None;
    for tick in [101, 102] {
        let expected = fixture
            .sessions
            .resident_entity_snapshots(&[guard_uuid])
            .await[0]
            .clone()
            .expect("the guard stays native and alive");
        let hit = fixture
            .sessions
            .commit_resident_damage(
                OWNER,
                vec![ResidentAttack {
                    uuid: guard_uuid,
                    amount: 1.0,
                    expected,
                }],
            )
            .await;
        assert!(hit[0].is_some(), "each injury is a committed native hit");
        if tick == 102 {
            before_routing = Some(fixture.position(guard_uuid).await);
        }
        fixture
            .runtime
            .continue_active_resident_combat(&mut storage, tick)
            .await
            .unwrap();
    }
    assert_eq!(phase(&storage), MoralePhase::Routing);
    assert!(
        matches!(
            fixture.goal(guard_uuid).await,
            mc_entity::GoalState::FollowPosition { target, .. }
                if target == Vec3::new(1.5, 64.0, 4.5)
        ),
        "the rout walks the squad to its rally instead of chasing"
    );
    // Routing replaces the goal; it must not move the guard itself, and the
    // guard must still start outside the rally's two-block arrival radius.
    let before_routing = before_routing.expect("the routing tick captured the guard's position");
    let routed_at = fixture.position(guard_uuid).await;
    assert!(
        (routed_at.x - before_routing.x).abs() < 0.001
            && (routed_at.z - before_routing.z).abs() < 0.001,
        "routing must not move the guard: {before_routing:?} -> {routed_at:?}"
    );
    assert!(
        (routed_at.x - 1.5).powi(2) + (routed_at.z - 4.5).powi(2) > 4.0,
        "the guard must walk to its rally instead of starting inside the arrival radius: {routed_at:?}"
    );

    // The squad reaches the rally by its own native walking goal.
    fixture
        .sessions
        .register_loaded_for_server_test("RallyObserver", (0, 0));
    let world_read = fixture.world.lock().await.read_view();
    let materials = BlockMaterialIds::new(
        state_of(&fixture.blocks, "air").0,
        Some(state_of(&fixture.blocks, "water").0),
        None,
    );
    let chunks = world_read.snapshot_chunks(&[ChunkPos { x: 0, z: 0 }]);
    let terrain = Terrain {
        chunks: &chunks,
        materials: &materials,
    };
    let guard_id = fixture
        .sessions
        .resident_entity_snapshots(&[guard_uuid])
        .await[0]
        .as_ref()
        .expect("living guard")
        .id;
    let mut walked_from = routed_at;
    for tick in 200..=400 {
        let queries = fixture
            .sessions
            .tick_entities_and_collect_physics_queries_with_terrain(tick, &world_read, &materials);
        let steps = queries
            .iter()
            .filter(|query| query.id == guard_id)
            .map(|query| {
                let stepped = mc_physics::step_entity(
                    EntityBody {
                        position: mc_physics::Vec3::new(
                            query.position.x,
                            query.position.y,
                            query.position.z,
                        ),
                        velocity: mc_physics::Vec3::new(
                            query.velocity.x,
                            query.velocity.y,
                            query.velocity.z,
                        ),
                        aabb: query.aabb,
                        on_ground: query.on_ground,
                    },
                    &terrain,
                    PhysicsConfig::living_entity(),
                );
                EntityPhysicsStep {
                    id: query.id,
                    position: Vec3::new(
                        stepped.body.position.x,
                        stepped.body.position.y,
                        stepped.body.position.z,
                    ),
                    velocity: Vec3::new(
                        stepped.body.velocity.x,
                        stepped.body.velocity.y,
                        stepped.body.velocity.z,
                    ),
                    on_ground: stepped.body.on_ground,
                    horizontal_collision: stepped.horizontal_collision,
                }
            })
            .collect::<Vec<_>>();
        fixture
            .sessions
            .apply_entity_physics_if_current_and_dispatch(tick, &queries, &steps);
        let at = fixture.position(guard_uuid).await;
        let step = ((at.x - walked_from.x).powi(2) + (at.z - walked_from.z).powi(2)).sqrt();
        assert!(
            step < 1.0,
            "one native physics tick must not teleport the guard: {walked_from:?} -> {at:?}"
        );
        walked_from = at;
        if (at.x - 1.5).abs() < 0.5 && (at.z - 4.5).abs() < 0.5 {
            break;
        }
    }
    let arrived = fixture.position(guard_uuid).await;
    assert!(
        (arrived.x - 1.5).abs() < 0.5 && (arrived.z - 4.5).abs() < 0.5,
        "the routed guard must walk to its rally, not teleport: {arrived:?}"
    );
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 401)
        .await
        .unwrap();
    assert_eq!(
        phase(&storage),
        MoralePhase::Rallied,
        "the officer at the rally restores the squad"
    );
    assert_eq!(fixture.goal(guard_uuid).await, mc_entity::GoalState::Idle);
    let reopened = PluginStorage::open(fixture.storage_root.path()).expect("reopen morale");
    let record = reopened.resident_orders().record(&guard).unwrap().clone();
    let morale = record.morale.as_ref().expect("terminal morale");
    assert_eq!(morale.phase, MoralePhase::Rallied);
    assert!(
        morale.goal_applied,
        "the restored squad's goal is durably applied"
    );
}

/// (CP-047) A resident that dies while demobilising publishes the loot its
/// configured mob table declares, keeps exactly one owner of its issued gear
/// (the durable record) and cannot finish the demobilisation, before or after a
/// restart. No corpse loot can carry the same gear.
#[tokio::test]
async fn a_demobilising_resident_killed_outright_keeps_one_gear_owner_and_configured_loot() {
    use crate::play::ResidentAttack;
    use mc_data::items::{ItemRegistry, ItemReport};
    use mc_data::loot::{LootDrop, LootTables};

    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(5.5, 64.0, 4.5))
        .await;
    seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:iron_sword", 1), ("minecraft:arrow", 8)],
    );
    let leather = Identifier::parse("minecraft:leather").expect("leather identifier");
    let villager = Identifier::parse("minecraft:villager").expect("villager identifier");
    fixture.sessions.configure_arrow_kill_rewards(
        Some(98),
        None,
        None,
        Arc::new(ItemRegistry::from_report(&[ItemReport {
            id: leather.clone(),
            protocol_id: 41,
        }])),
        Arc::new(solaris_required_item_facts()),
        Arc::new(LootTables::from_drop_lists(
            std::collections::BTreeMap::from([(villager, vec![LootDrop::single(leather)])]),
            std::collections::BTreeMap::new(),
        )),
    );
    let before = gear(&storage, &handle);
    // Death must be handled on a resident that is already demobilising.
    let record = storage
        .resident_orders()
        .record(&handle)
        .cloned()
        .expect("the seeded gear carries a record");
    let outcome = fixture
        .execute(
            &mut storage,
            &demobilize_request("demob-before-death", &handle, record.revision),
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
        mc_script::ScriptDemobilizeState::Demobilizing,
        "a resident with no warehouse to hand its gear to stays demobilising"
    );
    assert_eq!(resident.reason, Some(ScriptWorkPauseReason::NoStorage));
    assert!(resident.returned.is_empty());
    assert_eq!(
        gear(&storage, &handle),
        before,
        "an unfinished demobilisation keeps the gear on the resident"
    );
    let record = storage
        .resident_orders()
        .record(&handle)
        .cloned()
        .expect("the demobilising record survives");
    let expected = fixture.sessions.resident_entity_snapshots(&[uuid]).await[0]
        .clone()
        .expect("the resident is native and alive");
    let entity = expected.id;
    let hit = fixture
        .sessions
        .commit_resident_damage(
            OWNER,
            vec![ResidentAttack {
                uuid,
                amount: 1_000.0,
                expected,
            }],
        )
        .await;
    assert!(
        hit[0].as_ref().is_some_and(|hit| hit.killed),
        "the resident must really die: {hit:?}"
    );
    assert_eq!(
        fixture.sessions.published_entity_health_for_test(entity),
        Some(0.0),
        "an accepted death reaches the session projection every other source uses"
    );
    let drops = fixture
        .sessions
        .persisted_entity_records()
        .into_iter()
        .filter(|record| record.snapshot.type_name == "minecraft:item")
        .collect::<Vec<_>>();
    assert_eq!(
        drops.len(),
        1,
        "the death publishes exactly the loot its configured mob table declares"
    );
    let stack = drops[0]
        .snapshot
        .item_stack
        .as_ref()
        .expect("a dropped item entity carries its stack");
    assert_eq!(
        (stack.item_id, stack.count),
        (41, 1),
        "the published loot is the configured drop, never the issued gear"
    );
    assert_eq!(
        gear(&storage, &handle),
        before,
        "the durable record stays the only owner of the issued gear"
    );
    assert_eq!(
        storage
            .resident_orders()
            .record(&handle)
            .expect("the record survives the death")
            .assignment,
        crate::script::storage::resident_orders::DurableAssignment::Demobilizing,
        "a death does not undo the pending demobilisation"
    );
    for attempt in ["demob-after-real-death", "demob-after-real-death-repeat"] {
        let outcome = fixture
            .execute(
                &mut storage,
                &demobilize_request(attempt, &handle, record.revision),
            )
            .await;
        assert_eq!(
            outcome.failure(),
            Some(ScriptOperationFailure::NotFound),
            "a dead resident cannot finish demobilisation ({attempt})"
        );
    }
    assert_eq!(gear(&storage, &handle), before);

    // A restart keeps the same single owner and the same refusal.
    let mut reopened = PluginStorage::open(fixture.storage_root.path()).expect("reopen orders");
    assert_eq!(gear(&reopened, &handle), before);
    let outcome = fixture
        .execute(
            &mut reopened,
            &demobilize_request("demob-after-restart", &handle, record.revision),
        )
        .await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::NotFound),
        "a restart does not let a dead resident finish demobilisation"
    );
    assert_eq!(gear(&reopened, &handle), before);
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
    fixture
        .runtime
        .recover_resident_orders(&mut reopened)
        .await
        .unwrap();
    assert!(
        reopened.resident_orders().pending_admissions().is_empty(),
        "the accepted batch is applied exactly once"
    );
    fixture
        .runtime
        .recover_resident_orders(&mut reopened)
        .await
        .unwrap();
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

/// (f) Demobilising a serving resident interrupts movement durably, even when
/// no warehouse can yet take its issued gear.
#[tokio::test]
async fn demobilisation_without_a_warehouse_keeps_handle_and_gear() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(5.5, 64.0, 4.5))
        .await;
    seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:iron_sword", 1), ("minecraft:arrow", 8)],
    );
    let march = fixture
        .execute(
            &mut storage,
            &order_request(
                "march-before-demob",
                std::slice::from_ref(&handle),
                &[0],
                ScriptResidentOrder::Move {
                    dimension: "minecraft:overworld".to_owned(),
                    anchor: ScriptBlockPosition::new(13, 64, 8),
                    heading_degrees: 0,
                    formation: formation(),
                },
            ),
        )
        .await;
    assert_eq!(march.failure(), None);
    let prior_goal = fixture.goal(uuid).await;
    assert!(matches!(
        prior_goal,
        mc_entity::GoalState::FollowPosition { .. }
    ));
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
    assert_eq!(fixture.goal(uuid).await, mc_entity::GoalState::Idle);
    let admission_id = storage
        .force_pending_admission_for_test(OWNER, "demob-replay", std::slice::from_ref(&record))
        .expect("committed demobilization requires Idle on replay");
    fixture
        .sessions
        .apply_resident_goals(vec![ResidentGoal {
            uuid,
            goal: prior_goal,
        }])
        .await;

    // Everything is durable: the reopened journal still shows the same state.
    let mut reopened = PluginStorage::open(fixture.storage_root.path()).unwrap();
    fixture
        .runtime
        .recover_resident_orders(&mut reopened)
        .await
        .unwrap();
    assert_eq!(fixture.goal(uuid).await, mc_entity::GoalState::Idle);
    assert!(
        reopened.resident_orders().pending_admissions().is_empty(),
        "demobilization admission {admission_id} is acknowledged after Idle"
    );
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
    let entity_id = fixture.sessions.resident_entity_snapshots(&[uuid]).await[0]
        .as_ref()
        .expect("resident still present before conversion")
        .id;
    assert!(fixture.sessions.convert_resident_entity_for_test(entity_id));
    let deceased = fixture
        .execute(
            &mut reopened,
            &demobilize_request("demob-after-death", &handle, record.revision),
        )
        .await;
    assert_eq!(
        deceased.failure(),
        Some(ScriptOperationFailure::NotFound),
        "a converted or dead villager cannot finish demobilization"
    );
    assert_eq!(
        reopened
            .resident_orders()
            .record(&handle)
            .unwrap()
            .assignment,
        crate::script::storage::resident_orders::DurableAssignment::Demobilizing
    );
    assert_eq!(
        gear(&reopened, &handle),
        before,
        "death does not mint a second gear owner"
    );
}

#[tokio::test]
async fn civilian_with_issued_gear_demobilizes_without_a_prior_military_order() {
    use crate::script::storage::resident_orders::DurableAssignment;

    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 4, Vec3::new(5.5, 64.0, 4.5))
        .await;
    seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_sword", 1)]);
    let before = storage.resident_orders().record(&handle).unwrap().clone();
    assert_eq!(before.assignment, DurableAssignment::Civilian);
    let first = fixture
        .execute(
            &mut storage,
            &demobilize_request("civilian-stop", &handle, before.revision),
        )
        .await;
    assert_eq!(first.failure(), None, "{first:?}");
    let stopped = storage.resident_orders().record(&handle).unwrap().clone();
    assert_eq!(stopped.assignment, DurableAssignment::Demobilizing);
    assert!(stopped.work.is_none());
    assert_eq!(fixture.goal(uuid).await, mc_entity::GoalState::Idle);
    let finished = fixture
        .execute(
            &mut storage,
            &demobilize_request("civilian-finish", &handle, stopped.revision),
        )
        .await;
    assert_eq!(finished.failure(), None, "{finished:?}");
    let restored = storage.resident_orders().record(&handle).unwrap();
    assert_eq!(restored.assignment, DurableAssignment::Civilian);
    assert_eq!(restored.entity_uuid, uuid.to_string());
    assert_eq!(
        gear(&storage, &handle),
        vec![("minecraft:iron_sword".to_owned(), 1)]
    );
}

/// Ambulatory evacuation moves the original wounded patient and medic on the
/// regional physics path; injuries, UUID and gear stay on the original entity.
#[tokio::test]
async fn wounded_patient_and_medic_walk_without_replacing_patient() {
    use mc_physics::{BlockMaterial, BlockMaterialIds, BlockSampler, EntityBody, PhysicsConfig};

    struct Terrain<'a> {
        chunks: &'a mc_world::WorldReadSnapshot,
        materials: &'a BlockMaterialIds,
    }
    impl BlockSampler for Terrain<'_> {
        fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
            self.chunks
                .get_cached_block(BlockPos { x, y, z })
                .map_or(BlockMaterial::Air, |state| self.materials.classify(state.0))
        }
    }
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (medic, medic_uuid) = fixture
        .resident(&mut storage, 31, Vec3::new(3.5, 64.0, 10.5))
        .await;
    let (patient, patient_uuid) = fixture
        .resident(&mut storage, 32, Vec3::new(4.5, 64.0, 10.5))
        .await;
    seed_gear(
        &mut storage,
        &patient,
        patient_uuid,
        &[("minecraft:emerald", 1)],
    );
    let before_hit = fixture
        .sessions
        .resident_entity_snapshots(&[patient_uuid])
        .await[0]
        .clone()
        .expect("same living patient");
    let patient_id = before_hit.id;
    let injury = fixture
        .sessions
        .commit_resident_damage(
            OWNER,
            vec![crate::play::ResidentAttack {
                uuid: patient_uuid,
                amount: 4.0,
                expected: before_hit,
            }],
        )
        .await;
    assert!(injury[0].is_some());
    let injured_health = fixture.health(patient_uuid).await;
    assert!(injured_health > 0.0 && injured_health < 20.0);
    let mut handles = vec![medic, patient.clone()];
    handles.sort_unstable();
    let result = fixture
        .execute(
            &mut storage,
            &order_request(
                "wounded-escort",
                &handles,
                &[0, 0],
                ScriptResidentOrder::Move {
                    dimension: "minecraft:overworld".to_owned(),
                    anchor: ScriptBlockPosition::new(7, 64, 10),
                    heading_degrees: 0,
                    formation: formation(),
                },
            ),
        )
        .await;
    let (_, outcomes, _) = order_of(&result);
    assert!(
        outcomes
            .iter()
            .all(|member| member.state == ScriptOrderMemberState::Applied),
        "{result:?}"
    );
    for uuid in [medic_uuid, patient_uuid] {
        assert!(
            matches!(
                fixture.goal(uuid).await,
                mc_entity::GoalState::FollowPosition { .. }
            ),
            "both members receive native walking goals without a teleport"
        );
    }
    assert_eq!(
        fixture.position(patient_uuid).await,
        Vec3::new(4.5, 64.0, 10.5)
    );
    let current = fixture
        .sessions
        .resident_entity_snapshots(&[patient_uuid])
        .await[0]
        .clone()
        .expect("patient remains native");
    assert_eq!(current.id, patient_id);
    assert_eq!(fixture.health(patient_uuid).await, injured_health);
    let original_positions = [
        fixture.position(medic_uuid).await,
        fixture.position(patient_uuid).await,
    ];
    let medic_id = fixture
        .sessions
        .resident_entity_snapshots(&[medic_uuid])
        .await[0]
        .as_ref()
        .expect("living medic")
        .id;
    fixture
        .sessions
        .register_loaded_for_server_test("EvacuationObserver", (0, 0));
    let world_read = fixture.world.lock().await.read_view();
    let materials = BlockMaterialIds::new(
        state_of(&fixture.blocks, "air").0,
        Some(state_of(&fixture.blocks, "water").0),
        None,
    );
    let chunks = world_read.snapshot_chunks(&[ChunkPos { x: 0, z: 0 }]);
    let terrain = Terrain {
        chunks: &chunks,
        materials: &materials,
    };
    for tick in 1..=30 {
        let queries = fixture
            .sessions
            .tick_entities_and_collect_physics_queries_with_terrain(tick, &world_read, &materials);
        if tick == 1 {
            assert!(
                queries.iter().any(|query| query.id == medic_id)
                    && queries.iter().any(|query| query.id == patient_id),
                "both residents must enter native simulation"
            );
        }
        let steps = queries
            .iter()
            .filter(|query| [medic_id, patient_id].contains(&query.id))
            .map(|query| {
                let stepped = mc_physics::step_entity(
                    EntityBody {
                        position: mc_physics::Vec3::new(
                            query.position.x,
                            query.position.y,
                            query.position.z,
                        ),
                        velocity: mc_physics::Vec3::new(
                            query.velocity.x,
                            query.velocity.y,
                            query.velocity.z,
                        ),
                        aabb: query.aabb,
                        on_ground: query.on_ground,
                    },
                    &terrain,
                    PhysicsConfig::living_entity(),
                );
                EntityPhysicsStep {
                    id: query.id,
                    position: Vec3::new(
                        stepped.body.position.x,
                        stepped.body.position.y,
                        stepped.body.position.z,
                    ),
                    velocity: Vec3::new(
                        stepped.body.velocity.x,
                        stepped.body.velocity.y,
                        stepped.body.velocity.z,
                    ),
                    on_ground: stepped.body.on_ground,
                    horizontal_collision: stepped.horizontal_collision,
                }
            })
            .collect::<Vec<_>>();
        fixture
            .sessions
            .apply_entity_physics_if_current_and_dispatch(tick, &queries, &steps);
    }
    for (uuid, before) in [
        (medic_uuid, original_positions[0]),
        (patient_uuid, original_positions[1]),
    ] {
        let after = fixture.position(uuid).await;
        let dx = after.x - before.x;
        let dz = after.z - before.z;
        assert!(
            dx * dx + dz * dz > 0.04,
            "{uuid} failed to walk from {before:?} to {after:?}"
        );
    }
    assert_eq!(fixture.health(patient_uuid).await, injured_health);
    let reopened = PluginStorage::open(fixture.storage_root.path()).unwrap();
    let same_patient = reopened.resident_orders().record(&patient).unwrap();
    assert_eq!(same_patient.entity_uuid, patient_uuid.to_string());
    assert_eq!(
        gear(&reopened, &patient),
        vec![("minecraft:emerald".to_owned(), 1)]
    );
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
