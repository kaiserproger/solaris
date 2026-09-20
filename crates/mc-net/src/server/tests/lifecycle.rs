use super::super::*;
use super::support::*;
use mc_script::{
    ScriptEventKind, ScriptGameMode, ScriptPlayerContext, ScriptPlayerId, script_boundary_pair,
};
use std::future::Future;
use std::num::NonZeroUsize;
use std::task::Poll;

#[test]
fn connection_task_limit_scales_with_players_and_stays_bounded() {
    assert_eq!(connection_task_limit(0), MIN_CONNECTION_TASKS);
    assert_eq!(connection_task_limit(8), MIN_CONNECTION_TASKS);
    assert_eq!(connection_task_limit(20), 56);
    assert_eq!(connection_task_limit(128), 272);
    assert_eq!(connection_task_limit(512), MAX_CONNECTION_TASKS);
    assert_eq!(connection_task_limit(u32::MAX), MAX_CONNECTION_TASKS);
}

#[test]
fn pre_auth_connection_limit_is_smaller_and_bounded() {
    assert_eq!(pre_auth_connection_limit(0), MIN_PRE_AUTH_CONNECTIONS);
    assert_eq!(pre_auth_connection_limit(8), MIN_PRE_AUTH_CONNECTIONS);
    assert_eq!(pre_auth_connection_limit(20), 28);
    assert_eq!(pre_auth_connection_limit(128), MAX_PRE_AUTH_CONNECTIONS);
    assert_eq!(
        pre_auth_connection_limit(u32::MAX),
        MAX_PRE_AUTH_CONNECTIONS
    );
    for max_players in [0, 1, 8, 20, 128, 512, u32::MAX] {
        assert!(pre_auth_connection_limit(max_players) <= connection_task_limit(max_players));
    }
}

