use super::{
    ChunkPipelineResources, CommandPermissions, Compression, GameMode, PendingTeleport, PlayerPose,
    SessionRegistry, SurvivalState, XpState, decode_player_position_sync_packets,
    execute_player_command, play_loop_slow_client_test_config, simulation_channel,
};

#[tokio::test]
async fn teleport_command_waits_for_pending_confirmation_before_repositioning_player() {
    let config = play_loop_slow_client_test_config();
    let sessions = SessionRegistry::new();
    let (simulation, _simulation_owner) = simulation_channel();
    let mut writer = Vec::new();
    let mut game_mode = GameMode::Survival;
    let mut survival_state = SurvivalState::FULL;
    let mut xp_state = XpState::default();
    let original_pose = PlayerPose::new(1.0, 65.0, 2.0);
    let mut player_pose = original_pose;
    let mut next_teleport_id = 8;
    let mut pending_teleport = Some(PendingTeleport::new(7, 0));
    let mut chunk_stream = None;
    let chunk_pipeline_resources = ChunkPipelineResources::with_limits(1, 1);

    execute_player_command(
        &mut writer,
        Compression::Disabled,
        "/tp 10 70 -5",
        CommandPermissions::CONSOLE,
        &mut game_mode,
        &mut survival_state,
        &mut xp_state,
        &config,
        &sessions,
        &simulation,
        None,
        &mut player_pose,
        None,
        &chunk_pipeline_resources,
        &mut chunk_stream,
        &mut next_teleport_id,
        &mut pending_teleport,
    )
    .await
    .unwrap();

    assert!(
        decode_player_position_sync_packets(&writer).is_empty(),
        "teleport commands must not issue a newer position sync while an earlier teleport is still pending"
    );
    assert_eq!(pending_teleport.unwrap().id, 7);
    assert_eq!(next_teleport_id, 8);
    assert_eq!(player_pose.x, original_pose.x);
    assert_eq!(player_pose.y, original_pose.y);
    assert_eq!(player_pose.z, original_pose.z);
    assert_eq!(player_pose.yaw, original_pose.yaw);
    assert_eq!(player_pose.pitch, original_pose.pitch);
}
