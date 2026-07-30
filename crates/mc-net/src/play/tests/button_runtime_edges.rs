use super::{
    BlockEdit, ChunkPos, Identifier, SCHEDULED_BLOCK_PLANNING_WITHOUT_WRITER_COUNT, ServerConfig,
    SessionRegistry, apply_block_edit_batch_with_scheduled_ticks_to_storage_conditionally,
    button_and_door_test_registry, button_test_registry, due_scheduled_block_ticks,
    in_memory_button_world, piston_test_registry, plan_scheduled_block_tick_edits,
    plan_toggle_block_interaction, play_loop_slow_client_test_config,
    register_loaded_button_session, register_ticketed_button_session, resident_block_edit_inputs,
    run_scheduled_block_ticks, run_scheduled_block_ticks_with_protection,
    scheduled_block_planning_chunks, scheduled_block_tick_edits,
};
use std::sync::Arc;
#[tokio::test]
async fn scheduled_button_tick_ignores_ticketed_chunk_until_loaded() {
    let blocks = Arc::new(button_test_registry());
    let world = Arc::new(tokio::sync::Mutex::new(in_memory_button_world(Arc::clone(
        &blocks,
    ))));
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = world.lock().await;
        storage
            .set_block_at(pos, mc_world::BlockStateId(2))
            .expect("place powered button");
        storage
            .schedule_block_tick(mc_world::ScheduledBlockTick::new(
                pos,
                Identifier::parse("minecraft:stone_button").unwrap(),
                120,
                0,
            ))
            .expect("schedule button release");
    }
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let session_id = register_ticketed_button_session(&sessions, "TicketedButton");

    let before_loaded = run_scheduled_block_ticks(&config, &sessions, 120).await;
    assert_eq!(before_loaded.drained, 0);
    assert_eq!(before_loaded.applied, 0);
    {
        let mut storage = world.lock().await;
        assert_eq!(
            storage.get_cached_block(pos),
            Some(mc_world::BlockStateId(2))
        );
        let ticks = storage
            .scheduled_block_ticks(ChunkPos { x: 0, z: 0 })
            .expect("read scheduled block ticks")
            .expect("cached chunk should expose ticks");
        assert_eq!(ticks.len(), 1);
    }

    let _ = sessions.mark_loaded(session_id, (0, 0));
    SCHEDULED_BLOCK_PLANNING_WITHOUT_WRITER_COUNT.with(|count| count.set(0));
    let after_loaded = run_scheduled_block_ticks(&config, &sessions, 120).await;

    assert_eq!(after_loaded.drained, 1);
    assert_eq!(after_loaded.applied, 1);
    SCHEDULED_BLOCK_PLANNING_WITHOUT_WRITER_COUNT.with(|count| {
        assert_eq!(
            count.get(),
            1,
            "ordinary scheduled-block planning must not hold the world writer"
        );
    });
    let storage = world.lock().await;
    assert_eq!(
        storage.get_cached_block(pos),
        Some(mc_world::BlockStateId(1))
    );
}

#[tokio::test]
async fn stale_scheduled_button_plan_keeps_due_tick_after_aba() {
    let blocks = Arc::new(button_test_registry());
    let mut storage = in_memory_button_world(Arc::clone(&blocks));
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    storage
        .set_block_at(pos, mc_world::BlockStateId(2))
        .expect("place powered button");
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            pos,
            Identifier::parse("minecraft:stone_button").unwrap(),
            120,
            0,
        ))
        .expect("schedule button release");
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let chunk = ChunkPos { x: 0, z: 0 };
    let storage = world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let loaded = world_read.snapshot_chunks(&[chunk]);
    let due = due_scheduled_block_ticks(&loaded, &[chunk], 120, 1);
    assert_eq!(due.len(), 1);
    let planning_chunks = scheduled_block_planning_chunks(&due);
    let snapshot = world_read.snapshot_chunks(&planning_chunks);
    let plan = plan_scheduled_block_tick_edits(&config, &snapshot, &due, None)
        .expect("button-only batch uses snapshot planning");
    assert_eq!(
        plan.edits,
        vec![BlockEdit {
            pos,
            new_state: mc_world::BlockStateId(1),
        }]
    );

    let mut storage = world.lock().await;
    assert_eq!(
        storage
            .set_block_at(pos, mc_world::BlockStateId(1))
            .unwrap(),
        Some(mc_world::BlockStateId(2))
    );
    assert_eq!(
        storage
            .set_block_at(pos, mc_world::BlockStateId(2))
            .unwrap(),
        Some(mc_world::BlockStateId(1))
    );
    drop(storage);
    let (edits, preconditions) =
        resident_block_edit_inputs(&plan.edits, &plan.preconditions, None).unwrap();
    assert_eq!(
        world_mutation.apply_scheduled_block_tick_plan_conditionally(
            &mc_world::ResidentScheduledBlockTickPlan {
                consumed_ticks: &due,
                edits: &edits,
                preconditions: &preconditions,
                light_table: None,
                leaf_trigger_tick: Some(121),
            },
        ),
        mc_world::ResidentBlockEditBatchResult::Stale
    );
    let mut storage = world.lock().await;
    assert_eq!(
        storage.get_cached_block(pos),
        Some(mc_world::BlockStateId(2))
    );
    let restored = storage.scheduled_block_ticks(chunk).unwrap().unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].pos, pos);
    assert_eq!(restored[0].block.as_str(), "minecraft:stone_button");
}

