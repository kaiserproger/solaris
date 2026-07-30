use super::{
    PlayerPose, player_pose_collides_with_solid, set_collision_test_block,
    vanilla_collision_state_id, vanilla_collision_test_state,
};

#[tokio::test]
async fn player_collision_scans_fence_below_at_deflated_top_boundary() {
    let state = vanilla_collision_test_state();
    let fence = vanilla_collision_state_id(
        &state,
        "minecraft:oak_fence",
        &[
            ("east", "false"),
            ("north", "false"),
            ("south", "false"),
            ("west", "false"),
            ("waterlogged", "false"),
        ],
    );
    set_collision_test_block(&state, fence).await;
    let oracle_deflation = f64::from(1.0e-5_f32);

    assert!(
        !player_pose_collides_with_solid(
            Some(&state),
            PlayerPose::new(0.5, 65.5 - oracle_deflation / 2.0, 0.5),
        )
        .await,
        "sub-boundary overlap with the fence top is deflated away"
    );
    assert!(
        player_pose_collides_with_solid(
            Some(&state),
            PlayerPose::new(0.5, 65.5 - oracle_deflation * 2.0, 0.5),
        )
        .await,
        "the minimum Y scan must retain the 1.5-block fence below"
    );
}
