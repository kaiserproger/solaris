use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;
use std::task::Poll;

use tokio::sync::mpsc;

use super::{
    AppliedBlockEdit, BlockStateId, Chunk, ChunkPos, EntityId, EntityItemStack,
    FALLING_BLOCK_LANDING_PLANNING_COMPLETION_COUNT, FALLING_BLOCK_START_PLANNING_COMPLETION_COUNT,
    Identifier, ItemRegistry, LoggedInProfile, OutboundCommand, PlayerPose, ServerConfig,
    ServerEntitySnapshot, SessionRegistry, Vec3, dispatch_and_clear_setup_packets,
    fluid_test_facts, fluid_test_registry, in_memory_button_world, insert_fluid_test_chunk,
    interaction_state_for_blocks, play_loop_slow_client_test_config, simulation_channel,
    start_falling_blocks_after_edits,
};
use crate::play::falling_blocks::{
    FallingBlockStart, LandedFallingBlock, plan_falling_block_starts,
};
use mc_data::items::ItemReport;

#[test]
fn falling_block_starts_when_support_edit_becomes_replaceable() {
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
    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let sand = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let upper_sand = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    world.set_block_at(support, BlockStateId(1)).unwrap();
    world.set_block_at(sand, BlockStateId(16)).unwrap();
    world.set_block_at(upper_sand, BlockStateId(16)).unwrap();
    world.set_block_at(support, BlockStateId(0)).unwrap();

    let plan = plan_falling_block_starts(
        blocks,
        &facts,
        &world,
        &[AppliedBlockEdit {
            pos: support,
            previous: BlockStateId(1),
            new_state: BlockStateId(0),
        }],
        BlockStateId(0),
    );

    assert_eq!(
        plan.starts,
        vec![
            FallingBlockStart {
                pos: sand,
                state: BlockStateId(16),
            },
            FallingBlockStart {
                pos: upper_sand,
                state: BlockStateId(16),
            }
        ]
    );
}

#[tokio::test]
async fn falling_block_start_planning_does_not_wait_for_world_writer() {
    let blocks = Arc::new(fluid_test_registry());
    let mut state = interaction_state_for_blocks(Arc::clone(&blocks));
    state.block_facts = Arc::new(fluid_test_facts());
    state.entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    insert_fluid_test_chunk(&state).await;
    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let sand = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    state
        .world
        .lock()
        .await
        .set_block_at(sand, mc_world::BlockStateId(16))
        .expect("place falling sand");
    let applied = [AppliedBlockEdit {
        pos: support,
        previous: mc_world::BlockStateId(1),
        new_state: mc_world::BlockStateId(0),
    }];
    let world = Arc::clone(&state.world);
    let world_writer = world.lock().await;
    let mut writer = tokio::io::sink();
    FALLING_BLOCK_START_PLANNING_COMPLETION_COUNT.with(|count| count.set(0));
    let mut pass = Box::pin(start_falling_blocks_after_edits(
        &mut state,
        &mut writer,
        &applied,
    ));

    let pending = std::future::poll_fn(|cx| {
        Poll::Ready(matches!(Future::poll(pass.as_mut(), cx), Poll::Pending))
    })
    .await;
    assert!(
        pending,
        "falling-block removal commit must wait for the writer"
    );
    FALLING_BLOCK_START_PLANNING_COMPLETION_COUNT.with(|count| {
        assert_eq!(
            count.get(),
            1,
            "falling-block column discovery must finish before waiting for the writer"
        );
    });

    drop(world_writer);
    pass.await.expect("falling block starts");
    assert_eq!(
        world.lock().await.get_cached_block(sand),
        Some(mc_world::BlockStateId(0))
    );

    world
        .lock()
        .await
        .set_block_at(sand, mc_world::BlockStateId(16))
        .expect("place replacement falling sand");
    let entities_before = state.sessions.pressure_snapshot().server_entities;
    let mut world_writer = world.lock().await;
    FALLING_BLOCK_START_PLANNING_COMPLETION_COUNT.with(|count| count.set(0));
    let mut stale_pass = Box::pin(start_falling_blocks_after_edits(
        &mut state,
        &mut writer,
        &applied,
    ));
    let pending = std::future::poll_fn(|cx| {
        Poll::Ready(matches!(
            Future::poll(stale_pass.as_mut(), cx),
            Poll::Pending
        ))
    })
    .await;
    assert!(
        pending,
        "stale falling-block commit must wait for the writer"
    );
    FALLING_BLOCK_START_PLANNING_COMPLETION_COUNT.with(|count| assert_eq!(count.get(), 1));
    world_writer
        .set_block_at(sand, mc_world::BlockStateId(1))
        .expect("replace planned falling block");
    drop(world_writer);
    stale_pass.await.expect("stale falling start is rejected");
    assert_eq!(
        world.lock().await.get_cached_block(sand),
        Some(mc_world::BlockStateId(1))
    );
    assert_eq!(
        state.sessions.pressure_snapshot().server_entities,
        entities_before
    );
}

