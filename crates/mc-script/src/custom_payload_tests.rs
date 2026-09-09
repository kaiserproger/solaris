//! Regression coverage for script custom-payload routing and brand events.

use std::num::NonZeroUsize;

use super::*;

fn nonzero(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).unwrap()
}

fn channel_manifest(id: &str, channel: &str) -> ValidatedScriptPluginManifest {
    ScriptPluginManifest::new(id, id, "0.1.0", SCRIPT_API_VERSION)
        .declare_custom_payload_channel(channel)
        .validate()
        .unwrap()
}

#[test]
fn custom_payload_routes_to_channel_owner_with_moved_bytes() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    endpoint
        .register_plugin_routes(&channel_manifest("owner", "owner:telemetry"))
        .unwrap();

    assert!(boundary.allows_custom_payload("owner:telemetry"));
    assert!(!boundary.allows_custom_payload("other:telemetry"));

    let body = vec![0x00, 0x01, 0x02, 0xff, 0x00, 0x7f];
    assert_eq!(
        boundary.try_enqueue_custom_payload(
            ScriptPlayerId::new(7),
            ScriptProtocolPhase::Play,
            "owner:telemetry",
            body.clone(),
        ),
        Ok(true)
    );

    let event = endpoint.recv_event_blocking().unwrap();
    assert_eq!(event.target_plugin_id(), Some("owner"));
    assert_eq!(event.event_name(), "player.custom_payload");
    let ScriptEventKind::CustomPayload {
        player_id,
        phase,
        channel,
        payload,
    } = event.kind()
    else {
        panic!("expected custom payload event, got {:?}", event.kind());
    };
    assert_eq!(*player_id, ScriptPlayerId::new(7));
    assert_eq!(*phase, ScriptProtocolPhase::Play);
    assert_eq!(channel, "owner:telemetry");
    assert_eq!(*payload, body);
}

#[test]
fn unknown_and_oversized_payloads_report_false_before_retention() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    endpoint
        .register_plugin_routes(&channel_manifest("owner", "owner:telemetry"))
        .unwrap();

    assert_eq!(
        boundary.try_enqueue_custom_payload(
            ScriptPlayerId::new(7),
            ScriptProtocolPhase::Configuration,
            "unknown:channel",
            vec![1, 2, 3],
        ),
        Ok(false)
    );
    assert_eq!(
        boundary.try_enqueue_custom_payload(
            ScriptPlayerId::new(7),
            ScriptProtocolPhase::Play,
            "owner:telemetry",
            vec![0xaa; MAX_SCRIPT_CUSTOM_PAYLOAD_BYTES + 1],
        ),
        Ok(false)
    );

    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    assert!(matches!(
        endpoint.recv_event_blocking().unwrap().kind(),
        ScriptEventKind::ServerStarted
    ));
}

#[test]
fn payload_body_at_host_bound_is_accepted() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    endpoint
        .register_plugin_routes(&channel_manifest("owner", "owner:telemetry"))
        .unwrap();

    assert_eq!(
        boundary.try_enqueue_custom_payload(
            ScriptPlayerId::new(7),
            ScriptProtocolPhase::Play,
            "owner:telemetry",
            vec![0x55; MAX_SCRIPT_CUSTOM_PAYLOAD_BYTES],
        ),
        Ok(true)
    );
    let event = endpoint.recv_event_blocking().unwrap();
    assert!(matches!(
        event.kind(),
        ScriptEventKind::CustomPayload { payload, .. }
            if payload.len() == MAX_SCRIPT_CUSTOM_PAYLOAD_BYTES
    ));
}

#[test]
fn full_queue_reports_backpressure_for_owned_channels() {
    let (boundary, _endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    let manifest = ScriptPluginManifest::new("owner", "owner", "0.1.0", SCRIPT_API_VERSION)
        .declare_custom_payload_channel("owner:telemetry")
        .validate()
        .unwrap();
    _endpoint.register_plugin_routes(&manifest).unwrap();
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    assert_eq!(
        boundary.try_enqueue_custom_payload(
            ScriptPlayerId::new(7),
            ScriptProtocolPhase::Play,
            "owner:telemetry",
            vec![1],
        ),
        Err(ScriptQueueError::Full)
    );
}

#[test]
fn channel_conflicts_reject_atomically_without_stealing_ownership() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    endpoint
        .register_plugin_routes(&channel_manifest("owner", "owner:telemetry"))
        .unwrap();

    let rival = ScriptPluginManifest::new("rival", "rival", "0.1.0", SCRIPT_API_VERSION)
        .declare_player_command_root("rivalcmd")
        .declare_custom_payload_channel("owner:telemetry")
        .validate()
        .unwrap();
    assert_eq!(
        endpoint.register_plugin_routes(&rival),
        Err(ScriptRouteRegistrationError::ChannelConflict {
            channel: "owner:telemetry".to_owned(),
            owner_plugin_id: "owner".to_owned(),
        })
    );

    assert!(boundary.player_command_roots().is_empty());
    assert_eq!(
        boundary.try_enqueue_custom_payload(
            ScriptPlayerId::new(7),
            ScriptProtocolPhase::Play,
            "owner:telemetry",
            vec![9],
        ),
        Ok(true)
    );
    assert_eq!(
        endpoint.recv_event_blocking().unwrap().target_plugin_id(),
        Some("owner")
    );
}

