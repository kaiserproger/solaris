use super::{
    PlayerPose, low_id_exact_farmland_test_state, player_pose_collides_with_solid,
    set_collision_test_block,
};

#[tokio::test]
async fn player_collision_uses_farmland_fallback_for_exact_low_id_semantics() {
    let (state, farmland) = low_id_exact_farmland_test_state();
    set_collision_test_block(&state, farmland).await;

    assert!(
        !player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.9375, 0.5)).await,
        "exact farmland semantics retain the direct 15/16 fallback on a noncanonical id"
    );
    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.90, 0.5)).await,
        "the exact farmland fallback still rejects overlap below its top"
    );
}
