use super::{
    BlockStateId, GameMode, Identifier, PlayerPose, Vec3,
    apply_block_edit_batch_to_storage_conditionally, bed_respawn_pose,
    bed_sleep_is_blocked_by_monster, bed_sleep_is_obstructed, canonical_bed_position, fluid_block,
    insert_fluid_test_chunk, interaction_state_for_blocks, next_morning_time,
    plan_bed_occupied_edits, plan_loaded_bed_interaction, prop_schema, safe_bed_wake_pose,
    simple_block, simulation, state,
};
use mc_data::blocks::BlockReport;
use std::sync::Arc;

fn bed_occupancy_test_registry() -> mc_world::BlockRegistry {
    mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:red_bed").unwrap(),
            properties: prop_schema(&[
                ("facing", &["north"]),
                ("occupied", &["false", "true"]),
                ("part", &["head", "foot"]),
            ]),
            states: vec![
                state(
                    1,
                    true,
                    &[("facing", "north"), ("occupied", "false"), ("part", "foot")],
                ),
                state(
                    2,
                    false,
                    &[("facing", "north"), ("occupied", "false"), ("part", "head")],
                ),
                state(
                    3,
                    false,
                    &[("facing", "north"), ("occupied", "true"), ("part", "foot")],
                ),
                state(
                    4,
                    false,
                    &[("facing", "north"), ("occupied", "true"), ("part", "head")],
                ),
            ],
        },
    ])
    .unwrap()
}

#[test]
fn bed_respawn_pose_uses_block_above_bed() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:red_bed").unwrap(),
            properties: prop_schema(&[("facing", &["north"])]),
            states: vec![state(1, true, &[("facing", "north")])],
        },
    ])
    .unwrap();
    let pose = bed_respawn_pose(
        mc_world::BlockPos { x: 3, y: 64, z: -2 },
        blocks.by_id(mc_world::BlockStateId(1)).unwrap(),
    );

    assert_eq!((pose.x, pose.y, pose.z, pose.yaw), (3.5, 65.0, -1.5, 180.0));
}

#[test]
fn bed_halves_share_the_head_position_as_reservation_key() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:red_bed").unwrap(),
            properties: prop_schema(&[
                ("facing", &["north"]),
                ("occupied", &["false"]),
                ("part", &["head", "foot"]),
            ]),
            states: vec![
                state(
                    1,
                    true,
                    &[("facing", "north"), ("occupied", "false"), ("part", "foot")],
                ),
                state(
                    2,
                    false,
                    &[("facing", "north"), ("occupied", "false"), ("part", "head")],
                ),
            ],
        },
    ])
    .unwrap();
    let foot = mc_world::BlockPos { x: 3, y: 64, z: 2 };
    let head = mc_world::BlockPos { x: 3, y: 64, z: 1 };

    assert_eq!(
        canonical_bed_position(foot, blocks.by_id(mc_world::BlockStateId(1)).unwrap()),
        head
    );
    assert_eq!(
        canonical_bed_position(head, blocks.by_id(mc_world::BlockStateId(2)).unwrap()),
        head
    );
}

#[tokio::test]
async fn bed_head_and_foot_clicks_share_the_exact_head_centered_respawn_pose() {
    let blocks = Arc::new(bed_occupancy_test_registry());
    let state = interaction_state_for_blocks(blocks);
    insert_fluid_test_chunk(&state).await;
    let head = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    let foot = mc_world::BlockPos { x: 3, y: 64, z: 2 };
    {
        let mut world = state.world.lock().await;
        world.set_block_at(head, BlockStateId(2)).unwrap();
        world.set_block_at(foot, BlockStateId(1)).unwrap();
    }

    let (head_pose, head_canonical) =
        plan_loaded_bed_interaction(&state.world_read, &state.blocks, head)
            .expect("head click plans a respawn");
    let (foot_pose, foot_canonical) =
        plan_loaded_bed_interaction(&state.world_read, &state.blocks, foot)
            .expect("foot click plans a respawn");

    assert_eq!(head_canonical, head);
    assert_eq!(foot_canonical, head);
    assert_eq!(
        (head_pose.x, head_pose.y, head_pose.z, head_pose.yaw),
        (3.5, 65.0, 1.5, 180.0)
    );
    assert_eq!(
        (foot_pose.x, foot_pose.y, foot_pose.z, foot_pose.yaw),
        (head_pose.x, head_pose.y, head_pose.z, head_pose.yaw)
    );
}

