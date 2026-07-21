use std::num::NonZeroUsize;

use super::*;

#[tokio::test]
async fn admitted_player_teleport_is_capability_gated_and_builds_targeted_result() {
    let request = ScriptPlayerTeleportRequest::try_new(
        "warp-home",
        ScriptPlayerId::new(7),
        ScriptPosition::try_new(12.5, 70.0, -4.5).unwrap(),
    )
    .unwrap();
    assert_eq!(request.request_id(), "warp-home");
    assert_eq!(request.player_id(), ScriptPlayerId::new(7));
    assert_eq!(
        request.position(),
        ScriptPosition::try_new(12.5, 70.0, -4.5).unwrap()
    );

    let command = ScriptCommand::TeleportPlayer {
        request: request.clone(),
    };
    assert_eq!(
        command.required_capability_kind(),
        Some(ScriptCommandCapabilityKind::PlayerTeleport)
    );
    let mut denied = CommandBatch::new(NonZeroUsize::new(1).unwrap());
    assert_eq!(
        denied.try_push_authorized(command.clone(), &CommandCapabilities::default()),
        Err(CommandBatchError::PermissionDenied {
            capability: ScriptCommandCapabilityKind::PlayerTeleport,
        })
    );
    assert!(denied.commands().is_empty());

    let manifest = ScriptPluginManifest::new("warps", "Warps", "0.1.0", SCRIPT_API_VERSION)
        .declare_player_teleport()
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
    let result = admitted.player_teleport_result(None).unwrap();
    assert_eq!(result.target_plugin_id(), Some("warps"));
    assert_eq!(result.event_name(), "player.teleport_result");
    assert!(matches!(
        result.kind(),
        ScriptEventKind::PlayerTeleportResult {
            request_id,
            player_id,
            position,
            failure: None,
        } if request_id == "warp-home"
            && *player_id == ScriptPlayerId::new(7)
            && *position == ScriptPosition::try_new(12.5, 70.0, -4.5).unwrap()
    ));
    for (failure, code) in [
        (
            ScriptPlayerTeleportFailure::PlayerUnavailable,
            "player_unavailable",
        ),
        (
            ScriptPlayerTeleportFailure::TeleportPending,
            "teleport_pending",
        ),
        (
            ScriptPlayerTeleportFailure::RuntimeUnavailable,
            "runtime_unavailable",
        ),
    ] {
        assert_eq!(failure.as_str(), code);
    }
}

#[test]
fn player_teleport_request_rejects_invalid_request_ids() {
    let position = ScriptPosition::try_new(0.0, 64.0, 0.0).unwrap();
    for request_id in ["", "contains space", &"x".repeat(MAX_SCRIPT_ID_BYTES + 1)] {
        assert!(
            ScriptPlayerTeleportRequest::try_new(request_id, ScriptPlayerId::new(1), position,)
                .is_err(),
            "accepted request id {request_id:?}"
        );
    }
}
