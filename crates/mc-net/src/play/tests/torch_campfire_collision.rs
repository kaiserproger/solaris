use super::{
    PlayerPose, player_pose_collides_with_solid, set_collision_test_block,
    vanilla_collision_state_id, vanilla_collision_test_state,
};

#[tokio::test]
async fn player_collision_uses_exact_shapes_for_torch_and_campfire() {
    let state = vanilla_collision_test_state();
    let torch = vanilla_collision_state_id(&state, "minecraft:torch", &[]);
    set_collision_test_block(&state, torch).await;
    assert!(
        !player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.0, 0.5)).await,
        "the empty torch collision shape must come from the embedded table"
    );

    let campfire = vanilla_collision_state_id(
        &state,
        "minecraft:campfire",
        &[
            ("facing", "north"),
            ("lit", "true"),
            ("signal_fire", "false"),
            ("waterlogged", "false"),
        ],
    );
    set_collision_test_block(&state, campfire).await;
    assert!(
        !player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.4375, 0.5)).await,
        "the player may stand on the campfire's exact 7/16-block top"
    );
    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.42, 0.5)).await,
        "the campfire body must collide below its exact top"
    );
}