#[tokio::test]
async fn bed_planning_rejects_a_mismatched_second_half() {
    let blocks = Arc::new(bed_occupancy_test_registry());
    let state = interaction_state_for_blocks(blocks);
    insert_fluid_test_chunk(&state).await;
    let head = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    let foot = mc_world::BlockPos { x: 3, y: 64, z: 2 };
    {
        let mut world = state.world.lock().await;
        world.set_block_at(head, BlockStateId(2)).unwrap();
        world
            .set_block_at(foot, BlockStateId(2))
            .expect("place mismatched head in foot position");
    }

    assert!(
        plan_loaded_bed_interaction(&state.world_read, &state.blocks, head).is_none(),
        "loaded interaction must reject a second head in the foot position"
    );
    assert!(
        plan_bed_occupied_edits(&state.world_read, &state.blocks, head, true).is_none(),
        "occupancy planning must reject mismatched halves"
    );
}

#[tokio::test]
async fn bed_occupancy_stale_token_rejects_the_whole_edit_set() {
    let blocks = Arc::new(bed_occupancy_test_registry());
    let state = interaction_state_for_blocks(blocks);
    insert_fluid_test_chunk(&state).await;
    let head = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    let foot = mc_world::BlockPos { x: 3, y: 64, z: 2 };
    {
        let mut world = state.world.lock().await;
        world.set_block_at(head, BlockStateId(2)).unwrap();
        world.set_block_at(foot, BlockStateId(1)).unwrap();
    }
    let (edits, preconditions) =
        plan_bed_occupied_edits(&state.world_read, &state.blocks, head, true)
            .expect("matching bed halves plan occupancy edits");

    let outcome = {
        let mut world = state.world.lock().await;
        world.set_block_at(foot, BlockStateId(3)).unwrap();
        world.set_block_at(foot, BlockStateId(1)).unwrap();
        apply_block_edit_batch_to_storage_conditionally(&mut world, None, &edits, &preconditions)
    };

    assert!(
        outcome.is_none(),
        "ABA mutation must stale the occupancy plan"
    );
    let world = state.world.lock().await;
    assert_eq!(world.get_cached_block(head), Some(BlockStateId(2)));
    assert_eq!(world.get_cached_block(foot), Some(BlockStateId(1)));
}

#[tokio::test]
async fn bed_mixed_occupancy_aba_on_unchanged_half_rejects_the_edit() {
    let blocks = Arc::new(bed_occupancy_test_registry());
    let state = interaction_state_for_blocks(blocks);
    insert_fluid_test_chunk(&state).await;
    let head = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    let foot = mc_world::BlockPos { x: 3, y: 64, z: 2 };
    {
        let mut world = state.world.lock().await;
        world.set_block_at(head, BlockStateId(2)).unwrap();
        world.set_block_at(foot, BlockStateId(3)).unwrap();
    }
    let (edits, preconditions) =
        plan_bed_occupied_edits(&state.world_read, &state.blocks, head, true)
            .expect("mixed occupancy still plans the stale half update");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].pos, head);
    assert_eq!(preconditions.len(), 2);

    let outcome = {
        let mut world = state.world.lock().await;
        world.set_block_at(foot, BlockStateId(1)).unwrap();
        world.set_block_at(foot, BlockStateId(3)).unwrap();
        apply_block_edit_batch_to_storage_conditionally(&mut world, None, &edits, &preconditions)
    };

    assert!(
        outcome.is_none(),
        "ABA on the unchanged half must stale the whole bed plan"
    );
    let world = state.world.lock().await;
    assert_eq!(world.get_cached_block(head), Some(BlockStateId(2)));
    assert_eq!(world.get_cached_block(foot), Some(BlockStateId(3)));
}

