use std::sync::Arc;

use mc_world::{BlockPos, BlockStateId};

use super::tests::{
    fluid_test_facts, fluid_test_registry, insert_fluid_test_chunk, interaction_state_for_blocks,
};
use super::{PlayerPose, player_water_overlap};
use crate::play::chunk_stream::passable_block_name;

#[test]
fn water_plants_do_not_become_full_cube_collision_fallbacks() {
    for name in [
        "minecraft:kelp",
        "minecraft:kelp_plant",
        "minecraft:seagrass",
        "minecraft:tall_seagrass",
        "minecraft:bubble_column",
    ] {
        assert!(passable_block_name(name), "{name} must be passable");
    }
}

#[test]
fn torches_do_not_become_full_cube_collision_fallbacks() {
    for name in [
        "minecraft:torch",
        "minecraft:wall_torch",
        "minecraft:soul_torch",
        "minecraft:soul_wall_torch",
        "minecraft:redstone_torch",
        "minecraft:redstone_wall_torch",
    ] {
        assert!(passable_block_name(name), "{name} must be passable");
    }
}

#[tokio::test]
async fn swimming_pose_submerges_eyes_in_one_block_of_water() {
    let mut state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    state.block_facts = Arc::new(fluid_test_facts());
    insert_fluid_test_chunk(&state).await;
    state
        .world
        .lock()
        .await
        .set_block_at(BlockPos { x: 0, y: 64, z: 0 }, BlockStateId(2))
        .unwrap();

    let mut pose = PlayerPose::new(0.5, 64.0, 0.5);
    pose.swimming = true;

    assert_eq!(player_water_overlap(&state, pose).await, (true, true));
}
