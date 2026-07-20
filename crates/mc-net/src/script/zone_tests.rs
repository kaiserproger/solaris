use std::num::NonZeroUsize;

use mc_script::{
    AdmittedScriptCommand, LuaHostConfig, ScriptEvent, ScriptEventKind, ScriptPlayerContext,
    ScriptPlayerId, start_lua_host,
};

use super::zone::{
    PluginZoneAdapter, ZoneAdapterError, ZoneCapacity, ZoneCommandOutcome, ZoneLimits,
    ZoneObservationOutcome,
};
use crate::server::ScriptEventSink;

async fn admitted_zone_commands(
    plugin_id: &str,
    commands: &str,
    command_count: usize,
) -> Vec<AdmittedScriptCommand> {
    let plugins = tempfile::tempdir().unwrap();
    let plugin = plugins.path().join(plugin_id);
    std::fs::create_dir(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        format!(
            r#"id = "{plugin_id}"
name = "Zone test"
version = "0.1.0"
api = "0.6.0"
events = ["server.started"]
capabilities = ["zones"]
"#,
        ),
    )
    .unwrap();
    std::fs::write(
        plugin.join("main.lua"),
        format!(
            r#"
function on_server_started(_event)
{commands}
end
"#,
        ),
    )
    .unwrap();

    let (boundary, host) = start_lua_host(LuaHostConfig::new(plugins.path())).unwrap();
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    let mut admitted = Vec::with_capacity(command_count);
    for _ in 0..command_count {
        let command = boundary.recv_command().await.unwrap();
        admitted.push(boundary.accept_host_command(command).unwrap());
    }
    drop(boundary);
    tokio::task::spawn_blocking(move || host.join())
        .await
        .unwrap()
        .unwrap();
    admitted
}

fn adapter_with_limits(limits: ZoneLimits) -> (PluginZoneAdapter, mc_script::ScriptHostEndpoint) {
    let (boundary, endpoint) = mc_script::script_boundary_pair(
        NonZeroUsize::new(16).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    );
    (
        PluginZoneAdapter::with_limits_for_test(ScriptEventSink::new(boundary), limits),
        endpoint,
    )
}

fn context(x: f64, y: f64, z: f64) -> ScriptPlayerContext {
    ScriptPlayerContext::new("zone-player-uuid", "ZonePlayer", false, x, y, z)
}

#[tokio::test]
async fn admitted_zone_commands_remain_scoped_to_their_exact_owner() {
    let mut owner_a = admitted_zone_commands(
        "owner-a",
        r#"
    solaris.upsert_zone("shared", "minecraft:overworld", 0, 0, 0, 10, 10, 10)
    solaris.remove_zone("shared")
"#,
        2,
    )
    .await;
    let mut owner_b = admitted_zone_commands(
        "owner-b",
        r#"    solaris.upsert_zone("shared", "minecraft:overworld", 0, 0, 0, 10, 10, 10)"#,
        1,
    )
    .await;
    let (adapter, mut events) = adapter_with_limits(ZoneLimits::production());

    assert_eq!(
        adapter.route_admitted(owner_a.remove(0)),
        Ok(ZoneCommandOutcome::Applied)
    );
    assert_eq!(
        adapter.route_admitted(owner_b.remove(0)),
        Ok(ZoneCommandOutcome::Applied)
    );
    assert_eq!(
        adapter.route_admitted(owner_a.remove(0)),
        Ok(ZoneCommandOutcome::Applied)
    );
    assert_eq!(
        adapter
            .observe_player(
                ScriptPlayerId::new(7),
                1,
                "minecraft:overworld",
                context(5.0, 5.0, 5.0),
            )
            .await,
        Ok(ZoneObservationOutcome::Changed {
            entered: 1,
            exited: 0,
        })
    );

    let event = events.recv_event().await.unwrap();
    assert_eq!(event.target_plugin_id(), Some("owner-b"));
    assert!(matches!(
        event.kind(),
        ScriptEventKind::PlayerZoneEntered {
            player_id,
            zone_id,
            ..
        } if *player_id == ScriptPlayerId::new(7) && zone_id == "shared"
    ));
}

