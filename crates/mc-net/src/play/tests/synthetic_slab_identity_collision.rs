use super::{
    PlayerPose, minecraft_synthetic_slab_overlap_test_state, player_pose_collides_with_solid,
    set_collision_test_block,
};

#[tokio::test]
async fn player_collision_rejects_minecraft_synthetic_slab_identity_on_overlapping_id() {
    let (state, synthetic_slab) = minecraft_synthetic_slab_overlap_test_state();
    set_collision_test_block(&state, synthetic_slab).await;

    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.5, 0.5)).await,
        "a synthetic Minecraft slab name must not inherit the overlapping vanilla slab shape"
    );
}
