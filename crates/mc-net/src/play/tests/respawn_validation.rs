use std::sync::Arc;

use mc_world::BlockStateId;

use super::{PlayerPose, insert_fluid_test_chunk, interaction_state_for_blocks, simple_block};
use crate::play::command_execution::validate_respawn_pose;
use crate::play::spawn::validated_respawn_y;

fn stone_fixture() -> crate::play::InteractionState {
    let reports = vec![
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:stone"),
    ];
    let mut state = interaction_state_for_blocks(Arc::new(
        mc_world::BlockRegistry::from_report(&reports).unwrap(),
    ));
    state.block_facts = Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
        &reports,
    ));
    state.block_light = Some(Arc::new(
        mc_data::block_light::BlockLightTable::from_arrays(
            "test",
            vec![0, 0],
            vec![0, 15],
            vec![true, false],
        ),
    ));
    state
}

async fn stone_surface_fixture() -> crate::play::InteractionState {
    let state = stone_fixture();
    insert_fluid_test_chunk(&state).await;
    state
        .world
        .lock()
        .await
        .set_block_at(mc_world::BlockPos { x: 0, y: 64, z: 0 }, BlockStateId(1))
        .unwrap();
    state
}

#[tokio::test]
async fn respawn_column_validation_rebuilds_heightmap_and_recomputes_surface() {
    let state = stone_surface_fixture().await;
    assert_eq!(
        validated_respawn_y(
            &state.blocks,
            &state.block_facts,
            state.block_light.as_deref(),
            &state.world_read,
            0,
            0,
        ),
        Some(66),
        "the validated column must land two blocks above the live surface"
    );
}

#[tokio::test]
async fn respawn_pose_validation_snaps_stale_y_and_falls_back_to_world_spawn() {
    let state = stone_surface_fixture().await;

    // Terrain moved since the pose was stored: the Y snaps to the live
    // surface at the same X/Z.
    let validated = validate_respawn_pose(&state, PlayerPose::new(0.5, 90.0, 0.5));
    assert_eq!(
        (validated.x, validated.y, validated.z),
        (0.5, 66.0, 0.5),
        "a stale stored Y must be replaced with the current surface"
    );

    // A column over a non-resident chunk cannot be validated: the fallback
    // re-resolves the resident world spawn instead of replaying the stale Y.
    let validated = validate_respawn_pose(&state, PlayerPose::new(4096.5, 90.0, 4096.5));
    assert_eq!(validated.y, 66.0, "the world-spawn fallback must validate");
}
