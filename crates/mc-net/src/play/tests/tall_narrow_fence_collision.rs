use super::{
    PlayerPose, player_pose_collides_with_solid, set_collision_test_block,
    vanilla_collision_state_id, vanilla_collision_test_state,
};

#[tokio::test]
async fn player_collision_uses_tall_narrow_fence_box() {
    let state = vanilla_collision_test_state();
    let fence = vanilla_collision_state_id(
        &state,
        "minecraft:oak_fence",
        &[
            ("east", "false"),
            ("north", "false"),
            ("south", "false"),
            ("west", "false"),
            ("waterlogged", "false"),
        ],
    );
    set_collision_test_block(&state, fence).await;

    assert!(
        !player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.05, 64.0, 0.5)).await,
        "space beside an isolated fence post is empty"
    );
    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 65.25, 0.5)).await,
        "the isolated fence post collision extends to 1.5 blocks"
    );
}
