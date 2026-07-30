use super::{
    PlayerPose, player_pose_collides_with_solid, set_collision_test_block,
    vanilla_collision_state_id, vanilla_collision_test_state,
};

#[tokio::test]
async fn player_collision_uses_bottom_slab_box() {
    let state = vanilla_collision_test_state();
    let slab = vanilla_collision_state_id(
        &state,
        "minecraft:stone_slab",
        &[("type", "bottom"), ("waterlogged", "false")],
    );
    set_collision_test_block(&state, slab).await;

    assert!(
        !player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.5, 0.5)).await,
        "a player may stand on the bottom slab's half-block top"
    );
    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.49, 0.5)).await,
        "a player may not overlap the bottom slab box"
    );
}
