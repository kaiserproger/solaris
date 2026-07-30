use super::{
    PlayerPose, player_pose_collides_with_solid, set_collision_test_block,
    vanilla_collision_state_id, vanilla_collision_test_state,
};

#[tokio::test]
async fn player_collision_uses_oracle_aabb_deflation_boundary() {
    let state = vanilla_collision_test_state();
    let slab = vanilla_collision_state_id(
        &state,
        "minecraft:stone_slab",
        &[("type", "bottom"), ("waterlogged", "false")],
    );
    set_collision_test_block(&state, slab).await;
    let oracle_deflation = f64::from(1.0e-5_f32);

    assert!(
        !player_pose_collides_with_solid(
            Some(&state),
            PlayerPose::new(0.5, 64.5 - oracle_deflation / 2.0, 0.5),
        )
        .await,
        "an overlap below the oracle deflation remains non-colliding"
    );
    assert!(
        player_pose_collides_with_solid(
            Some(&state),
            PlayerPose::new(0.5, 64.5 - oracle_deflation * 2.0, 0.5),
        )
        .await,
        "an overlap beyond the oracle deflation collides"
    );
}