#[tokio::test]
async fn bed_interaction_planning_does_not_wait_for_world_writer() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            BlockReport {
                id: Identifier::parse("minecraft:red_bed").unwrap(),
                properties: prop_schema(&[("facing", &["north"])]),
                states: vec![state(1, true, &[("facing", "north")])],
            },
        ])
        .unwrap(),
    );
    let state = interaction_state_for_blocks(blocks);
    insert_fluid_test_chunk(&state).await;
    let position = mc_world::BlockPos { x: 3, y: 64, z: 2 };
    state
        .world
        .lock()
        .await
        .set_block_at(position, mc_world::BlockStateId(1))
        .expect("place bed");

    let world = Arc::clone(&state.world);
    let world_writer = world.lock().await;
    let (pose, canonical_bed) =
        plan_loaded_bed_interaction(&state.world_read, &state.blocks, position)
            .expect("published bed should remain interactive");

    assert_eq!((pose.x, pose.y, pose.z, pose.yaw), (3.5, 65.0, 2.5, 180.0));
    assert_eq!(canonical_bed, position);
    drop(world_writer);
}

#[tokio::test]
async fn bed_obstruction_uses_suffocation_above_both_halves() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            BlockReport {
                id: Identifier::parse("minecraft:red_bed").unwrap(),
                properties: prop_schema(&[("facing", &["north"]), ("part", &["head", "foot"])]),
                states: vec![
                    state(1, true, &[("facing", "north"), ("part", "foot")]),
                    state(2, false, &[("facing", "north"), ("part", "head")]),
                ],
            },
            simple_block(3, "minecraft:stone"),
            simple_block(4, "minecraft:barrier"),
            simple_block(5, "minecraft:oak_slab"),
        ])
        .unwrap(),
    );
    let mut interaction = interaction_state_for_blocks(Arc::clone(&blocks));
    interaction.block_light = Some(Arc::new(
        mc_data::block_light::BlockLightTable::from_arrays_with_suffocating(
            "test",
            vec![0; 6],
            vec![0, 0, 0, 15, 0, 15],
            vec![true, true, true, false, true, false],
            vec![false, false, false, true, true, false],
        ),
    ));
    insert_fluid_test_chunk(&interaction).await;
    let head = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    let foot = mc_world::BlockPos { x: 3, y: 64, z: 2 };
    {
        let mut world = interaction.world.lock().await;
        world.set_block_at(head, BlockStateId(2)).unwrap();
        world.set_block_at(foot, BlockStateId(1)).unwrap();
        world
            .set_block_at(mc_world::BlockPos { x: 3, y: 65, z: 2 }, BlockStateId(3))
            .unwrap();
    }

    assert!(bed_sleep_is_obstructed(
        &interaction.world_read,
        &interaction.blocks,
        interaction.block_light.as_deref(),
        head
    ));

    interaction
        .world
        .lock()
        .await
        .set_block_at(mc_world::BlockPos { x: 3, y: 65, z: 2 }, BlockStateId(5))
        .unwrap();
    assert!(!bed_sleep_is_obstructed(
        &interaction.world_read,
        &interaction.blocks,
        interaction.block_light.as_deref(),
        head
    ));

    interaction
        .world
        .lock()
        .await
        .set_block_at(mc_world::BlockPos { x: 3, y: 65, z: 1 }, BlockStateId(4))
        .unwrap();
    assert!(bed_sleep_is_obstructed(
        &interaction.world_read,
        &interaction.blocks,
        interaction.block_light.as_deref(),
        head
    ));
}

#[test]
fn nearby_monster_blocks_survival_sleep_but_not_creative_sleep() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            BlockReport {
                id: Identifier::parse("minecraft:red_bed").unwrap(),
                properties: prop_schema(&[("facing", &["north"])]),
                states: vec![state(1, true, &[("facing", "north")])],
            },
        ])
        .unwrap(),
    );
    let state = interaction_state_for_blocks(blocks);
    let bed = mc_world::BlockPos { x: 0, y: 64, z: 0 };
    state.sessions.spawn_command_entity(
        &simulation::SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_owned(),
        Vec3::new(7.5, 68.5, 7.5),
    );

    let hostile_nearby = state.sessions.has_rest_preventing_hostile_near_bed(bed);
    assert!(bed_sleep_is_blocked_by_monster(
        GameMode::Survival,
        hostile_nearby
    ));
    assert!(!bed_sleep_is_blocked_by_monster(
        GameMode::Creative,
        hostile_nearby
    ));
}

