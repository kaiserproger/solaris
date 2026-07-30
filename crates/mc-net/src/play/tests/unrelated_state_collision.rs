use super::{
    PlayerPose, player_pose_collides_with_solid, set_collision_test_block,
    synthetic_collision_overlap_test_state,
};

#[tokio::test]
async fn player_collision_does_not_apply_vanilla_shape_to_unrelated_overlapping_state_id() {
    let (state, synthetic_solid) = synthetic_collision_overlap_test_state();
    assert!(
        mc_data::collision_shapes::vanilla_collision_shapes()
            .get(synthetic_solid.0)
            .is_some(),
        "the synthetic state must overlap a covered vanilla state id"
    );
    set_collision_test_block(&state, synthetic_solid).await;

    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.5, 0.5)).await,
        "an unrelated synthetic solid keeps full-cube collision despite its overlapping state id"
    );
}
