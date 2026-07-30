use super::{
    PlayerPose, fake_farmland_slab_overlap_test_state, player_pose_collides_with_solid,
    set_collision_test_block,
};

#[tokio::test]
async fn player_collision_rejects_fake_farmland_identity_on_overlapping_slab_id() {
    let (state, fake_farmland) = fake_farmland_slab_overlap_test_state();
    set_collision_test_block(&state, fake_farmland).await;

    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.5, 0.5)).await,
        "fake farmland properties must neither inherit the slab table shape nor farmland height"
    );
}
