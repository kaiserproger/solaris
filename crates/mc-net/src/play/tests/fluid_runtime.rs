use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{
    AppliedBlockEdit, BlockEdit, BlockEditBatchOutcome, BlockStateId, Chunk, ChunkPos, FluidKind,
    Identifier, ItemRegistry, ItemReport, ItemStack, ItemToBlockTable, PlayerInventory,
    WATER_FLOW_DELAY_TICKS, apply_block_edit_to_storage, fluid_test_facts, fluid_test_registry,
    fluid_tick_edits, insert_fluid_test_chunk, interaction_state_for_blocks,
    plan_bucket_replacement, published_block_precondition, schedule_fluid_ticks_for_interaction,
    schedule_fluid_ticks_near_applied, simulation_channel,
};

#[test]
fn bucket_items_resolve_fluid_sources() {
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: Identifier::parse("minecraft:bucket").unwrap(),
            protocol_id: 60,
        },
        ItemReport {
            id: Identifier::parse("minecraft:water_bucket").unwrap(),
            protocol_id: 61,
        },
        ItemReport {
            id: Identifier::parse("minecraft:lava_bucket").unwrap(),
            protocol_id: 62,
        },
    ]);
    let blocks = fluid_test_registry();
    let table = ItemToBlockTable::build(&items, &blocks);

    assert_eq!(table.empty_bucket_item(), Some(60));
    assert_eq!(table.bucket_fluid_kind(61), Some(FluidKind::Water));
    assert_eq!(table.bucket_fluid_kind(62), Some(FluidKind::Lava));
    assert_eq!(
        table.fluid_source_state(FluidKind::Water),
        Some(BlockStateId(2))
    );
    assert_eq!(
        table.fluid_source_state(FluidKind::Lava),
        Some(BlockStateId(10))
    );
}

#[test]
fn bucket_replacement_updates_single_held_stack_only() {
    let mut inventory = PlayerInventory::empty();
    inventory
        .set_hotbar(
            0,
            ItemStack {
                item_id: 61,
                count: 1,
                damage: None,
                enchantments: Vec::new(),
                custom_name: None,
                item_model: None,
            },
        )
        .unwrap();

    let (next, changed) =
        plan_bucket_replacement(&inventory, PlayerInventory::HOTBAR_BASE, 60, 16).unwrap();

    assert_eq!(next.held(0).unwrap().item_id, 60);
    assert_eq!(next.held(0).unwrap().count, 1);
    assert_eq!(
        changed,
        vec![(PlayerInventory::HOTBAR_BASE, next.held(0).unwrap().clone())]
    );

    inventory
        .set_hotbar(
            0,
            ItemStack {
                item_id: 60,
                count: 2,
                damage: None,
                enchantments: Vec::new(),
                custom_name: None,
                item_model: None,
            },
        )
        .unwrap();
    let (next, changed) =
        plan_bucket_replacement(&inventory, PlayerInventory::HOTBAR_BASE, 61, 1).unwrap();
    assert_eq!(next.held(0).unwrap().item_id, 60);
    assert_eq!(next.held(0).unwrap().count, 1);
    assert_eq!(next.slots[9].item_id, 61);
    assert_eq!(next.slots[9].count, 1);
    assert_eq!(
        changed,
        vec![
            (PlayerInventory::HOTBAR_BASE, next.held(0).unwrap().clone(),),
            (9, next.slots[9].clone())
        ]
    );

    let mut full_inventory = inventory.clone();
    for slot in 9..=44 {
        if slot != PlayerInventory::HOTBAR_BASE {
            full_inventory.slots[slot] = ItemStack::new(99, 64);
        }
    }
    assert!(
        plan_bucket_replacement(&full_inventory, PlayerInventory::HOTBAR_BASE, 61, 1).is_none()
    );

    inventory.slots[45] = ItemStack {
        item_id: 60,
        count: 1,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
    };
    let (next, changed) = plan_bucket_replacement(&inventory, 45, 61, 1).unwrap();
    assert_eq!(next.slots[45].item_id, 61);
    assert_eq!(next.slots[45].count, 1);
    assert_eq!(changed, vec![(45, next.slots[45].clone())]);
}

#[tokio::test]
async fn bucket_precondition_reads_published_state_while_world_writer_is_held() {
    let state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    insert_fluid_test_chunk(&state).await;
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let expected_token = state.world.lock().await.block_mutation_token(pos).unwrap();
    let writer = state.world.lock().await;

    let precondition = published_block_precondition(&state, pos).unwrap();

    assert_eq!(precondition.pos, pos);
    assert_eq!(precondition.expected_state, BlockStateId(0));
    assert_eq!(precondition.expected_token, expected_token);
    drop(writer);
}