#[tokio::test]
async fn zone_capacity_failure_is_explicit_and_does_not_partially_mutate() {
    let mut commands = admitted_zone_commands(
        "owner",
        r#"
    solaris.upsert_zone("first", "minecraft:overworld", 0, 0, 0, 10, 10, 10)
    solaris.upsert_zone("second", "minecraft:overworld", 0, 0, 0, 10, 10, 10)
"#,
        2,
    )
    .await;
    let limits = ZoneLimits {
        total_zones: 1,
        zones_per_plugin: 1,
        tracked_players: 4,
        memberships: 4,
    };
    let (adapter, mut events) = adapter_with_limits(limits);

    assert_eq!(
        adapter.route_admitted(commands.remove(0)),
        Ok(ZoneCommandOutcome::Applied)
    );
    assert_eq!(
        adapter.route_admitted(commands.remove(0)),
        Err(ZoneAdapterError::Full(ZoneCapacity::TotalZones))
    );
    assert_eq!(
        adapter
            .observe_player(
                ScriptPlayerId::new(1),
                1,
                "minecraft:overworld",
                context(5.0, 5.0, 5.0),
            )
            .await,
        Ok(ZoneObservationOutcome::Changed {
            entered: 1,
            exited: 0,
        })
    );
    let event = events.recv_event().await.unwrap();
    assert!(matches!(
        event.kind(),
        ScriptEventKind::PlayerZoneEntered { zone_id, .. } if zone_id == "first"
    ));
}

#[tokio::test]
async fn per_plugin_zone_capacity_is_independent_of_total_capacity() {
    let mut owner = admitted_zone_commands(
        "owner",
        r#"
    solaris.upsert_zone("first", "minecraft:overworld", 0, 0, 0, 1, 1, 1)
    solaris.upsert_zone("second", "minecraft:overworld", 0, 0, 0, 1, 1, 1)
"#,
        2,
    )
    .await;
    let mut other = admitted_zone_commands(
        "other",
        r#"    solaris.upsert_zone("first", "minecraft:overworld", 0, 0, 0, 1, 1, 1)"#,
        1,
    )
    .await;
    let limits = ZoneLimits {
        total_zones: 2,
        zones_per_plugin: 1,
        tracked_players: 1,
        memberships: 1,
    };
    let (adapter, _events) = adapter_with_limits(limits);

    assert_eq!(
        adapter.route_admitted(owner.remove(0)),
        Ok(ZoneCommandOutcome::Applied)
    );
    assert_eq!(
        adapter.route_admitted(owner.remove(0)),
        Err(ZoneAdapterError::Full(ZoneCapacity::ZonesPerPlugin))
    );
    assert_eq!(
        adapter.route_admitted(other.remove(0)),
        Ok(ZoneCommandOutcome::Applied)
    );
}

#[tokio::test]
async fn duplicate_upsert_and_missing_remove_are_explicit_no_ops() {
    let mut commands = admitted_zone_commands(
        "owner",
        r#"
    solaris.upsert_zone("same", "minecraft:overworld", 0, 0, 0, 1, 1, 1)
    solaris.upsert_zone("same", "minecraft:overworld", 0, 0, 0, 1, 1, 1)
    solaris.remove_zone("missing")
"#,
        3,
    )
    .await;
    let (adapter, _events) = adapter_with_limits(ZoneLimits::production());

    assert_eq!(
        adapter.route_admitted(commands.remove(0)),
        Ok(ZoneCommandOutcome::Applied)
    );
    assert_eq!(
        adapter.route_admitted(commands.remove(0)),
        Ok(ZoneCommandOutcome::NoOp)
    );
    assert_eq!(
        adapter.route_admitted(commands.remove(0)),
        Ok(ZoneCommandOutcome::NoOp)
    );
}

#[tokio::test]
async fn changed_zone_keeps_membership_when_player_remains_inside() {
    let mut commands = admitted_zone_commands(
        "owner",
        r#"
    solaris.upsert_zone("market", "minecraft:overworld", 0, 0, 0, 10, 10, 10)
    solaris.upsert_zone("market", "minecraft:overworld", -5, -5, -5, 15, 15, 15)
"#,
        2,
    )
    .await;
    let (adapter, mut events) = adapter_with_limits(ZoneLimits::production());
    assert_eq!(
        adapter.route_admitted(commands.remove(0)),
        Ok(ZoneCommandOutcome::Applied)
    );
    assert_eq!(
        adapter
            .observe_player(
                ScriptPlayerId::new(1),
                1,
                "minecraft:overworld",
                context(5.0, 5.0, 5.0),
            )
            .await,
        Ok(ZoneObservationOutcome::Changed {
            entered: 1,
            exited: 0,
        })
    );
    assert!(matches!(
        events.recv_event().await.unwrap().kind(),
        ScriptEventKind::PlayerZoneEntered { zone_id, .. } if zone_id == "market"
    ));

    assert_eq!(
        adapter.route_admitted(commands.remove(0)),
        Ok(ZoneCommandOutcome::Applied)
    );
    assert_eq!(
        adapter
            .observe_player(
                ScriptPlayerId::new(1),
                2,
                "minecraft:overworld",
                context(5.0, 5.0, 5.0),
            )
            .await,
        Ok(ZoneObservationOutcome::NoOp)
    );
}

