use super::{
    PlayerPose, player_pose_collides_with_solid, set_collision_test_block,
    vanilla_collision_state_id, vanilla_collision_test_state,
};

#[tokio::test]
async fn powder_snow_uses_falling_collision_shape_after_long_fall() {
    let state = vanilla_collision_test_state();
    let powder_snow = vanilla_collision_state_id(&state, "minecraft:powder_snow", &[]);
    set_collision_test_block(&state, powder_snow).await;

    let mut above_shape = PlayerPose::new(0.5, 64.9, 0.5);
    above_shape.fall_start_y = 68.0;
    assert!(
        !player_pose_collides_with_solid(Some(&state), above_shape).await,
        "the falling collision shape ends at the exact 0.9F boundary"
    );

    let mut inside_shape = PlayerPose::new(0.5, 64.89, 0.5);
    inside_shape.fall_start_y = 68.0;
    assert!(
        player_pose_collides_with_solid(Some(&state), inside_shape).await,
        "a fall longer than 2.5 blocks collides with powder snow's 0.9F shape"
    );
}