#[test]
fn fluid_tick_flows_sideways_when_blocked_below() {
    let facts = fluid_test_facts();
    let registry = Arc::new(fluid_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(2)).unwrap();
    world
        .set_block_at(mc_world::BlockPos { y: 63, ..pos }, BlockStateId(1))
        .unwrap();

    let edits = fluid_tick_edits(
        blocks,
        &facts,
        &world,
        pos,
        BlockStateId(2),
        facts.fluid(2).unwrap(),
    );

    assert_eq!(edits.len(), 4);
    assert!(edits.iter().all(|edit| edit.new_state == BlockStateId(3)));
}

#[test]
fn fluid_tick_does_not_materialize_neighbour_chunks() {
    struct CountingAirGenerator {
        calls: Arc<AtomicUsize>,
    }

    impl mc_world::ChunkGenerator for CountingAirGenerator {
        fn generate(&self, pos: ChunkPos) -> Chunk {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let mut chunk = Chunk::empty(
                pos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            );
            chunk.status = "minecraft:full".into();
            chunk.dirty = true;
            chunk
        }
    }

    let facts = fluid_test_facts();
    let registry = Arc::new(fluid_test_registry());
    let generated_chunks = Arc::new(AtomicUsize::new(0));
    let mut world = mc_world::WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 16)
        .with_generator(Arc::new(CountingAirGenerator {
            calls: Arc::clone(&generated_chunks),
        }));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let source = mc_world::BlockPos {
        x: 15,
        y: 64,
        z: 15,
    };
    world.set_block_at(source, BlockStateId(2)).unwrap();
    world
        .set_block_at(mc_world::BlockPos { y: 63, ..source }, BlockStateId(1))
        .unwrap();

    let _ = fluid_tick_edits(
        registry.as_ref(),
        &facts,
        &world,
        source,
        BlockStateId(2),
        facts.fluid(2).unwrap(),
    );

    assert_eq!(generated_chunks.load(Ordering::Relaxed), 0);
    assert_eq!(world.cache_len(), 1);
}

#[test]
fn unsupported_flow_decays_to_air() {
    let facts = fluid_test_facts();
    let registry = Arc::new(fluid_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(4)).unwrap();
    world
        .set_block_at(mc_world::BlockPos { y: 63, ..pos }, BlockStateId(1))
        .unwrap();

    let edits = fluid_tick_edits(
        blocks,
        &facts,
        &world,
        pos,
        BlockStateId(4),
        facts.fluid(4).unwrap(),
    );

    assert_eq!(
        edits,
        vec![BlockEdit {
            pos,
            new_state: BlockStateId(0)
        }]
    );
}

#[test]
fn removed_bucket_source_drains_own_spread_from_source_cell() {
    let facts = fluid_test_facts();
    let registry = Arc::new(fluid_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let source = mc_world::BlockPos { x: 8, y: 64, z: 8 };
    seed_fluid_test_floor(&mut world, 1..=15, source.y - 1, 1..=15);
    world.set_block_at(source, BlockStateId(2)).unwrap();

    for _ in 0..6 {
        run_fluid_test_step(blocks, &facts, &mut world, 1..=15, source.y, 1..=15);
    }

    world.set_block_at(source, BlockStateId(0)).unwrap();
    let mut source_refilled = false;
    for _ in 0..16 {
        run_fluid_test_step(blocks, &facts, &mut world, 1..=15, source.y, 1..=15);
        let state = world.get_block(source).unwrap().unwrap();
        if facts.fluid(state.0).is_some() {
            source_refilled = true;
            break;
        }
    }

    assert!(
        !source_refilled,
        "removed bucket source cell must not be repopulated by its own stale flowing water"
    );
}

fn seed_fluid_test_floor(
    world: &mut mc_world::WorldStorage,
    xs: std::ops::RangeInclusive<i32>,
    y: i32,
    zs: std::ops::RangeInclusive<i32>,
) {
    for x in xs {
        for z in zs.clone() {
            world
                .set_block_at(mc_world::BlockPos { x, y, z }, BlockStateId(1))
                .unwrap();
        }
    }
}

fn run_fluid_test_step(
    blocks: &mc_world::BlockRegistry,
    facts: &mc_data::block_facts::BlockFactsTable,
    world: &mut mc_world::WorldStorage,
    xs: std::ops::RangeInclusive<i32>,
    y: i32,
    zs: std::ops::RangeInclusive<i32>,
) {
    let mut positions = Vec::new();
    for x in xs {
        for z in zs.clone() {
            let pos = mc_world::BlockPos { x, y, z };
            if world
                .get_block(pos)
                .ok()
                .flatten()
                .is_some_and(|state| facts.fluid(state.0).is_some())
            {
                positions.push(pos);
            }
        }
    }

    let mut outcome = BlockEditBatchOutcome::default();
    for pos in positions {
        let Some(state) = world.get_cached_block(pos) else {
            continue;
        };
        let Some(fluid) = facts.fluid(state.0) else {
            continue;
        };
        for edit in fluid_tick_edits(blocks, facts, world, pos, state, fluid) {
            apply_block_edit_to_storage(world, None, &edit, &mut outcome);
        }
    }
}

#[test]
fn scheduling_fluid_edits_uses_current_tick_delay() {
    let facts = fluid_test_facts();
    let registry = Arc::new(fluid_test_registry());
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(2)).unwrap();

    schedule_fluid_ticks_near_applied(
        &mut world,
        &facts,
        100,
        &[AppliedBlockEdit {
            pos,
            previous: BlockStateId(0),
            new_state: BlockStateId(2),
        }],
    );

    let ticks = world.scheduled_fluid_ticks(cpos).unwrap().unwrap();
    assert_eq!(ticks.len(), 1);
    assert_eq!(ticks[0].pos, pos);
    assert_eq!(ticks[0].trigger_tick, 100 + WATER_FLOW_DELAY_TICKS);
}