#[test]
fn channel_limit_rejects_atomically() {
    let (boundary, endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    let mut full = ScriptPluginManifest::new("full", "full", "0.1.0", SCRIPT_API_VERSION);
    for index in 0..MAX_PLUGIN_PAYLOAD_CHANNELS {
        full = full.declare_custom_payload_channel(format!("full:channel-{index}"));
    }
    endpoint
        .register_plugin_routes(&full.validate().unwrap())
        .unwrap();

    let overflow = channel_manifest("overflow", "overflow:extra");
    assert_eq!(
        endpoint.register_plugin_routes(&overflow),
        Err(ScriptRouteRegistrationError::ChannelLimitExceeded {
            limit: MAX_PLUGIN_PAYLOAD_CHANNELS,
            requested: MAX_PLUGIN_PAYLOAD_CHANNELS + 1,
        })
    );
    for index in 0..MAX_PLUGIN_PAYLOAD_CHANNELS {
        assert!(boundary.allows_custom_payload(&format!("full:channel-{index}")));
    }
    assert!(!boundary.allows_custom_payload("overflow:extra"));
}

#[test]
fn unregister_releases_channels_and_roots_together() {
    let (boundary, endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    let manifest = ScriptPluginManifest::new("owner", "owner", "0.1.0", SCRIPT_API_VERSION)
        .declare_player_command_root("owned")
        .declare_custom_payload_channel("owner:telemetry")
        .validate()
        .unwrap();
    endpoint.register_plugin_routes(&manifest).unwrap();
    endpoint.unregister_plugin_routes("owner");

    assert!(!boundary.allows_custom_payload("owner:telemetry"));
    assert!(boundary.player_command_roots().is_empty());
    assert_eq!(
        boundary.try_enqueue_custom_payload(
            ScriptPlayerId::new(7),
            ScriptProtocolPhase::Play,
            "owner:telemetry",
            vec![1],
        ),
        Ok(false)
    );
}

#[test]
fn raw_payload_manifest_cannot_claim_loader_control_channels() {
    let manifest = ScriptPluginManifest::new("owner", "owner", "0.1.0", SCRIPT_API_VERSION)
        .declare_custom_payload_channel("solaris:loader/ui")
        .validate();
    assert!(matches!(
        manifest,
        Err(ScriptPluginManifestError::InvalidCustomPayloadChannel { .. })
    ));
}

#[test]
fn raw_payload_command_cannot_bypass_loader_ui_authorization() {
    let channel = "solaris:loader/ui";
    let capabilities = CommandCapabilities::default().allow_custom_payload_channel(channel);
    let mut batch = CommandBatch::new(nonzero(1));
    assert!(matches!(
        batch.try_push_authorized(
            ScriptCommand::SendCustomPayload {
                player_id: ScriptPlayerId::new(7),
                channel: channel.to_owned(),
                payload: vec![0],
            },
            &capabilities,
        ),
        Err(CommandBatchError::InvalidCommand { .. })
    ));
    assert!(batch.commands().is_empty());
}

#[test]
fn send_command_requires_the_exact_declared_channel() {
    let manifest = channel_manifest("owner", "owner:telemetry");
    let capabilities = manifest.to_command_capabilities();

    let denied = ScriptCommand::SendCustomPayload {
        player_id: ScriptPlayerId::new(7),
        channel: "other:telemetry".to_owned(),
        payload: vec![1, 2, 3],
    };
    assert_eq!(
        denied.required_capability_kind(),
        Some(ScriptCommandCapabilityKind::CustomPayloadChannel)
    );
    assert_eq!(
        ScriptCommandCapabilityKind::CustomPayloadChannel.code(),
        "custom_payload"
    );
    let mut batch = CommandBatch::new(nonzero(2));
    assert_eq!(
        batch.try_push_authorized(denied, &capabilities),
        Err(CommandBatchError::PermissionDenied {
            capability: ScriptCommandCapabilityKind::CustomPayloadChannel,
        })
    );
    assert!(batch.commands().is_empty());

    let allowed = ScriptCommand::SendCustomPayload {
        player_id: ScriptPlayerId::new(7),
        channel: "owner:telemetry".to_owned(),
        payload: vec![0xde, 0xad, 0xbe, 0xef],
    };
    batch
        .try_push_authorized(allowed.clone(), &capabilities)
        .unwrap();
    assert_eq!(batch.commands(), &[allowed]);

    let mut bare = CommandBatch::new(nonzero(1));
    assert_eq!(
        bare.try_push(ScriptCommand::SendCustomPayload {
            player_id: ScriptPlayerId::new(7),
            channel: "owner:telemetry".to_owned(),
            payload: vec![1],
        }),
        Err(CommandBatchError::PermissionDenied {
            capability: ScriptCommandCapabilityKind::CustomPayloadChannel,
        })
    );

    let oversized = ScriptCommand::SendCustomPayload {
        player_id: ScriptPlayerId::new(7),
        channel: "owner:telemetry".to_owned(),
        payload: vec![0xaa; MAX_SCRIPT_CUSTOM_PAYLOAD_BYTES + 1],
    };
    let mut oversized_batch = CommandBatch::new(nonzero(1));
    assert!(matches!(
        oversized_batch.try_push_authorized(oversized, &capabilities),
        Err(CommandBatchError::InvalidCommand { .. })
    ));
}

#[test]
fn client_brand_preserves_empty_and_bounds_bytes() {
    let empty = ScriptEvent::client_brand(ScriptPlayerId::new(7), "").unwrap();
    assert_eq!(empty.event_name(), "player.client_brand");
    assert!(matches!(
        empty.kind(),
        ScriptEventKind::ClientBrand { brand, .. } if brand.is_empty()
    ));
    empty.validate().unwrap();

    let oversized = "x".repeat(MAX_SCRIPT_CUSTOM_PAYLOAD_BYTES + 1);
    assert!(matches!(
        ScriptEvent::client_brand(ScriptPlayerId::new(7), &oversized),
        Err(ScriptDtoError::ValueTooLong { .. })
    ));
}

#[test]
fn channel_manifest_validation_rejects_malformed_and_duplicate_channels() {
    for channel in [
        "no-colon",
        "UPPER:path",
        "owner:",
        ":path",
        "owner:has:colon",
    ] {
        let manifest = ScriptPluginManifest::new("owner", "owner", "0.1.0", SCRIPT_API_VERSION)
            .declare_custom_payload_channel(channel);
        assert!(
            manifest.validate().is_err(),
            "channel {channel:?} must be rejected"
        );
    }

    let duplicate = ScriptPluginManifest::new("owner", "owner", "0.1.0", SCRIPT_API_VERSION)
        .declare_custom_payload_channel("owner:telemetry")
        .declare_custom_payload_channel("owner:telemetry")
        .validate();
    assert!(matches!(
        duplicate,
        Err(ScriptPluginManifestError::DuplicateCapability { .. })
    ));
}

#[test]
fn root_and_channel_routes_share_one_atomic_lifecycle() {
    let (boundary, endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    let manifest = ScriptPluginManifest::new("owner", "owner", "0.1.0", SCRIPT_API_VERSION)
        .declare_player_command_root("owned")
        .declare_custom_payload_channel("owner:telemetry")
        .validate()
        .unwrap();
    endpoint.register_plugin_routes(&manifest).unwrap();
    assert_eq!(boundary.player_command_roots(), vec!["owned".to_owned()]);
    assert!(boundary.allows_custom_payload("owner:telemetry"));

    boundary.close_event_admission();
    assert!(boundary.player_command_roots().is_empty());
    assert!(!boundary.allows_custom_payload("owner:telemetry"));
    assert_eq!(
        boundary.try_enqueue_custom_payload(
            ScriptPlayerId::new(7),
            ScriptProtocolPhase::Play,
            "owner:telemetry",
            vec![1],
        ),
        Ok(false)
    );
    assert_eq!(
        endpoint.register_plugin_routes(&manifest),
        Err(ScriptRouteRegistrationError::AuthorityPoisoned)
    );
}

#[cfg(feature = "lua-runtime")]
#[test]
fn reload_replaces_both_route_kinds_atomically() {
    let (boundary, endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    endpoint
        .register_plugin_routes(&channel_manifest("owner", "owner:telemetry"))
        .unwrap();

    let clashing = vec![
        channel_manifest("alpha", "shared:channel"),
        channel_manifest("beta", "shared:channel"),
    ];
    assert!(matches!(
        endpoint.plugin_routes.replace_all(&clashing),
        Err(ScriptRouteRegistrationError::ChannelConflict { .. })
    ));
    assert!(boundary.allows_custom_payload("owner:telemetry"));

    let replacement = vec![
        ScriptPluginManifest::new("next", "next", "0.1.0", SCRIPT_API_VERSION)
            .declare_player_command_root("nextcmd")
            .declare_custom_payload_channel("next:telemetry")
            .validate()
            .unwrap(),
    ];
    endpoint.plugin_routes.replace_all(&replacement).unwrap();
    assert!(!boundary.allows_custom_payload("owner:telemetry"));
    assert!(boundary.allows_custom_payload("next:telemetry"));
    assert_eq!(boundary.player_command_roots(), vec!["nextcmd".to_owned()]);
}
