use std::num::NonZeroUsize;

use super::*;

#[tokio::test]
async fn admitted_player_query_is_bounded_capability_gated_and_targeted() {
    let request = ScriptOnlinePlayersRequest::try_new("catalog-viewers", 2).unwrap();
    let command = ScriptCommand::ListOnlinePlayers {
        request: request.clone(),
    };
    assert_eq!(
        command.required_capability_kind(),
        Some(ScriptCommandCapabilityKind::PlayerQueries)
    );
    let mut denied = CommandBatch::new(NonZeroUsize::new(1).unwrap());
    assert!(matches!(
        denied.try_push_authorized(command.clone(), &CommandCapabilities::default()),
        Err(CommandBatchError::PermissionDenied {
            capability: ScriptCommandCapabilityKind::PlayerQueries,
        })
    ));

    let manifest = ScriptPluginManifest::new("catalog", "Catalog", "0.1.0", SCRIPT_API_VERSION)
        .declare_player_queries()
        .validate()
        .unwrap();
    let admission = HostCommandAdmission::from_manifest(&manifest);
    let (boundary, endpoint) =
        script_boundary_pair(NonZeroUsize::new(1).unwrap(), NonZeroUsize::new(1).unwrap());
    let mut batch = CommandBatch::new(NonZeroUsize::new(1).unwrap());
    batch
        .try_push_authorized(command, &manifest.to_command_capabilities())
        .unwrap();
    endpoint.try_submit_plugin_batch(&admission, batch).unwrap();
    let admitted = boundary
        .accept_host_command(boundary.recv_command().await.unwrap())
        .unwrap();
    let player = ScriptOnlinePlayerSnapshot::try_new(
        ScriptPlayerId::new(7),
        ScriptPlayerContext::new(
            "00000000-0000-0000-0000-000000000007",
            "Alice",
            false,
            1.0,
            64.0,
            2.0,
        ),
        "minecraft:overworld",
    )
    .unwrap();
    let result = admitted.online_players_result(vec![player], true).unwrap();
    assert_eq!(result.target_plugin_id(), Some("catalog"));
    assert!(matches!(
        result.kind(),
        ScriptEventKind::OnlinePlayersResult { request_id, players, truncated }
            if request_id == "catalog-viewers"
                && players[0].player_id() == ScriptPlayerId::new(7)
                && *truncated
    ));
}

#[test]
fn player_query_rejects_zero_and_oversized_limits() {
    assert!(ScriptOnlinePlayersRequest::try_new("players", 0).is_err());
    assert!(
        ScriptOnlinePlayersRequest::try_new("players", MAX_ONLINE_PLAYER_QUERY_LIMIT + 1).is_err()
    );
}