#[tokio::test]
async fn interaction_fluid_scheduling_uses_shared_simulation_tick() {
    let mut state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    let (simulation, mut simulation_owner) = simulation_channel();
    state.simulation = simulation;
    state.block_facts = Arc::new(fluid_test_facts());
    insert_fluid_test_chunk(&state).await;
    state.sessions.advance_world_time(100);

    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    {
        let mut world = state.world.lock().await;
        world.set_block_at(pos, BlockStateId(2)).unwrap();
    }

    schedule_fluid_ticks_for_interaction(
        &state,
        &[AppliedBlockEdit {
            pos,
            previous: BlockStateId(0),
            new_state: BlockStateId(2),
        }],
    )
    .await;

    {
        let mut world = state.world.lock().await;
        assert!(
            world
                .scheduled_fluid_ticks(ChunkPos { x: 0, z: 0 })
                .unwrap()
                .unwrap()
                .is_empty(),
            "network task must not schedule fluid ticks directly"
        );
    }
    assert_eq!(
        simulation_owner
            .process_tick_with_world(&state.sessions, Some(&state.world), None, 1)
            .processed,
        1
    );

    let ticks = {
        let mut world = state.world.lock().await;
        world
            .scheduled_fluid_ticks(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .unwrap()
            .to_vec()
    };
    assert_eq!(ticks.len(), 1);
    assert_eq!(ticks[0].pos, pos);
    assert_eq!(ticks[0].trigger_tick, 100 + WATER_FLOW_DELAY_TICKS);
}

#[test]
fn water_lava_interactions_make_solid_blocks() {
    let facts = fluid_test_facts();
    let registry = Arc::new(fluid_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let water_pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let lava_source_pos = mc_world::BlockPos { x: 5, y: 64, z: 4 };
    world.set_block_at(water_pos, BlockStateId(2)).unwrap();
    world
        .set_block_at(lava_source_pos, BlockStateId(10))
        .unwrap();

    let edits = fluid_tick_edits(
        blocks,
        &facts,
        &world,
        water_pos,
        BlockStateId(2),
        facts.fluid(2).unwrap(),
    );
    assert_eq!(
        edits,
        vec![BlockEdit {
            pos: lava_source_pos,
            new_state: BlockStateId(14),
        }]
    );

    world
        .set_block_at(lava_source_pos, BlockStateId(0))
        .unwrap();
    let lava_flow_pos = mc_world::BlockPos { x: 4, y: 63, z: 4 };
    world.set_block_at(lava_flow_pos, BlockStateId(11)).unwrap();
    let edits = fluid_tick_edits(
        blocks,
        &facts,
        &world,
        lava_flow_pos,
        BlockStateId(11),
        facts.fluid(11).unwrap(),
    );
    assert_eq!(
        edits,
        vec![BlockEdit {
            pos: lava_flow_pos,
            new_state: BlockStateId(1),
        }]
    );
}
