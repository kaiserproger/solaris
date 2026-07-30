use super::{
    PlayerPose, player_pose_collides_with_solid, set_collision_test_block,
    wrong_property_slab_overlap_test_state,
};

#[tokio::test]
async fn player_collision_rejects_wrong_properties_under_canonical_slab_name_and_id() {
    let (state, altered_slab) = wrong_property_slab_overlap_test_state();
    set_collision_test_block(&state, altered_slab).await;

    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.5, 0.5)).await,
        "a canonical name and numeric id are insufficient when ordered properties differ"
    );
}