#[tokio::test]
async fn falling_block_landing_on_solid_drops_item_and_despawns_entity() {
    let blocks = Arc::new(fluid_test_registry());
    let mut storage = in_memory_button_world(Arc::clone(&blocks));
    let landing_pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    storage
        .set_block_at(landing_pos, mc_world::BlockStateId(1))
        .expect("place occupied landing block");
    let world_read = storage.read_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:sand").unwrap(),
        protocol_id: 42,
    }]));
    let entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    let config = ServerConfig {
        blocks: Arc::clone(&blocks),
        world: Some(Arc::clone(&world)),
        items,
        entity_types,
        block_facts: Arc::new(fluid_test_facts()),
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(46),
        name: "FallingBlockViewer".to_string(),
    };
    let (tx, mut rx) = mpsc::channel(16);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(4.5, 64.0, 4.5),
    );
    let mut setup_dispatches = sessions.mark_loaded(session_id, (0, 0));

    let falling_spawn =
        sessions.spawn_falling_block(70, Vec3::new(4.5, 65.0, 4.5), mc_world::BlockStateId(16));
    let falling_id = falling_spawn
        .iter()
        .find_map(|dispatch| match &dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .expect("falling block spawn dispatch");
    setup_dispatches.extend(falling_spawn);
    dispatch_and_clear_setup_packets(setup_dispatches, &mut [&mut rx]);

    let (_simulation, owner) = simulation_channel();
    let applied = owner
        .land_falling_blocks(
            &config,
            &sessions,
            Some(&world_read),
            &[LandedFallingBlock {
                id: falling_id,
                pos: landing_pos,
                state: mc_world::BlockStateId(16),
            }],
        )
        .await;

    assert_eq!(applied, 0);
    assert_eq!(
        world.lock().await.get_cached_block(landing_pos),
        Some(mc_world::BlockStateId(1))
    );
    assert!(sessions.server_entity_snapshot(falling_id).is_none());

    let item_spawn = rx
        .try_recv()
        .expect("blocked falling block should spawn item drop");
    assert!(matches!(
        item_spawn,
        OutboundCommand::SpawnEntity(ServerEntitySnapshot {
            type_name,
            item_stack: Some(stack),
            ..
        }) if type_name == "minecraft:item" && stack == EntityItemStack::new(42, 1)
    ));
    assert!(matches!(
        rx.try_recv(),
        Ok(OutboundCommand::DespawnEntity(entity)) if entity.id == falling_id
    ));
}

#[tokio::test]
async fn falling_block_landing_planning_does_not_wait_for_world_writer() {
    let blocks = Arc::new(fluid_test_registry());
    let storage = in_memory_button_world(Arc::clone(&blocks));
    let world_read = storage.read_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        blocks,
        world: Some(Arc::clone(&world)),
        block_facts: Arc::new(fluid_test_facts()),
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let landing_pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let candidate = LandedFallingBlock {
        id: EntityId(99),
        pos: landing_pos,
        state: mc_world::BlockStateId(16),
    };
    let (_simulation, owner) = simulation_channel();
    let world_writer = world.lock().await;
    FALLING_BLOCK_LANDING_PLANNING_COMPLETION_COUNT.with(|count| count.set(0));
    let candidates = [candidate];
    let mut pass =
        Box::pin(owner.land_falling_blocks(&config, &sessions, Some(&world_read), &candidates));

    let pending = std::future::poll_fn(|cx| {
        Poll::Ready(matches!(Future::poll(pass.as_mut(), cx), Poll::Pending))
    })
    .await;
    assert!(
        pending,
        "falling-block landing commit must wait for the writer"
    );
    FALLING_BLOCK_LANDING_PLANNING_COMPLETION_COUNT.with(|count| {
        assert_eq!(
            count.get(),
            1,
            "falling-block landing planning must finish before waiting for the writer"
        );
    });

    drop(world_writer);
    assert_eq!(pass.await, 1);
    assert_eq!(
        world.lock().await.get_cached_block(landing_pos),
        Some(mc_world::BlockStateId(16))
    );
    assert_eq!(
        world_read.get_cached_block(landing_pos),
        Some(mc_world::BlockStateId(16))
    );
}

#[tokio::test]
async fn stale_falling_block_landing_plan_keeps_entity_and_replacement() {
    let blocks = Arc::new(fluid_test_registry());
    let storage = in_memory_button_world(Arc::clone(&blocks));
    let world_read = storage.read_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        blocks,
        world: Some(Arc::clone(&world)),
        block_facts: Arc::new(fluid_test_facts()),
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(47),
        name: "StaleFallingBlock".to_string(),
    };
    let (tx, mut rx) = mpsc::channel(16);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(4.5, 64.0, 4.5),
    );
    let _ = sessions.mark_loaded(session_id, (0, 0));
    let falling_id = sessions
        .spawn_falling_block(70, Vec3::new(4.5, 65.0, 4.5), mc_world::BlockStateId(16))
        .into_iter()
        .find_map(|dispatch| match dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .expect("falling block spawn dispatch");
    while rx.try_recv().is_ok() {}

    let landing_pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let candidates = [LandedFallingBlock {
        id: falling_id,
        pos: landing_pos,
        state: mc_world::BlockStateId(16),
    }];
    let (_simulation, owner) = simulation_channel();
    let mut world_writer = world.lock().await;
    FALLING_BLOCK_LANDING_PLANNING_COMPLETION_COUNT.with(|count| count.set(0));
    let mut pass =
        Box::pin(owner.land_falling_blocks(&config, &sessions, Some(&world_read), &candidates));
    let pending = std::future::poll_fn(|cx| {
        Poll::Ready(matches!(Future::poll(pass.as_mut(), cx), Poll::Pending))
    })
    .await;
    assert!(
        pending,
        "falling-block landing commit must wait for the writer"
    );
    FALLING_BLOCK_LANDING_PLANNING_COMPLETION_COUNT.with(|count| assert_eq!(count.get(), 1));
    world_writer
        .set_block_at(landing_pos, mc_world::BlockStateId(1))
        .expect("replace landing cell after snapshot planning");
    drop(world_writer);

    assert_eq!(pass.await, 0);
    assert_eq!(
        world.lock().await.get_cached_block(landing_pos),
        Some(mc_world::BlockStateId(1))
    );
    assert!(sessions.server_entity_snapshot(falling_id).is_some());
    assert!(rx.try_recv().is_err());
}