#[test]
fn script_sink_routes_known_player_commands_with_exact_payload() {
    let (boundary, mut endpoint) =
        script_boundary_pair(NonZeroUsize::new(2).unwrap(), NonZeroUsize::new(1).unwrap());
    let manifest = mc_script::ScriptPluginManifest::new(
        "greetings",
        "Greetings",
        "0.1.0",
        mc_script::COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_player_command_root("hello")
    .validate()
    .unwrap();
    endpoint.register_plugin_routes(&manifest).unwrap();
    let sink = ScriptEventSink::new(boundary);

    assert_eq!(
        sink.enqueue_player_command_with_operator(7, "Alex", "missing arg", false),
        mc_script::PlayerCommandAdmission::NotOwned
    );
    assert_eq!(
        sink.enqueue_player_command_with_operator(7, "Alex", "hello one  two ", false),
        mc_script::PlayerCommandAdmission::Enqueued
    );
    let event = endpoint.recv_event_blocking().unwrap();
    assert_eq!(event.target_plugin_id(), Some("greetings"));
    assert!(matches!(
        event.kind(),
        ScriptEventKind::PlayerCommand {
            player_id,
            username,
            root,
            arguments,
            ..
        } if *player_id == ScriptPlayerId::new(7)
            && username == "Alex"
            && root == "hello"
            && arguments == "one  two "
    ));
}

#[test]
fn script_sink_reports_queue_full_as_dropped_and_closed_as_unavailable() {
    let (boundary, endpoint) =
        script_boundary_pair(NonZeroUsize::new(1).unwrap(), NonZeroUsize::new(1).unwrap());
    let manifest = mc_script::ScriptPluginManifest::new(
        "greetings",
        "Greetings",
        "0.1.0",
        mc_script::COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_player_command_root("hello")
    .validate()
    .unwrap();
    endpoint.register_plugin_routes(&manifest).unwrap();
    let sink = ScriptEventSink::new(boundary);
    sink.enqueue_event(ScriptEvent::server_started());

    assert_eq!(
        sink.enqueue_player_command_with_operator(7, "Alex", "hello full", false),
        mc_script::PlayerCommandAdmission::Dropped
    );

    drop(endpoint);
    assert_eq!(
        sink.enqueue_player_command_with_operator(7, "Alex", "hello closed", false),
        mc_script::PlayerCommandAdmission::NotOwned
    );
    assert!(sink.player_command_roots().is_empty());
}

#[tokio::test]
async fn committed_script_event_worker_waits_for_exact_queue_capacity_notification() {
    let one = NonZeroUsize::new(1).unwrap();
    let (boundary, mut endpoint) = script_boundary_pair(one, one);
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    let sink = ScriptEventSink::new(boundary);
    let sessions = play::SessionRegistry::new();
    let receiver = sessions.install_script_commit_event_outbox();
    sessions
        .try_enqueue_script_commit_event_for_test(
            ScriptEvent::try_player_died_with_context(
                ScriptPlayerId::new(7),
                ScriptPlayerContext::new(
                    "123e4567-e89b-12d3-a456-426614174000",
                    "Alex",
                    false,
                    1.5,
                    64.0,
                    -2.5,
                ),
                "minecraft:overworld",
                ScriptGameMode::Survival,
            )
            .unwrap(),
        )
        .unwrap();
    sessions.close_script_commit_event_outbox();
    let worker = tokio::spawn(forward_committed_script_events(receiver, sink));

    assert!(matches!(
        endpoint.recv_event().await.unwrap().kind(),
        ScriptEventKind::ServerStarted
    ));
    assert!(matches!(
        endpoint.recv_event().await.unwrap().kind(),
        ScriptEventKind::PlayerDied { player_id, .. }
            if *player_id == ScriptPlayerId::new(7)
    ));
    worker.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn stalled_required_script_sink_times_out_and_fails_remaining_backlog() {
    let one = NonZeroUsize::new(1).unwrap();
    let (boundary, _endpoint) = script_boundary_pair(one, one);
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    let sink = ScriptEventSink::new(boundary);
    let sessions = play::SessionRegistry::new();
    let receiver = sessions.install_script_commit_event_outbox();
    for tick in 1..=2 {
        sessions
            .try_enqueue_script_commit_event_for_test(ScriptEvent::server_tick(tick))
            .unwrap();
    }
    sessions.close_script_commit_event_outbox();
    let failure = sessions.script_commit_event_monitor();
    let worker = forward_committed_script_events(receiver, sink);
    tokio::pin!(worker);
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);
    assert!(matches!(
        std::future::Future::poll(worker.as_mut(), &mut context),
        std::task::Poll::Pending
    ));

    tokio::time::advance(SCRIPT_COMMIT_FORWARD_TIMEOUT + Duration::from_millis(1)).await;

    assert!(matches!(
        worker.await,
        Err(ScriptCommitForwardError::RequiredTimeout { timeout })
            if timeout == SCRIPT_COMMIT_FORWARD_TIMEOUT
    ));
    assert!(failure.failed());
    let snapshot = sessions.script_commit_event_outbox_snapshot();
    assert_eq!(snapshot.depth, 0);
    assert_eq!(snapshot.dequeued, 1);
    assert_eq!(snapshot.required_abandoned_on_receiver_drop, 1);
}

#[tokio::test]
async fn required_committed_script_event_overflow_requests_shutdown() {
    let sessions = play::SessionRegistry::new();
    let _receiver = sessions.install_script_commit_event_outbox();
    let capacity = sessions.script_commit_event_outbox_snapshot().capacity;
    let shutdown = ShutdownHandle::default();
    let watcher = tokio::spawn(watch_script_commit_event_failure(
        sessions.script_commit_event_monitor(),
        shutdown.clone(),
    ));

    for tick in 0..capacity {
        sessions
            .try_enqueue_script_commit_event_for_test(ScriptEvent::server_tick(tick as u64))
            .unwrap();
    }
    assert!(
        sessions
            .try_enqueue_script_commit_event_for_test(ScriptEvent::server_stopping(
                "required overflow"
            ),)
            .is_err()
    );

    tokio::time::timeout(Duration::from_secs(1), shutdown.wait_requested())
        .await
        .expect("required outbox failure did not request shutdown");
    let snapshot = sessions.script_commit_event_outbox_snapshot();
    assert_eq!(snapshot.depth, capacity);
    assert_eq!(snapshot.max_depth, capacity);
    assert_eq!(snapshot.required_overflow, 1);
    watcher.await.unwrap();
}

#[tokio::test]
async fn shutdown_wait_wakes_when_shutdown_is_requested() {
    let shutdown = ShutdownHandle::default();
    let mut waiter = Box::pin(shutdown.wait_requested());

    std::future::poll_fn(|context| match waiter.as_mut().poll(context) {
        Poll::Pending => Poll::Ready(()),
        Poll::Ready(()) => panic!("shutdown wait completed before the request"),
    })
    .await;
    shutdown.request();

    tokio::time::timeout(Duration::from_secs(1), waiter.as_mut())
        .await
        .expect("shutdown waiter did not wake");
}

#[tokio::test]
async fn shutdown_wait_observes_request_made_before_registration() {
    let shutdown = ShutdownHandle::default();
    shutdown.request();

    tokio::time::timeout(Duration::from_secs(1), shutdown.wait_requested())
        .await
        .expect("pre-requested shutdown wait did not complete");
}

#[tokio::test]
async fn script_spawn_rejects_unknown_registry_identifier_without_queueing() {
    let config = ServerConfig {
        tab_list: crate::server::TabListConfig::default(),
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "test".to_owned(),
        max_players: 1,
        view_distance: 2,
        data: Arc::new(mc_data::testing::stub()),
        blocks: Arc::new(BlockRegistry::from_report(&[]).unwrap()),
        world: None,
        tags: Arc::new(TagsData::default()),
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(LootTables::default()),
        block_light: None,
        items: Arc::new(ItemRegistry::default()),
        item_facts: Arc::new(ItemFactsTable::default()),
        block_facts: Arc::new(BlockFactsTable::default()),
        entity_types: canonical_entity_types(),
        biome_spawns: Arc::new(BiomeSpawnRules::default()),
        chunk_pipeline: ChunkPipelinePolicy::default(),
        random_tick: play::RandomTickPolicy::default(),
        command_permissions: CommandPermissionConfig::new(Vec::<String>::new(), false),
        loader_manifest: None,
        shutdown: ShutdownHandle::default(),
    };
    let (simulation, _owner) = play::simulation_channel();
    assert_eq!(
        resolve_script_entity_type(&config, "minecraft:missing"),
        None
    );
    assert_eq!(simulation.snapshot().depth, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn console_time_set_waits_for_server_owned_simulation_turn() {
    let config = ServerConfig {
        tab_list: crate::server::TabListConfig::default(),
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "test".to_owned(),
        max_players: 1,
        view_distance: 2,
        data: Arc::new(mc_data::testing::stub()),
        blocks: Arc::new(BlockRegistry::from_report(&[]).unwrap()),
        world: None,
        tags: Arc::new(TagsData::default()),
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(LootTables::default()),
        block_light: None,
        items: Arc::new(ItemRegistry::default()),
        item_facts: Arc::new(ItemFactsTable::default()),
        block_facts: Arc::new(BlockFactsTable::default()),
        entity_types: canonical_entity_types(),
        biome_spawns: Arc::new(BiomeSpawnRules::default()),
        chunk_pipeline: ChunkPipelinePolicy::default(),
        random_tick: play::RandomTickPolicy::default(),
        command_permissions: CommandPermissionConfig::new(Vec::<String>::new(), false),
        loader_manifest: None,
        shutdown: ShutdownHandle::default(),
    };
    let sessions = Arc::new(play::SessionRegistry::new());
    let chunk_pipeline_resources = ChunkPipelineResources::with_limits(1, 1);
    let (simulation, mut owner) = play::simulation_channel();
    let control = OperatorControlHandle {
        sessions: Arc::clone(&sessions),
        simulation: simulation.clone(),
        shutdown: config.shutdown.clone(),
        runtime_control: None,
        resources: chunk_pipeline_resources,
        operators: config.command_permissions.operator_identities(),
        whitelist: config.command_permissions.whitelist_identities(),
    };
    let mut command = Box::pin(control.set_world_time(13_000));

    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(command.as_mut(), cx).is_pending(),
            "console time set must wait for its owner response"
        );
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(sessions.world_time(), 0);
    assert_eq!(simulation.snapshot().depth, 1);

    assert_eq!(owner.process_tick(&sessions, 1).processed, 1);
    command.await.expect("owner accepted time change");
    assert_eq!(sessions.world_time(), 13_000);
}

#[test]
fn stopped_entity_ticker_requests_server_shutdown() {
    let shutdown = ShutdownHandle::default();

    let error = handle_entity_ticker_exit(&shutdown, Ok(())).expect_err("unexpected stop fails");

    assert!(shutdown.is_requested());
    assert_eq!(error.kind(), ErrorKind::BrokenPipe);
}

#[test]
fn runtime_metrics_policy_normalizes_log_interval() {
    let policy = RuntimeMetricsPolicy {
        log_interval_ticks: 0,
        slow_tick_ms: 0,
    }
    .normalized();

    assert_eq!(policy.log_interval_ticks, 1);
    assert_eq!(policy.slow_tick_ms, 0);
}

#[test]
fn simulation_command_window_marks_off_tick_scope() {
    let mut window = SimulationCommandTelemetryWindow::default();

    window.record_off_tick(41, 2);
    let telemetry = window.finish_tick(1, 0);

    assert_eq!(telemetry.elapsed_us, 42);
    assert_eq!(telemetry.processed, 2);
    assert_eq!(
        telemetry.scope,
        SimulationCommandTelemetryScope::SincePreviousTickBoundary
    );
    assert_eq!(telemetry.scope.as_str(), "since_previous_tick_boundary");
}

#[test]
fn simulation_command_gate_bounds_off_tick_work_between_ticks() {
    let mut gate = SimulationCommandGate::default();

    assert!(gate.accepts_off_tick_batch());
    gate.record_off_tick_batch();
    assert!(!gate.accepts_off_tick_batch());
    gate.record_tick_boundary();
    assert!(gate.accepts_off_tick_batch());
}

#[tokio::test]
async fn script_command_task_drains_buffered_command_before_shutdown() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let config = Arc::new(save_all_test_config(
        tmp.path(),
        Arc::new(BlockRegistry::from_report(&[]).unwrap()),
        Arc::new(ItemRegistry::default()),
        canonical_entity_types(),
    ));
    let (boundary, endpoint) =
        script_boundary_pair(NonZeroUsize::new(4).unwrap(), NonZeroUsize::new(4).unwrap());
    let scripts = ScriptEventSink::new(boundary);
    endpoint
        .try_submit_command(ScriptCommand::BroadcastChatMessage {
            message: "accepted before shutdown".to_owned(),
        })
        .unwrap();
    drop(endpoint);
    let shutdown = config.shutdown.clone();
    shutdown.request();
    let (simulation, _owner) = play::simulation_channel();

    run_script_commands(ScriptCommandTask {
        scripts: scripts.clone(),
        zones: PluginZoneAdapter::new(scripts.clone()),
        storage: None,
        config,
        sessions: Arc::new(play::SessionRegistry::new()),
        simulation,
        shutdown,
    })
    .await;

    assert!(
        scripts.recv_command().await.is_none(),
        "buffered script command must be consumed before the shutdown fence"
    );
}

#[test]
fn loaded_block_tick_hint_only_wakes_for_due_loaded_chunk() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).unwrap());
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let scheduled_ticks = world.scheduled_tick_view();
    let cpos = mc_world::ChunkPos { x: 2, z: 3 };
    let pos = mc_world::BlockPos {
        x: 2 * 16 + 1,
        y: 2,
        z: 3 * 16 + 1,
    };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(
            cpos,
            mc_world::Chunk::empty(cpos, mc_world::BlockStateId(0), biome),
        )
        .unwrap();
    world
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            pos,
            mc_data::Identifier::parse("minecraft:wheat").unwrap(),
            10,
            0,
        ))
        .unwrap();

    assert!(!loaded_block_tick_due(&scheduled_ticks, &[(2, 3)], 9));
    assert!(!loaded_block_tick_due(&scheduled_ticks, &[(0, 0)], 10));
    assert!(loaded_block_tick_due(&scheduled_ticks, &[(2, 3)], 10));
}

