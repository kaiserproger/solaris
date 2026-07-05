//! Load-oriented scenarios.
//!
//! Sidecar-dependent load tests are ignored so missing local vanilla sidecars
//! are reported as degraded coverage instead of a silent green pass. If run
//! without sidecars, they fail with an explicit degraded message. Run them with:
//!
//! ```text
//! cargo test -p mc-test-harness --test load_scenarios -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, BlockChangedAck, BlockUpdate, ClientboundContainerSetSlot, ClientboundKeepAlive,
    ConfirmTeleportation, Direction, GameEvent, InteractionHand, LevelChunkWithLight,
    MovePlayerFlags, SectionBlocksUpdate, ServerboundChatCommand, ServerboundKeepAlive,
    ServerboundMovePlayerPos, ServerboundUseItemOn, SetCenterChunk, SynchronizePlayerPosition,
    pack_block_pos, pack_section_pos, pack_section_relative_pos, unpack_block_pos,
};
use mc_test_harness::client::Client;

const VIEW_DISTANCE: i32 = 1;
const LOAD_CHUNK_IO_THREADS: usize = 1;
const LOAD_CHUNK_WORKER_THREADS: usize = 2;
const M52_BASELINE_CLIENTS: usize = 4;
const M52_BASELINE_SUMMONS: usize = 8;
const M52_SLOW_READER_SUMMONS: usize = 256;
const M52_HEALTHY_OBSERVER_SUMMONS: usize = 128;
const M52_BASELINE_ELAPSED_BUDGET: Duration = Duration::from_secs(30);
const M52_LOCK_MAX_HOLD_BUDGET_US: u64 = 250_000;
const M96_REPLAY_CLIENTS: usize = 4;
const M96_PRESSURE_SUMMONS: usize = 128;
const M96_REPLAY_ELAPSED_BUDGET: Duration = Duration::from_secs(45);
const M96_LOCK_MAX_HOLD_BUDGET_US: u64 = 250_000;
const O2_STOP_FLUSH_CLIENTS: usize = 4;
const O2_STOP_VIEW_DISTANCE: i32 = 8;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires local data/vanilla sidecars; degraded when absent"]
async fn bounded_multiplayer_survival_replay_covers_sequential_contention_and_slow_reader() {
    let server = start_load_server().await;
    let addr = server.addr;
    let started = Instant::now();

    let paused_reader = PausedReaderClient::connect(addr, "M96PausedReader").await;
    let pressure_before = server.outbound_pressure_snapshot();

    let mut client_tasks = Vec::new();
    for idx in 0..M96_REPLAY_CLIENTS {
        client_tasks.push(tokio::spawn(async move {
            let (mut client, sync) = connect_to_play(addr, &format!("M96Soak{idx}")).await;
            drain_until_chunk(&mut client, (0, 0)).await;
            (client, sync)
        }));
    }

    let mut clients = Vec::new();
    for task in client_tasks {
        clients.push(task.await.expect("M96 replay client task joins"));
    }

    let (mut editor_a, sync_a) = clients.remove(0);
    let (mut editor_b, sync_b) = clients.remove(0);
    let (mut observer, _) = clients.remove(0);
    let (mut reconnecting, reconnect_sync) = clients.remove(0);

    editor_a
        .write_packet(&ServerboundChatCommand {
            command: "gamemode creative".to_string(),
        })
        .await
        .expect("set creative for editor A");
    editor_b
        .write_packet(&ServerboundChatCommand {
            command: "gamemode creative".to_string(),
        })
        .await
        .expect("set creative for editor B");
    editor_a
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:dirt 64 0".to_string(),
        })
        .await
        .expect("give dirt to editor A");
    editor_b
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:stone 64 0".to_string(),
        })
        .await
        .expect("give stone to editor B");
    wait_for_inventory_stack(&mut editor_a).await;
    wait_for_inventory_stack(&mut editor_b).await;

    let target = find_placeable_target(&server, 4, 0, sync_a.y.floor() as i32).await;
    let support = pack_block_pos(target.0, target.1 - 1, target.2);
    let target_pos = pack_block_pos(target.0, target.1, target.2);
    editor_a
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: support,
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 1,
        })
        .await
        .expect("editor A places same-block contender");
    editor_b
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: support,
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 2,
        })
        .await
        .expect("editor B places same-block contender");

    let ack_a = wait_for_ack(&mut editor_a, 1).await;
    let ack_b = wait_for_ack(&mut editor_b, 2).await;
    let observer_updates =
        drain_target_block_updates(&mut observer, target_pos, Duration::from_secs(5)).await;
    assert!(
        ack_a && ack_b,
        "both sequential same-block edit attempts should receive BlockChangedAck"
    );
    assert!(
        (1..=2).contains(&observer_updates),
        "observer should see one bounded final/resync update for sequential same-block contention surrogate, got {observer_updates}"
    );

    let chunk_before_disconnect = server.chunk_pipeline_metrics.snapshot();
    reconnecting
        .write_packet(&ServerboundMovePlayerPos {
            x: reconnect_sync.x + 48.0,
            y: reconnect_sync.y,
            z: reconnect_sync.z,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("start chunk work before reconnect");
    drop(reconnecting);
    let (mut rejoined, _) = connect_to_play(addr, "M96SoakReconnect").await;
    drain_until_chunk(&mut rejoined, (0, 0)).await;
    let chunk_after_rejoin = server.chunk_pipeline_metrics.snapshot();
    assert!(
        chunk_after_rejoin.max_cpu_active >= chunk_before_disconnect.max_cpu_active,
        "disconnect-after-move must not poison later chunk streaming: before={chunk_before_disconnect:?} after={chunk_after_rejoin:?}"
    );

    let spawn_y = sync_b.y.floor() as i32;
    for idx in 0..M52_BASELINE_SUMMONS {
        editor_b
            .write_packet(&ServerboundChatCommand {
                command: format!(
                    "summon minecraft:zombie {} {} {}",
                    6 + idx % 4,
                    spawn_y,
                    2 + idx / 4
                ),
            })
            .await
            .expect("summon replay zombie");
    }
    let spawns = drain_counting(&mut observer, Duration::from_secs(5), AddEntity::ID).await;
    assert!(
        spawns > 0,
        "observer should receive replay entity broadcasts"
    );

    for idx in 0..M96_PRESSURE_SUMMONS {
        editor_b
            .write_packet(&ServerboundChatCommand {
                command: format!(
                    "summon minecraft:zombie {} {} {}",
                    10 + idx % 16,
                    spawn_y,
                    6 + idx / 16
                ),
            })
            .await
            .expect("summon replay pressure zombie");
    }

    let pressure_after = wait_for_outbound_pressure_increase(&server, pressure_before).await;
    assert_slow_reader_retry_bounded(pressure_before, pressure_after);
    assert_ne!(
        pressure_after, pressure_before,
        "paused reader should produce a measured outbound pressure delta"
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed <= M96_REPLAY_ELAPSED_BUDGET,
        "M96 bounded replay exceeded debug elapsed budget: elapsed={elapsed:?} budget={M96_REPLAY_ELAPSED_BUDGET:?}"
    );

    let chunk_snapshot = server.chunk_pipeline_metrics.snapshot();
    assert_eq!(
        chunk_snapshot.active_cpu, 0,
        "chunk CPU work should not remain active after M96 replay sample: {:?}",
        chunk_snapshot
    );
    assert_eq!(
        chunk_snapshot.active_io, 0,
        "chunk IO work should not remain active after M96 replay sample: {:?}",
        chunk_snapshot
    );
    assert!(
        chunk_snapshot.max_cpu_active <= server.chunk_worker_threads,
        "chunk CPU permits exceeded during M96 replay: {:?}",
        chunk_snapshot
    );
    assert!(
        chunk_snapshot.max_io_active <= server.chunk_io_threads,
        "chunk IO permits exceeded during M96 replay: {:?}",
        chunk_snapshot
    );

    let pressure = mc_net::lock_pressure_snapshot();
    assert!(
        pressure.session_registry.max_hold_us <= M96_LOCK_MAX_HOLD_BUDGET_US,
        "session registry lock hold exceeded M96 replay budget: {:?}",
        pressure.session_registry
    );
    assert!(
        pressure.world_storage.max_hold_us <= M96_LOCK_MAX_HOLD_BUDGET_US,
        "world storage lock hold exceeded M96 replay budget: {:?}",
        pressure.world_storage
    );
    eprintln!(
        "M96 bounded_replay clients={} sequential_same_block_updates={} spawns={} elapsed_ms={} outbound_before={:?} outbound_after={:?} chunk_before_disconnect={:?} chunk_pipeline={:?} session_lock={:?} world_lock={:?}",
        M96_REPLAY_CLIENTS + 1,
        observer_updates,
        spawns,
        elapsed.as_millis(),
        pressure_before,
        pressure_after,
        chunk_before_disconnect,
        chunk_snapshot,
        pressure.session_registry,
        pressure.world_storage,
    );

    paused_reader.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires local data/vanilla sidecars; degraded when absent"]
async fn vd8_multi_client_stop_drains_and_flushes_disk_world_under_stream_load() {
    let server = start_load_server_with_options(LoadServerOptions {
        view_distance: O2_STOP_VIEW_DISTANCE,
        disk_backed: true,
        runtime_control: true,
    })
    .await;
    let addr = server.addr;
    let save_pressure_before = mc_net::lock_pressure_snapshot().save_all_flush;

    let mut client_tasks = Vec::new();
    for idx in 0..O2_STOP_FLUSH_CLIENTS {
        client_tasks.push(tokio::spawn(async move {
            let (mut client, _) = connect_to_play(addr, &format!("O2Stop{idx}")).await;
            let chunks = drain_unique_chunks(&mut client, 9).await;
            (client, chunks)
        }));
    }

    let mut clients = Vec::new();
    let mut streamed_chunks = std::collections::BTreeSet::new();
    for task in client_tasks {
        let (client, chunks) = task.await.expect("O2 stop client task joins");
        streamed_chunks.extend(chunks);
        clients.push(client);
    }
    let mut stopper = clients.remove(0);
    let dirty_before = {
        let world = server.world.lock().await;
        world.stats().dirty_chunks
    };
    assert!(
        dirty_before > 0,
        "disk-backed VD8 stream should dirty generated chunks before stop"
    );

    stopper
        .write_packet(&ServerboundChatCommand {
            command: "stop".to_string(),
        })
        .await
        .expect("send stop command");
    wait_for_shutdown_requested(&server.shutdown).await;
    for client in clients {
        drop(client);
    }
    drop(stopper);

    assert!(
        server.shutdown.is_requested(),
        "player /stop should signal shutdown"
    );
    let runtime_control = server
        .runtime_control
        .as_ref()
        .expect("O2 stop gate enables runtime control");
    assert!(
        runtime_control.snapshot().draining,
        "player /stop should request runtime-control drain"
    );

    let serve_result = tokio::time::timeout(Duration::from_secs(10), server.serve_task)
        .await
        .expect("server should exit after player /stop")
        .expect("server task should join");
    serve_result.expect("server serve should exit cleanly");

    let chunk_snapshot = server.chunk_pipeline_metrics.snapshot();
    assert_eq!(
        chunk_snapshot.active_cpu, 0,
        "chunk CPU work should be drained after /stop: {:?}",
        chunk_snapshot
    );
    assert_eq!(
        chunk_snapshot.active_io, 0,
        "chunk IO work should be drained after /stop: {:?}",
        chunk_snapshot
    );

    let dirty_after = {
        let world = server.world.lock().await;
        world.stats().dirty_chunks
    };
    assert_eq!(
        dirty_after, 0,
        "player /stop should flush all disk-backed generated chunks after draining; dirty_before={dirty_before}"
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    let quiet_dirty_after = {
        let world = server.world.lock().await;
        world.stats().dirty_chunks
    };
    assert_eq!(
        quiet_dirty_after, 0,
        "detached chunk work should not dirty chunks after the final shutdown save"
    );

    let save_pressure_after = mc_net::lock_pressure_snapshot().save_all_flush;
    assert!(
        save_pressure_after.hold_count > save_pressure_before.hold_count,
        "player /stop should exercise save_all_flush lock path: before={save_pressure_before:?} after={save_pressure_after:?}"
    );
    let world_dir = server
        .world_dir
        .as_ref()
        .expect("O2 stop gate uses a disk-backed temp world");
    let mut reopened = mc_world::WorldStorage::open_with_capacity(
        world_dir.path(),
        Arc::clone(&server.blocks),
        streamed_chunks.len().max(4),
    )
    .expect("reopen disk-backed stop world");
    for (cx, cz) in &streamed_chunks {
        let chunk = reopened
            .get_chunk(mc_world::ChunkPos { x: *cx, z: *cz })
            .expect("read streamed chunk after stop flush");
        assert!(
            chunk.is_some(),
            "streamed chunk ({cx},{cz}) should exist on disk after stop flush"
        );
    }

    eprintln!(
        "O2 VD8 stop flush clients={} streamed_chunks={} dirty_before={} dirty_after={} chunk_pipeline={:?} save_all_before={:?} save_all_after={:?}",
        O2_STOP_FLUSH_CLIENTS,
        streamed_chunks.len(),
        dirty_before,
        dirty_after,
        chunk_snapshot,
        save_pressure_before,
        save_pressure_after,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires local data/vanilla sidecars; degraded when absent"]
async fn multicore_login_chunk_stream_and_broadcast_stays_within_budgets() {
    let server = start_load_server().await;
    let addr = server.addr;
    let started = Instant::now();

    let mut client_tasks = Vec::new();
    for idx in 0..M52_BASELINE_CLIENTS {
        client_tasks.push(tokio::spawn(async move {
            let (mut client, sync) = connect_to_play(addr, &format!("M52Load{idx}")).await;
            drain_until_chunk(&mut client, (0, 0)).await;
            (client, sync)
        }));
    }

    let mut clients = Vec::new();
    for task in client_tasks {
        clients.push(task.await.expect("client task joins"));
    }

    let (mut actor, actor_sync) = clients.remove(0);
    let (mut observer, _) = clients.remove(0);
    let spawn_y = actor_sync.y.floor() as i32;
    for idx in 0..M52_BASELINE_SUMMONS {
        actor
            .write_packet(&ServerboundChatCommand {
                command: format!(
                    "summon minecraft:zombie {} {} {}",
                    idx % 4,
                    spawn_y,
                    2 + idx / 4
                ),
            })
            .await
            .expect("summon zombie for broadcast baseline");
    }

    let spawns = drain_counting(&mut observer, Duration::from_secs(5), AddEntity::ID).await;
    assert!(
        spawns > 0,
        "observer should receive gameplay entity broadcasts from command path"
    );

    let elapsed = started.elapsed();
    assert!(
        elapsed <= M52_BASELINE_ELAPSED_BUDGET,
        "M52 multicore baseline exceeded debug elapsed budget: elapsed={elapsed:?} budget={M52_BASELINE_ELAPSED_BUDGET:?}"
    );

    let snapshot = server.chunk_pipeline_metrics.snapshot();
    assert!(
        snapshot.max_cpu_active > 0,
        "chunk load test should force CPU chunk preparation"
    );
    assert!(
        snapshot.max_io_active <= server.chunk_io_threads,
        "global IO permits exceeded: {:?}",
        snapshot
    );
    assert!(
        snapshot.max_cpu_active <= server.chunk_worker_threads,
        "global CPU permits exceeded: {:?}",
        snapshot
    );

    let pressure = mc_net::lock_pressure_snapshot();
    assert!(
        pressure.session_registry.max_hold_us <= M52_LOCK_MAX_HOLD_BUDGET_US,
        "session registry lock hold exceeded coarse M52 budget: {:?}",
        pressure.session_registry
    );
    assert!(
        pressure.world_storage.max_hold_us <= M52_LOCK_MAX_HOLD_BUDGET_US,
        "world storage lock hold exceeded coarse M52 budget: {:?}",
        pressure.world_storage
    );
    eprintln!(
        "M52 multicore clients={} summons={} observer_spawns={} elapsed_ms={} chunk_pipeline={:?} session_lock={:?} world_lock={:?}",
        M52_BASELINE_CLIENTS,
        M52_BASELINE_SUMMONS,
        spawns,
        elapsed.as_millis(),
        snapshot,
        pressure.session_registry,
        pressure.world_storage,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires local data/vanilla sidecars; degraded when absent"]
async fn paused_reader_does_not_stall_active_entity_broadcasts() {
    let server = start_load_server().await;
    let addr = server.addr;

    let paused_reader = PausedReaderClient::connect(addr, "M52PausedReader").await;

    let (mut active_client, active_sync) = connect_to_play(addr, "M52ActiveReader").await;
    drain_until_chunk(&mut active_client, (0, 0)).await;

    let pressure_before = server.outbound_pressure_snapshot();
    let spawn_y = active_sync.y.floor() as i32;
    for idx in 0..M52_SLOW_READER_SUMMONS {
        active_client
            .write_packet(&ServerboundChatCommand {
                command: format!(
                    "summon minecraft:zombie {} {} {}",
                    idx % 4,
                    spawn_y,
                    2 + idx / 4
                ),
            })
            .await
            .expect("summon zombie while peer reader is paused");
    }

    let spawns = drain_counting(&mut active_client, Duration::from_secs(5), AddEntity::ID).await;
    assert!(
        spawns > 0,
        "active client should keep receiving entity broadcasts while another reader is paused"
    );

    let pressure_after = wait_for_outbound_pressure_increase(&server, pressure_before).await;
    assert_slow_reader_retry_bounded(pressure_before, pressure_after);

    let chunk_snapshot = server.chunk_pipeline_metrics.snapshot();
    let stop_reasons = server.chunk_pipeline_metrics.observed_stop_reasons();
    assert!(
        !stop_reasons.is_empty(),
        "slow-reader gate should feed live chunk-stream stop reasons into autoscale accounting"
    );
    let scenarios = [mc_net::AutoscaleSoakScenario::SlowClient];
    let autoscale_report =
        mc_net::AutoscaleSoakReport::from_snapshot(mc_net::AutoscaleSoakSnapshot {
            profile: mc_net::AutoscaleSoakProfile::Balanced,
            scenarios: &scenarios,
            chunk_policy: server.chunk_policy,
            chunk_resources: chunk_snapshot,
            chunk_stop_reasons: &stop_reasons,
            outbound_pressure: pressure_after,
            save_all: None,
            memory_pressure_shed_chunks: 0,
            runtime_control: None,
        });
    assert!(
        matches!(
            autoscale_report.worker_backpressure,
            mc_net::AutoscalePrimitiveStatus::Present
        ),
        "slow-reader live resource metrics should stay within configured permits: {autoscale_report:?}"
    );
    assert!(
        autoscale_report.slow_client_pressure_observed
            && matches!(
                autoscale_report.slow_client_pressure,
                mc_net::AutoscalePrimitiveStatus::Present
            ),
        "slow-reader pressure should count as explicitly scoped slow-client evidence: {autoscale_report:?}"
    );
    assert!(
        !autoscale_report.queue_saturation_observed
            && autoscale_report
                .gaps
                .contains(&"queue-saturation scenario not run"),
        "slow-reader report must not promote unattempted queue-saturation evidence: {autoscale_report:?}"
    );
    assert!(
        autoscale_report.is_degraded(),
        "slow-reader bounded report must remain degraded without full O3 soak/recovery evidence"
    );

    let pressure = mc_net::lock_pressure_snapshot();
    eprintln!(
        "M52 slow_reader active_spawns={} summons={} paused_start=({:.1},{:.1},{:.1}) outbound_before={:?} outbound_after={:?} chunk_pipeline={:?} stop_reasons={:?} autoscale_report={:?} session_lock_max_hold_us={} session_lock_wait_us={}",
        spawns,
        M52_SLOW_READER_SUMMONS,
        paused_reader.sync.x,
        paused_reader.sync.y,
        paused_reader.sync.z,
        pressure_before,
        pressure_after,
        chunk_snapshot,
        stop_reasons,
        autoscale_report,
        pressure.session_registry.max_hold_us,
        pressure.session_registry.wait_us,
    );

    paused_reader.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires local data/vanilla sidecars; degraded when absent"]
async fn paused_reader_pressure_does_not_delay_healthy_observers() {
    let server = start_load_server().await;
    let addr = server.addr;

    let paused_reader = PausedReaderClient::connect(addr, "M52PauseObs").await;
    let (mut actor, actor_sync) = connect_to_play(addr, "M52PressureActor").await;
    drain_until_chunk(&mut actor, (0, 0)).await;
    let (mut observer_a, _) = connect_to_play(addr, "M52ObserverA").await;
    drain_until_chunk(&mut observer_a, (0, 0)).await;
    let (mut observer_b, _) = connect_to_play(addr, "M52ObserverB").await;
    drain_until_chunk(&mut observer_b, (0, 0)).await;

    let pressure_before = server.outbound_pressure_snapshot();
    let observer_a_task = tokio::spawn(async move {
        drain_counting_until(
            &mut observer_a,
            Duration::from_secs(10),
            AddEntity::ID,
            M52_HEALTHY_OBSERVER_SUMMONS,
        )
        .await
    });
    let observer_b_task = tokio::spawn(async move {
        drain_counting_until(
            &mut observer_b,
            Duration::from_secs(10),
            AddEntity::ID,
            M52_HEALTHY_OBSERVER_SUMMONS,
        )
        .await
    });

    let spawn_y = actor_sync.y.floor() as i32;
    let mut actor_spawns = 0usize;
    for idx in 0..M52_HEALTHY_OBSERVER_SUMMONS {
        actor
            .write_packet(&ServerboundChatCommand {
                command: format!(
                    "summon minecraft:zombie {} {} {}",
                    4 + idx % 4,
                    spawn_y,
                    4 + (idx / 4) % 4
                ),
            })
            .await
            .expect("summon zombie while healthy observers are draining");
        if idx % 8 == 7 {
            actor_spawns +=
                drain_counting(&mut actor, Duration::from_millis(50), AddEntity::ID).await;
        }
    }
    actor_spawns += drain_counting(&mut actor, Duration::from_millis(250), AddEntity::ID).await;

    let observed_a = observer_a_task
        .await
        .expect("healthy observer A drain task joins");
    let observed_b = observer_b_task
        .await
        .expect("healthy observer B drain task joins");
    let pressure_after = wait_for_outbound_pressure_increase(&server, pressure_before).await;
    assert_slow_reader_retry_bounded(pressure_before, pressure_after);

    assert_eq!(
        observed_a, M52_HEALTHY_OBSERVER_SUMMONS,
        "healthy observer A should receive every spawn while a peer reader is paused"
    );
    assert_eq!(
        observed_b, M52_HEALTHY_OBSERVER_SUMMONS,
        "healthy observer B should receive every spawn while a peer reader is paused"
    );
    eprintln!(
        "M52 slow_reader_healthy_observers summons={} observed_a={} observed_b={} actor_spawns={} paused_start=({:.1},{:.1},{:.1}) outbound_before={:?} outbound_after={:?}",
        M52_HEALTHY_OBSERVER_SUMMONS,
        observed_a,
        observed_b,
        actor_spawns,
        paused_reader.sync.x,
        paused_reader.sync.y,
        paused_reader.sync.z,
        pressure_before,
        pressure_after,
    );

    paused_reader.close();
}

#[tokio::test]
#[ignore = "M37 load report; run explicitly with --ignored --nocapture"]
async fn reports_spawn_exploration_block_entity_and_multi_client_load() {
    let server = start_load_server().await;
    let addr = server.addr;

    let started = Instant::now();
    let mut clients = Vec::new();
    for idx in 0..4 {
        let (mut client, sync) = connect_to_play(addr, &format!("M37Load{idx}")).await;
        drain_until_chunk(&mut client, (0, 0)).await;
        clients.push((client, sync));
    }
    eprintln!(
        "M37 load spawn_login_multi_client clients={} elapsed_ms={}",
        clients.len(),
        started.elapsed().as_millis()
    );

    let started = Instant::now();
    let (client, sync) = clients.get_mut(0).expect("first client");
    for step in 1..=8 {
        client
            .write_packet(&ServerboundMovePlayerPos {
                x: 16.5 * f64::from(step),
                y: sync.y,
                z: 0.5,
                flags: MovePlayerFlags::new(true, false),
            })
            .await
            .expect("send exploration move");
    }
    drain_until_chunk(client, (8, 0)).await;
    eprintln!(
        "M37 load exploration moves=8 elapsed_ms={}",
        started.elapsed().as_millis()
    );

    let started = Instant::now();
    client
        .write_packet(&ServerboundChatCommand {
            command: "gamemode creative".to_string(),
        })
        .await
        .expect("creative command");
    client
        .write_packet(&ServerboundChatCommand {
            command: "give minecraft:dirt 64".to_string(),
        })
        .await
        .expect("give dirt command");
    let base_y = sync.y.floor() as i32 - 1;
    let storm_x = 16 * 8;
    for sequence in 1..=16 {
        client
            .write_packet(&ServerboundMovePlayerPos {
                x: f64::from(storm_x + sequence) + 0.5,
                y: sync.y,
                z: 0.5,
                flags: MovePlayerFlags::new(true, false),
            })
            .await
            .expect("move for block storm");
        client
            .write_packet(&ServerboundUseItemOn {
                hand: InteractionHand::MainHand,
                position: pack_block_pos(storm_x + sequence, base_y, 0),
                direction: Direction::Up,
                cursor_x: 0.5,
                cursor_y: 1.0,
                cursor_z: 0.5,
                inside: false,
                world_border_hit: false,
                sequence,
            })
            .await
            .expect("place dirt");
    }
    let acks = drain_counting(client, Duration::from_secs(3), BlockChangedAck::ID).await;
    eprintln!(
        "M37 load block_edit_storm attempts=16 acks={} elapsed_ms={}",
        acks,
        started.elapsed().as_millis()
    );

    let started = Instant::now();
    for idx in 0..8 {
        client
            .write_packet(&ServerboundChatCommand {
                command: format!(
                    "summon minecraft:zombie {} {} {}",
                    storm_x + idx,
                    base_y + 1,
                    3
                ),
            })
            .await
            .expect("summon zombie");
    }
    let spawns = drain_counting(client, Duration::from_secs(5), AddEntity::ID).await;
    eprintln!(
        "M37 load entity_crowd summons=8 add_entity_frames={} elapsed_ms={}",
        spawns,
        started.elapsed().as_millis()
    );

    let pressure = mc_net::lock_pressure_snapshot();
    eprintln!(
        "M42 lock_pressure world(wait={}us max_wait={}us hold={}us max_hold={}us) session(wait={}us max_wait={}us hold={}us max_hold={}us) save(wait={}us max_wait={}us hold={}us max_hold={}us) chunk_prepare(wait={}us max_wait={}us hold={}us max_hold={}us) player_persistence(wait={}us max_wait={}us hold={}us max_hold={}us)",
        pressure.world_storage.wait_us,
        pressure.world_storage.max_wait_us,
        pressure.world_storage.hold_us,
        pressure.world_storage.max_hold_us,
        pressure.session_registry.wait_us,
        pressure.session_registry.max_wait_us,
        pressure.session_registry.hold_us,
        pressure.session_registry.max_hold_us,
        pressure.save_all_flush.wait_us,
        pressure.save_all_flush.max_wait_us,
        pressure.save_all_flush.hold_us,
        pressure.save_all_flush.max_hold_us,
        pressure.chunk_prepare.wait_us,
        pressure.chunk_prepare.max_wait_us,
        pressure.chunk_prepare.hold_us,
        pressure.chunk_prepare.max_hold_us,
        pressure.player_persistence.wait_us,
        pressure.player_persistence.max_wait_us,
        pressure.player_persistence.hold_us,
        pressure.player_persistence.max_hold_us,
    );
}

struct LoadServer {
    addr: std::net::SocketAddr,
    blocks: Arc<mc_world::BlockRegistry>,
    world: Arc<tokio::sync::Mutex<mc_world::WorldStorage>>,
    chunk_pipeline_metrics: mc_net::ChunkPipelineResourceMetrics,
    outbound_pressure: mc_net::OutboundPressureHandle,
    shutdown: mc_net::ShutdownHandle,
    runtime_control: Option<mc_net::RuntimeControlHandle>,
    serve_task: tokio::task::JoinHandle<std::io::Result<()>>,
    chunk_policy: mc_net::ChunkPipelinePolicy,
    chunk_io_threads: usize,
    chunk_worker_threads: usize,
    world_dir: Option<tempfile::TempDir>,
}

impl LoadServer {
    fn outbound_pressure_snapshot(&self) -> mc_net::OutboundPressureSnapshot {
        self.outbound_pressure.snapshot()
    }
}

struct LoadServerOptions {
    view_distance: i32,
    disk_backed: bool,
    runtime_control: bool,
}

impl Default for LoadServerOptions {
    fn default() -> Self {
        Self {
            view_distance: VIEW_DISTANCE,
            disk_backed: false,
            runtime_control: false,
        }
    }
}

async fn start_load_server() -> LoadServer {
    start_load_server_with_options(LoadServerOptions::default()).await
}

async fn start_load_server_with_options(options: LoadServerOptions) -> LoadServer {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    let registries_json = vanilla_dir.join("reports/registries.json");
    if !blocks_json.exists() || !registries_json.exists() {
        panic!(
            "load scenarios degraded: missing {} or {}; tests are ignored unless local vanilla sidecars are available",
            blocks_json.display(),
            registries_json.display()
        );
    }

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&report).expect("registry"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let world_dir = if options.disk_backed {
        let temp = tempfile::tempdir().expect("create disk-backed load world");
        std::fs::create_dir_all(temp.path().join("region")).expect("create legacy region dir");
        Some(temp)
    } else {
        None
    };
    let chunk_capacity = ((2 * options.view_distance + 5) as usize).pow(2);
    let storage = if let Some(temp) = world_dir.as_ref() {
        mc_world::WorldStorage::open_with_capacity(temp.path(), Arc::clone(&blocks), chunk_capacity)
            .expect("open disk-backed load world")
    } else {
        mc_world::WorldStorage::in_memory_with_capacity(Arc::clone(&blocks), chunk_capacity)
    }
    .with_generator(generator);
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let block_light = mc_data::block_light::load(vanilla_dir.join("reports/block_light.json"))
        .ok()
        .map(Arc::new);
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(
        &entity_report,
    ));
    let biome_spawns =
        mc_data::biomes::load_biome_spawn_rules(vanilla_dir.join("data/minecraft/worldgen/biome"))
            .map(Arc::new)
            .unwrap_or_default();

    let mut chunk_pipeline = mc_net::ChunkPipelinePolicy {
        chunk_prepare_batch_size: 2,
        chunk_io_threads: LOAD_CHUNK_IO_THREADS,
        chunk_worker_threads: LOAD_CHUNK_WORKER_THREADS,
        chunk_result_queue_size: 8,
        ..mc_net::ChunkPipelinePolicy::default()
    };
    if options.runtime_control {
        chunk_pipeline.runtime_control = Some(mc_net::RuntimeControlConfig {
            policy: mc_net::AutoscalePolicy::for_profile(mc_net::AutoscaleProfile::Balanced),
            initial_limits: mc_net::RuntimeControlLimits {
                view_distance: options.view_distance,
                chunk_send_rate: 8,
                chunk_load_rate: 16,
                chunk_generate_rate: 32,
            },
        });
    }

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M37 load scenarios".into(),
        max_players: 8,
        view_distance: options.view_distance,
        data,
        blocks: Arc::clone(&blocks),
        world: Some(Arc::clone(&world)),
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light,
        items,
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types,
        biome_spawns,
        chunk_pipeline,
        random_tick: mc_net::RandomTickPolicy {
            random_tick_speed: 3,
            chunk_budget: 8,
            fluid_tick_budget: 64,
            save_interval_ticks: 20,
            seed: 0,
        },
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let shutdown = cfg.shutdown.clone();
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local addr");
    let chunk_pipeline_metrics = bound.chunk_pipeline_metrics();
    let outbound_pressure = bound.outbound_pressure_handle();
    let runtime_control = bound.runtime_control_handle();
    let serve_task = tokio::spawn(async move { bound.serve().await });
    LoadServer {
        addr,
        blocks,
        world,
        chunk_pipeline_metrics,
        outbound_pressure,
        shutdown,
        runtime_control,
        serve_task,
        chunk_policy: chunk_pipeline,
        chunk_io_threads: LOAD_CHUNK_IO_THREADS,
        chunk_worker_threads: LOAD_CHUNK_WORKER_THREADS,
        world_dir,
    }
}

struct PausedReaderClient {
    client: Client,
    sync: SynchronizePlayerPosition,
}

impl PausedReaderClient {
    async fn connect(addr: std::net::SocketAddr, name: &str) -> Self {
        let (mut client, sync) = connect_to_play(addr, name).await;
        drain_until_chunk(&mut client, (0, 0)).await;
        Self { client, sync }
    }

    fn close(self) {
        drop(self.client);
    }
}

async fn find_placeable_target(
    server: &LoadServer,
    x: i32,
    z: i32,
    around_y: i32,
) -> (i32, i32, i32) {
    let mut world = server.world.lock().await;
    for support_y in (-64..=around_y + 8).rev() {
        let support = mc_world::BlockPos { x, y: support_y, z };
        let target = mc_world::BlockPos {
            x,
            y: support_y + 1,
            z,
        };
        let support_state = world.get_block(support).expect("load support column block");
        let target_state = world.get_block(target).expect("load target column block");
        if support_state.is_some_and(|state| !block_state_is_air(world.registry(), state))
            && target_state.is_none_or(|state| block_state_is_air(world.registry(), state))
        {
            return (x, support_y + 1, z);
        }
    }
    panic!("no placeable air block above support found near {x},{z}");
}

fn block_state_is_air(registry: &mc_world::BlockRegistry, state: mc_world::BlockStateId) -> bool {
    registry
        .by_id(state)
        .is_some_and(|entry| entry.block.id.path() == "air")
}

async fn wait_for_outbound_pressure_increase(
    server: &LoadServer,
    before: mc_net::OutboundPressureSnapshot,
) -> mc_net::OutboundPressureSnapshot {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let after = server.outbound_pressure_snapshot();
        if after.reliable_command_retries > before.reliable_command_retries
            || after.visibility_command_drops > before.visibility_command_drops
            || after.slow_client_write_timeouts > before.slow_client_write_timeouts
            || after.slow_client_pressure_sheds > before.slow_client_pressure_sheds
        {
            return after;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "paused reader should make outbound queue pressure observable: before={before:?} after={after:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn assert_slow_reader_retry_bounded(
    before: mc_net::OutboundPressureSnapshot,
    after: mc_net::OutboundPressureSnapshot,
) {
    assert!(
        after.reliable_command_retries_in_flight <= before.reliable_command_retries_in_flight + 1,
        "one paused reader should have at most one reliable retry in flight: before={before:?} after={after:?}"
    );
    assert!(
        after.max_reliable_command_retries_in_flight
            <= before.max_reliable_command_retries_in_flight + 1,
        "one paused reader should not raise max pending reliable retries by more than one: before={before:?} after={after:?}"
    );
}

async fn connect_to_play(
    addr: std::net::SocketAddr,
    name: &str,
) -> (Client, SynchronizePlayerPosition) {
    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client.drive_login(addr, name).await.expect("drive login");
    client
        .drive_configuration()
        .await
        .expect("drive configuration");
    let _ = client.read_play_login().await.expect("play entry");
    let _: mc_protocol::packets::play::ClientboundCommands =
        client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    let _: mc_protocol::packets::play::ClientboundInitializeBorder =
        client.read_typed().await.expect("InitializeBorder");
    let _: mc_protocol::packets::play::ClientboundSetTime =
        client.read_typed().await.expect("SetTime");
    let _: mc_protocol::packets::play::SetDefaultSpawnPosition =
        client.read_typed().await.expect("SetDefaultSpawnPosition");
    let _: GameEvent = client.read_typed().await.expect("GameEvent");
    let _: SetCenterChunk = client.read_typed().await.expect("SetCenterChunk");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");
    (client, sync)
}

async fn drain_until_chunk(client: &mut Client, target: (i32, i32)) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("drain chunk");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            let mut body = frame.body;
            let pkt = LevelChunkWithLight::decode(&mut body).expect("decode chunk");
            if (pkt.chunk_x, pkt.chunk_z) == target {
                return;
            }
        }
    }
}

async fn drain_unique_chunks(
    client: &mut Client,
    expected: usize,
) -> std::collections::BTreeSet<(i32, i32)> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut seen = std::collections::BTreeSet::new();
    loop {
        if seen.len() >= expected {
            return seen;
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for {expected} unique chunks; seen={seen:?}"
        );
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .expect("drain unique chunks");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            let mut body = frame.body;
            let pkt = LevelChunkWithLight::decode(&mut body).expect("decode chunk");
            seen.insert((pkt.chunk_x, pkt.chunk_z));
        }
    }
}

async fn wait_for_shutdown_requested(shutdown: &mc_net::ShutdownHandle) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if shutdown.is_requested() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for player /stop to request shutdown"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn drain_counting(client: &mut Client, duration: Duration, packet_id: i32) -> usize {
    let deadline = tokio::time::Instant::now() + duration;
    let mut count = 0usize;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return count;
        }
        let Ok(frame) = client.read_frame_with_timeout(remaining).await else {
            return count;
        };
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == packet_id {
            count += 1;
        }
    }
}