#[tokio::test]
async fn safe_bed_wake_uses_flat_floor_and_skips_unsafe_cells_in_vanilla_order() {
    let reports = vec![
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:red_bed").unwrap(),
            properties: prop_schema(&[("facing", &["north"])]),
            states: vec![state(1, true, &[("facing", "north")])],
        },
        simple_block(2, "minecraft:stone"),
        fluid_block(3, "minecraft:water", 0),
        simple_block(4, "minecraft:campfire"),
    ];
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&reports).unwrap());
    let mut state = interaction_state_for_blocks(blocks);
    state.block_facts = Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
        &reports,
    ));
    insert_fluid_test_chunk(&state).await;
    let bed = mc_world::BlockPos { x: 0, y: 64, z: 0 };
    {
        let mut world = state.world.lock().await;
        for x in -3..=3 {
            for z in -3..=3 {
                world
                    .set_block_at(mc_world::BlockPos { x, y: 63, z }, BlockStateId(2))
                    .unwrap();
            }
        }
        world.set_block_at(bed, BlockStateId(1)).unwrap();
        world
            .set_block_at(mc_world::BlockPos { x: 1, y: 64, z: 0 }, BlockStateId(3))
            .unwrap();
        world
            .set_block_at(mc_world::BlockPos { x: 1, y: 64, z: 1 }, BlockStateId(4))
            .unwrap();
        world
            .set_block_at(mc_world::BlockPos { x: 1, y: 64, z: 2 }, BlockStateId(2))
            .unwrap();
    }
    let sleeping_pose = PlayerPose::new(0.5, 65.0, 0.5);

    let wake = safe_bed_wake_pose(
        &state.world_read,
        &state.blocks,
        &state.block_facts,
        bed,
        sleeping_pose,
    );

    assert_eq!((wake.x, wake.y, wake.z), (0.5, 64.0, 2.5));
    assert!(wake.flags.on_ground);
}

#[tokio::test]
async fn safe_bed_wake_uses_above_head_after_surrounding_candidates_are_blocked() {
    let reports = vec![
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:red_bed").unwrap(),
            properties: prop_schema(&[("facing", &["north"])]),
            states: vec![state(1, true, &[("facing", "north")])],
        },
        simple_block(2, "minecraft:stone"),
    ];
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&reports).unwrap());
    let mut state = interaction_state_for_blocks(blocks);
    state.block_facts = Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
        &reports,
    ));
    insert_fluid_test_chunk(&state).await;
    let bed = mc_world::BlockPos { x: 0, y: 64, z: 0 };
    let foot = mc_world::BlockPos { x: 0, y: 64, z: 1 };
    {
        let mut world = state.world.lock().await;
        for x in -3..=3 {
            for z in -3..=3 {
                world
                    .set_block_at(mc_world::BlockPos { x, y: 63, z }, BlockStateId(2))
                    .unwrap();
                world
                    .set_block_at(mc_world::BlockPos { x, y: 64, z }, BlockStateId(2))
                    .unwrap();
            }
        }
        world.set_block_at(bed, BlockStateId(1)).unwrap();
        world.set_block_at(foot, BlockStateId(1)).unwrap();
    }

    let wake = safe_bed_wake_pose(
        &state.world_read,
        &state.blocks,
        &state.block_facts,
        bed,
        PlayerPose::new(0.5, 65.0, 0.5),
    );

    assert_eq!((wake.x, wake.y, wake.z), (0.5, 65.0, 0.5));
    assert!(wake.flags.on_ground);
}

#[test]
fn sleep_skip_targets_the_next_morning() {
    assert_eq!(next_morning_time(12_542), 24_000);
    assert_eq!(next_morning_time(47_999), 48_000);
    assert_eq!(next_morning_time(u64::MAX), u64::MAX);
}