#[tokio::test]
async fn membership_transitions_are_deterministic_once_only_and_stale_fenced() {
    let mut commands = admitted_zone_commands(
        "owner",
        r#"
    solaris.upsert_zone("zulu", "minecraft:overworld", 0, 0, 0, 10, 10, 10)
    solaris.upsert_zone("alpha", "minecraft:overworld", 0, 0, 0, 10, 10, 10)
"#,
        2,
    )
    .await;
    let (adapter, mut events) = adapter_with_limits(ZoneLimits::production());
    for command in commands.drain(..) {
        assert_eq!(
            adapter.route_admitted(command),
            Ok(ZoneCommandOutcome::Applied)
        );
    }

    assert_eq!(
        adapter
            .observe_player(
                ScriptPlayerId::new(1),
                10,
                "minecraft:overworld",
                context(5.0, 5.0, 5.0),
            )
            .await,
        Ok(ZoneObservationOutcome::Changed {
            entered: 2,
            exited: 0,
        })
    );
    for expected in ["alpha", "zulu"] {
        assert!(matches!(
            events.recv_event().await.unwrap().kind(),
            ScriptEventKind::PlayerZoneEntered { zone_id, .. } if zone_id == expected
        ));
    }
    assert_eq!(
        adapter
            .observe_player(
                ScriptPlayerId::new(1),
                10,
                "minecraft:overworld",
                context(20.0, 5.0, 5.0),
            )
            .await,
        Err(ZoneAdapterError::Stale {
            current_revision: 10,
        })
    );
    assert_eq!(
        adapter
            .observe_player(
                ScriptPlayerId::new(1),
                11,
                "minecraft:overworld",
                context(5.0, 5.0, 5.0),
            )
            .await,
        Ok(ZoneObservationOutcome::NoOp)
    );
    assert_eq!(
        adapter
            .observe_player(
                ScriptPlayerId::new(1),
                12,
                "minecraft:overworld",
                context(20.0, 5.0, 5.0),
            )
            .await,
        Ok(ZoneObservationOutcome::Changed {
            entered: 0,
            exited: 2,
        })
    );
    assert_eq!(
        adapter
            .observe_player(
                ScriptPlayerId::new(1),
                13,
                "minecraft:overworld",
                context(0.0, 0.0, 0.0),
            )
            .await,
        Ok(ZoneObservationOutcome::Changed {
            entered: 2,
            exited: 0,
        })
    );
    for expected in ["alpha", "zulu"] {
        assert!(matches!(
            events.recv_event().await.unwrap().kind(),
            ScriptEventKind::PlayerZoneEntered { zone_id, .. } if zone_id == expected
        ));
    }
}

#[tokio::test]
async fn membership_and_player_capacity_failures_leave_revision_retryable() {
    let mut commands = admitted_zone_commands(
        "owner",
        r#"
    solaris.upsert_zone("left", "minecraft:overworld", 0, 0, 0, 10, 10, 10)
    solaris.upsert_zone("right", "minecraft:overworld", 5, 0, 0, 15, 10, 10)
"#,
        2,
    )
    .await;
    let limits = ZoneLimits {
        total_zones: 2,
        zones_per_plugin: 2,
        tracked_players: 1,
        memberships: 1,
    };
    let (adapter, mut events) = adapter_with_limits(limits);
    for command in commands.drain(..) {
        adapter.route_admitted(command).unwrap();
    }

    assert_eq!(
        adapter
            .observe_player(
                ScriptPlayerId::new(1),
                1,
                "minecraft:overworld",
                context(7.0, 5.0, 5.0),
            )
            .await,
        Err(ZoneAdapterError::Full(ZoneCapacity::Memberships))
    );
    assert_eq!(
        adapter
            .observe_player(
                ScriptPlayerId::new(1),
                1,
                "minecraft:overworld",
                context(2.0, 5.0, 5.0),
            )
            .await,
        Ok(ZoneObservationOutcome::Changed {
            entered: 1,
            exited: 0,
        })
    );
    assert!(matches!(
        events.recv_event().await.unwrap().kind(),
        ScriptEventKind::PlayerZoneEntered { zone_id, .. } if zone_id == "left"
    ));
    assert_eq!(
        adapter
            .observe_player(
                ScriptPlayerId::new(2),
                1,
                "minecraft:overworld",
                context(20.0, 5.0, 5.0),
            )
            .await,
        Err(ZoneAdapterError::Full(ZoneCapacity::TrackedPlayers))
    );
}

