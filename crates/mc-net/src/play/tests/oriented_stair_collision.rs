use super::{
    PlayerPose, player_pose_collides_with_solid, set_collision_test_block,
    vanilla_collision_state_id, vanilla_collision_test_state,
};

#[tokio::test]
async fn player_collision_uses_oriented_stair_boxes() {
    let state = vanilla_collision_test_state();
    let stair = vanilla_collision_state_id(
        &state,
        "minecraft:oak_stairs",
        &[
            ("facing", "north"),
            ("half", "bottom"),
            ("shape", "straight"),
            ("waterlogged", "false"),
        ],
    );
    set_collision_test_block(&state, stair).await;

    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.5, 0.15)).await,
        "the north stair's upper step occupies its north half"
    );
    assert!(
        !player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.5, 0.85)).await,
        "the south half above a north stair's lower step is empty"
    );
}
