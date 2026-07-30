use super::{
    PlayerPose, player_pose_collides_with_solid, set_collision_test_block,
    vanilla_collision_state_id, vanilla_collision_test_state,
};

#[tokio::test]
async fn player_collision_uses_top_slab_box() {
    let state = vanilla_collision_test_state();
    let slab = vanilla_collision_state_id(
        &state,
        "minecraft:stone_slab",
        &[("type", "top"), ("waterlogged", "false")],
    );
    set_collision_test_block(&state, slab).await;

    assert!(
        !player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 62.7, 0.5)).await,
        "the lower half below a top slab is empty"
    );
    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 62.71, 0.5)).await,
        "the player's head may not enter the top slab box"
    );
}
