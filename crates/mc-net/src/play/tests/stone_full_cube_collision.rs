use super::{
    PlayerPose, player_pose_collides_with_solid, set_collision_test_block,
    vanilla_collision_state_id, vanilla_collision_test_state,
};

#[tokio::test]
async fn player_collision_uses_exact_full_cube_shape_for_stone() {
    let state = vanilla_collision_test_state();
    let stone = vanilla_collision_state_id(&state, "minecraft:stone", &[]);
    set_collision_test_block(&state, stone).await;

    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.0, 0.5)).await,
        "the exact stone shape remains a full cube"
    );
}