#[test]
fn loaded_fluid_tick_hint_only_wakes_for_due_loaded_chunk() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).unwrap());
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let scheduled_ticks = world.scheduled_tick_view();
    let cpos = mc_world::ChunkPos { x: 2, z: 3 };
    let pos = mc_world::BlockPos {
        x: 2 * 16 + 1,
        y: 2,
        z: 3 * 16 + 1,
    };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(
            cpos,
            mc_world::Chunk::empty(cpos, mc_world::BlockStateId(0), biome),
        )
        .unwrap();
    world
        .schedule_fluid_tick(mc_world::ScheduledFluidTick::new(
            pos,
            mc_data::Identifier::parse("minecraft:water").unwrap(),
            10,
            0,
        ))
        .unwrap();

    assert!(!loaded_fluid_tick_due(&scheduled_ticks, &[(2, 3)], 9));
    assert!(!loaded_fluid_tick_due(&scheduled_ticks, &[(0, 0)], 10));
    assert!(loaded_fluid_tick_due(&scheduled_ticks, &[(2, 3)], 10));
}

#[test]
fn runtime_metrics_logging_respects_interval_and_slow_budget() {
    let policy = RuntimeMetricsPolicy {
        log_interval_ticks: 5,
        slow_tick_ms: 50,
    };

    let mut gate = RuntimeMetricsLogGate::default();
    assert!(gate.should_log(10, 1, policy));
    assert!(gate.should_log(11, 50_000, policy));
    assert!(!gate.should_log(12, 50_001, policy));
    assert!(gate.should_log(15, 50_001, policy));
    assert!(!gate.should_log(16, 49_999, policy));
    assert!(gate.should_log(17, 50_000, policy));
}