#[test]
fn button_release_keeps_adjacent_door_powered_by_other_control() {
    let blocks = Arc::new(button_and_door_test_registry());
    let mut world = in_memory_button_world(Arc::clone(&blocks));
    let release_button_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let lower_door_pos = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let upper_door_pos = mc_world::BlockPos { x: 2, y: 65, z: 1 };
    let other_button_pos = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    world
        .set_block_at(release_button_pos, mc_world::BlockStateId(2))
        .expect("place releasing powered button");
    world
        .set_block_at(lower_door_pos, mc_world::BlockStateId(5))
        .expect("place powered lower iron door");
    world
        .set_block_at(upper_door_pos, mc_world::BlockStateId(6))
        .expect("place powered upper iron door");
    world
        .set_block_at(other_button_pos, mc_world::BlockStateId(2))
        .expect("place other powered button");

    let edits = scheduled_block_tick_edits(
        &blocks,
        &mut world,
        release_button_pos,
        mc_world::BlockStateId(2),
    )
    .expect("powered button release should edit the releasing button");

    assert_eq!(
        edits,
        vec![BlockEdit {
            pos: release_button_pos,
            new_state: mc_world::BlockStateId(1),
        }]
    );
}

#[tokio::test]
async fn button_press_powers_adjacent_iron_door_until_scheduled_release() {
    let blocks = Arc::new(button_and_door_test_registry());
    let world = Arc::new(tokio::sync::Mutex::new(in_memory_button_world(Arc::clone(
        &blocks,
    ))));
    let button_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let lower_door_pos = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let upper_door_pos = mc_world::BlockPos { x: 2, y: 65, z: 1 };
    {
        let mut storage = world.lock().await;
        storage
            .set_block_at(button_pos, mc_world::BlockStateId(1))
            .expect("place unpowered button");
        storage
            .set_block_at(lower_door_pos, mc_world::BlockStateId(3))
            .expect("place unpowered lower iron door");
        storage
            .set_block_at(upper_door_pos, mc_world::BlockStateId(4))
            .expect("place unpowered upper iron door");
        let plan = plan_toggle_block_interaction(
            &blocks,
            &*storage,
            button_pos,
            mc_world::BlockStateId(1),
            100,
        )
        .expect("button should press and power adjacent door");
        assert_eq!(
            plan.edits,
            vec![
                BlockEdit {
                    pos: button_pos,
                    new_state: mc_world::BlockStateId(2)
                },
                BlockEdit {
                    pos: lower_door_pos,
                    new_state: mc_world::BlockStateId(5)
                },
                BlockEdit {
                    pos: upper_door_pos,
                    new_state: mc_world::BlockStateId(6)
                },
            ]
        );
        let outcome = apply_block_edit_batch_with_scheduled_ticks_to_storage_conditionally(
            &mut storage,
            None,
            &plan.edits,
            &plan.preconditions,
            &plan.scheduled_block_ticks,
        )
        .expect("button plan should match its captured world version");
        assert_eq!(outcome.applied.len(), 3);
    }
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "ButtonDoor");

    let report = run_scheduled_block_ticks(&config, &sessions, 120).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 3);
    let storage = world.lock().await;
    assert_eq!(
        storage.get_cached_block(button_pos),
        Some(mc_world::BlockStateId(1))
    );
    assert_eq!(
        storage.get_cached_block(lower_door_pos),
        Some(mc_world::BlockStateId(3))
    );
    assert_eq!(
        storage.get_cached_block(upper_door_pos),
        Some(mc_world::BlockStateId(4))
    );
}

#[tokio::test]
async fn scheduled_button_release_keeps_piston_extended_when_head_is_protected() {
    let blocks = Arc::new(piston_test_registry());
    let world = Arc::new(tokio::sync::Mutex::new(in_memory_button_world(Arc::clone(
        &blocks,
    ))));
    let button = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let piston = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let arm = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    let destination = mc_world::BlockPos { x: 4, y: 64, z: 1 };
    {
        let mut storage = world.lock().await;
        for (pos, state_id) in [(button, 1), (piston, 5), (arm, 8)] {
            storage
                .set_block_at(pos, mc_world::BlockStateId(state_id))
                .expect("place scheduled piston test block");
        }
        let plan = plan_toggle_block_interaction(
            &blocks,
            &*storage,
            button,
            mc_world::BlockStateId(1),
            100,
        )
        .expect("button should extend adjacent piston");
        apply_block_edit_batch_with_scheduled_ticks_to_storage_conditionally(
            &mut storage,
            None,
            &plan.edits,
            &plan.preconditions,
            &plan.scheduled_block_ticks,
        )
        .expect("button extension plan remains current");
    }
    let zone = mc_script::ScriptAxisAlignedZone::try_new_with_protection(
        "piston-head",
        "minecraft:overworld",
        mc_script::ScriptPosition::try_new(3.0, 64.0, 1.0).unwrap(),
        mc_script::ScriptPosition::try_new(3.0, 64.0, 1.0).unwrap(),
        Some(
            mc_script::ScriptZoneProtection::try_actor_or_operator(
                "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            )
            .unwrap(),
        ),
    )
    .unwrap();
    let protection = Arc::new(crate::script::ZoneProtectionSnapshot::from_zones(vec![
        zone,
    ]));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "ProtectedPiston");

    let report =
        run_scheduled_block_ticks_with_protection(&config, &sessions, protection, 120).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    let storage = world.lock().await;
    assert_eq!(
        storage.get_cached_block(button),
        Some(mc_world::BlockStateId(1))
    );
    assert_eq!(
        storage.get_cached_block(piston),
        Some(mc_world::BlockStateId(6))
    );
    assert_eq!(
        storage.get_cached_block(arm),
        Some(mc_world::BlockStateId(7))
    );
    assert_eq!(
        storage.get_cached_block(destination),
        Some(mc_world::BlockStateId(8))
    );
}
