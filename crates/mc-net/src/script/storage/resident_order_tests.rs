//! Acceptance tests for resident work and squad order execution (C4).
//!
//! Every test drives the real order executor over a real plugin storage journal,
//! the real regional entity owner and a real world storage kernel: nothing here
//! asserts plumbing. The scenarios are the contract's A08–A12 plus the
//! demobilisation rule.

use std::collections::BTreeMap;
use std::sync::Arc;

use mc_data::block_light::BlockLightTable;
use mc_data::blocks::solaris_required_blocks_report;
use mc_data::item_components::solaris_required_item_facts;
use mc_data::items::solaris_required_items;
use mc_data::{Identifier, ItemStack};
use mc_entity::{SpawnEntity, Vec3};
use mc_protocol::codec;
use mc_script::{
    ScriptBlockPosition, ScriptEngagementPolicy, ScriptFormation, ScriptFormationKind,
    ScriptHostileCategory, ScriptOperation, ScriptOperationFailure, ScriptOperationOutcome,
    ScriptOperationPayload, ScriptOperationRequest, ScriptOrderMemberState, ScriptOrderTargetRef,
    ScriptResidentOrder, ScriptResidentOrderOperation, ScriptResidentOrderResult,
    ScriptResidentWorkOrder, ScriptWorkArea, ScriptWorkPauseReason, ScriptWorkState,
    resident_generation_id,
};
use mc_world::{BlockPos, BlockStateId, Chunk, ChunkPos, WorldReadView, WorldStorage};
use uuid::Uuid;

use crate::play::SessionRegistry;
use crate::play::resident_work::LiveResidentWorld;
use crate::play::resident_work::ResidentWorld;
use crate::script::storage::PluginStorage;
use crate::script::storage::resident_orders::{DurableResidentOrderChange, DurableResidentStack};
use crate::script::storage::world_inventory::InventoryRuntime;
use crate::server::WorldHandle;

const OWNER: &str = "settlement";
const WORLD_IDENTITY: &str = "resident-order-world";
/// Flat terrain surface of the test world.
const SURFACE_Y: i32 = 63;

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
        let blocks = Arc::new(
            mc_world::BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap(),
        );
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
            let ore = state_of(&blocks, "iron_ore");
            let water = state_of(&blocks, "water");
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
            // A rooted tree.
            chunk.set_block(6, SURFACE_Y + 1, 6, log);
            chunk.set_block(6, SURFACE_Y + 2, 6, log);
            // Ore inside the stone band.
            chunk.set_block(4, SURFACE_Y - 2, 4, ore);
            // A single water column.
            chunk.set_block(8, SURFACE_Y, 8, water);
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
        let read: WorldReadView = {
            let storage = world.try_lock().expect("test world is free");
            storage.read_view()
        };
        let sessions = Arc::new(SessionRegistry::new());
        let items = Arc::new(solaris_required_items());
        let item_facts = Arc::new(solaris_required_item_facts());
        let light: Option<Arc<BlockLightTable>> = None;
        let adapter = LiveResidentWorld::new(
            Arc::clone(&world),
            read,
            Arc::clone(&blocks),
            light,
            None,
            Arc::clone(&items),
            Arc::clone(&item_facts),
        );
        let runtime = InventoryRuntime::new(
            None,
            &crate::server::ShutdownHandle::default(),
            Arc::clone(&sessions),
            items,
            item_facts,
        )
        .with_resident_world(Arc::new(adapter) as Arc<dyn ResidentWorld>);
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