#[tokio::test]
async fn dimension_boundaries_and_player_cleanup_drive_fresh_entries() {
    let mut commands = admitted_zone_commands(
        "owner",
        r#"
    solaris.upsert_zone("overworld", "minecraft:overworld", 0, 0, 0, 10, 10, 10)
    solaris.upsert_zone("nether", "minecraft:the_nether", 0, 0, 0, 10, 10, 10)
"#,
        2,
    )
    .await;
    let (adapter, mut events) = adapter_with_limits(ZoneLimits::production());
    for command in commands.drain(..) {
        adapter.route_admitted(command).unwrap();
    }

    assert_eq!(
        adapter
            .observe_player(
                ScriptPlayerId::new(1),
                1,
                "minecraft:overworld",
                context(10.0, 10.0, 10.0),
            )
            .await,
        Ok(ZoneObservationOutcome::Changed {
            entered: 1,
            exited: 0,
        })
    );
    assert!(matches!(
        events.recv_event().await.unwrap().kind(),
        ScriptEventKind::PlayerZoneEntered { zone_id, .. } if zone_id == "overworld"
    ));
    assert_eq!(
        adapter.forget_player(ScriptPlayerId::new(1)),
        Ok(ZoneCommandOutcome::Applied)
    );
    assert_eq!(
        adapter.forget_player(ScriptPlayerId::new(1)),
        Ok(ZoneCommandOutcome::NoOp)
    );
    assert_eq!(
        adapter
            .observe_player(
                ScriptPlayerId::new(1),
                1,
                "minecraft:the_nether",
                context(0.0, 0.0, 0.0),
            )
            .await,
        Ok(ZoneObservationOutcome::Changed {
            entered: 1,
            exited: 0,
        })
    );
    assert!(matches!(
        events.recv_event().await.unwrap().kind(),
        ScriptEventKind::PlayerZoneEntered { zone_id, .. } if zone_id == "nether"
    ));
}

#[tokio::test]
async fn admitted_non_zone_command_is_rejected_without_mutation() {
    let mut commands =
        admitted_zone_commands("owner", r#"    solaris.broadcast("not a zone")"#, 1).await;
    let (adapter, _events) = adapter_with_limits(ZoneLimits::production());

    assert_eq!(
        adapter.route_admitted(commands.remove(0)),
        Err(ZoneAdapterError::WrongCommand)
    );
}

#[tokio::test]
async fn closed_registry_and_closed_event_queue_are_explicit() {
    let mut commands = admitted_zone_commands(
        "owner",
        r#"
    solaris.upsert_zone("zone", "minecraft:overworld", 0, 0, 0, 10, 10, 10)
    solaris.upsert_zone("after-close", "minecraft:overworld", 0, 0, 0, 10, 10, 10)
"#,
        2,
    )
    .await;
    let (adapter, events) = adapter_with_limits(ZoneLimits::production());
    adapter.route_admitted(commands.remove(0)).unwrap();
    drop(events);

    assert_eq!(
        adapter
            .observe_player(
                ScriptPlayerId::new(1),
                1,
                "minecraft:overworld",
                context(5.0, 5.0, 5.0),
            )
            .await,
        Err(ZoneAdapterError::PublicationClosed)
    );

    adapter.close().unwrap();
    assert_eq!(
        adapter.route_admitted(commands.remove(0)),
        Err(ZoneAdapterError::Closed)
    );
    assert_eq!(
        adapter
            .observe_player(
                ScriptPlayerId::new(1),
                2,
                "minecraft:overworld",
                context(5.0, 5.0, 5.0),
            )
            .await,
        Err(ZoneAdapterError::Closed)
    );
}