async fn drain_counting_until(
    client: &mut Client,
    duration: Duration,
    packet_id: i32,
    expected: usize,
) -> usize {
    let deadline = tokio::time::Instant::now() + duration;
    let mut count = 0usize;
    loop {
        if count >= expected {
            return count;
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return count;
        }
        let Ok(frame) = client.read_frame_with_timeout(remaining).await else {
            return count;
        };
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == packet_id {
            count += 1;
        }
    }
}

async fn wait_for_ack(client: &mut Client, expected_sequence: i32) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return false;
        }
        let Ok(frame) = client.read_frame_with_timeout(remaining).await else {
            return false;
        };
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode BlockChangedAck");
            if pkt.sequence == expected_sequence {
                return true;
            }
        }
    }
}

async fn wait_for_inventory_stack(client: &mut Client) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for server-authoritative inventory stack"
        );
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .expect("inventory stack sync frame");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode inventory ContainerSetSlot");
            if pkt.container_id == 0 && !pkt.item_stack.is_empty() {
                return;
            }
        }
    }
}

async fn drain_target_block_updates(
    client: &mut Client,
    target_pos: i64,
    duration: Duration,
) -> usize {
    let target = unpack_block_pos(target_pos);
    let target_section = pack_section_pos(
        target.0.div_euclid(16),
        target.1.div_euclid(16),
        target.2.div_euclid(16),
    );
    let target_relative = pack_section_relative_pos(target.0, target.1, target.2);
    let deadline = tokio::time::Instant::now() + duration;
    let mut updates = 0usize;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return updates;
        }
        let Ok(frame) = client.read_frame_with_timeout(remaining).await else {
            return updates;
        };
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode BlockUpdate");
            if unpack_block_pos(pkt.position) == target {
                updates += 1;
            }
        } else if frame.id == SectionBlocksUpdate::ID {
            let mut body = frame.body;
            let pkt = SectionBlocksUpdate::decode(&mut body).expect("decode SectionBlocksUpdate");
            if pkt.section_pos == target_section {
                updates += pkt
                    .changes
                    .iter()
                    .filter(|change| change.relative_pos == target_relative)
                    .count();
            }
        }
    }
}

async fn handle_keepalive(client: &mut Client, id: i32, body: &bytes::Bytes) -> bool {
    if id != ClientboundKeepAlive::ID {
        return false;
    }
    let mut body = body.clone();
    let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode KeepAlive");
    client
        .write_packet(&ServerboundKeepAlive { id: keepalive.id })
        .await
        .expect("echo KeepAlive");
    true
}
