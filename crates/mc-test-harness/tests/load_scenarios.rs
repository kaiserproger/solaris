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
    AddEntity, BlockChangedAck, BlockUpdate, ClientboundContainerSetContent,
    ClientboundContainerSetSlot, ClientboundKeepAlive, ClientboundOpenScreen,
    ClientboundSetEntityData, ClientboundSetExperience, ClientboundSetHealth, ConfirmTeleportation,
    ContainerInput, Direction, EntityDataValue, GameEvent, HashedStack, HashedStackComponentHashes,
    InteractionHand, LevelChunkWithLight, MovePlayerFlags, SectionBlocksUpdate,
    ServerboundChatCommand, ServerboundContainerClick, ServerboundKeepAlive,
    ServerboundMovePlayerPos, ServerboundMovePlayerStatusOnly, ServerboundUseItemOn,
    SetCenterChunk, SynchronizePlayerPosition, pack_block_pos, pack_section_pos,
    pack_section_relative_pos, unpack_block_pos,
};
use mc_test_harness::client::Client;
use mc_test_harness::replay::{
    ReplayConcurrentAction, ReplayConcurrentFixture, ReplayConcurrentGroup, ReplayScenarioManifest,
    ReplayStateObservation, ReplayStateValue,
};
use serde::Serialize;

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
const DETERMINISTIC_OUTBOUND_PRESSURE_BURST: usize = 192;
const M96_REPLAY_ELAPSED_BUDGET: Duration = Duration::from_secs(45);
const M96_LOCK_MAX_HOLD_BUDGET_US: u64 = 250_000;
const M96_CANCELLED_REQUEST_BUDGET: usize = 64;
const O2_STOP_FLUSH_CLIENTS: usize = 4;
const O2_STOP_VIEW_DISTANCE: i32 = 8;
const O2_VD8_CONCURRENT_CLIENTS: usize = 20;
const O2_VD8_WINDOW_EDGE: usize = (O2_STOP_VIEW_DISTANCE as usize * 2) + 1;
const O2_VD8_WINDOW_CHUNKS: usize = O2_VD8_WINDOW_EDGE * O2_VD8_WINDOW_EDGE;
const O2_VD8_JOIN_ELAPSED_BUDGET: Duration = Duration::from_secs(120);
const O2_VD8_LOCK_MAX_HOLD_BUDGET_US: u64 = 1_000_000;
const O2_VD8_FIRST_CHUNK_P99_BUDGET_MS: u64 = 2_500;
const O2_VD8_TICK_BUDGET_US: u64 = 50_000;
const O2_VD8_ENTITY_PHYSICS_MAX_BUDGET_US: u64 = 50_000;
const O2_VD8_QUEUE_EMPTY_STOP_BUDGET: usize = 100_000;
const O2_VD8_CHUNK_PREPARE_HOLD_COUNT_BUDGET: u64 = (O2_VD8_WINDOW_CHUNKS as u64) * 16;
const O2_VD8_CHUNK_RESULT_QUEUE_SIZE: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct WorkloadLatencyPercentilesMs {
    samples: usize,
    p50_ms: u64,
    p95_ms: u64,
    p99_ms: u64,
    max_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct ChunkWindowLatencyMs {
    first_ms: u64,
    ring1_ms: u64,
    ring2_ms: u64,
    full_ms: u64,
}

struct TimedChunkDrain {
    chunks: std::collections::BTreeSet<(i32, i32)>,
    latency: ChunkWindowLatencyMs,
}

fn workload_latency_percentiles_ms(
    values: impl IntoIterator<Item = u64>,
) -> WorkloadLatencyPercentilesMs {
    let mut values = values.into_iter().collect::<Vec<_>>();
    assert!(
        !values.is_empty(),
        "workload latency sample must not be empty"
    );
    values.sort_unstable();
    let nearest_rank = |percentile: usize| {
        let rank = percentile
            .saturating_mul(values.len())
            .div_ceil(100)
            .clamp(1, values.len());
        values[rank - 1]
    };
    WorkloadLatencyPercentilesMs {
        samples: values.len(),
        p50_ms: nearest_rank(50),
        p95_ms: nearest_rank(95),
        p99_ms: nearest_rank(99),
        max_ms: *values.last().expect("non-empty workload latency sample"),
    }
}

#[test]
fn prompt01_workload_latency_percentiles_use_nearest_rank() {
    let percentiles = workload_latency_percentiles_ms([1, 2, 3, 4, 100]);

    assert_eq!(percentiles.samples, 5);
    assert_eq!(percentiles.p50_ms, 3);
    assert_eq!(percentiles.p95_ms, 100);
    assert_eq!(percentiles.p99_ms, 100);
    assert_eq!(percentiles.max_ms, 100);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires local data/vanilla sidecars; degraded when absent"]
async fn checked_multiplayer_transaction_replay_is_deterministic_and_conservative() {
    let manifest = ReplayScenarioManifest::from_json(include_str!(
        "../../../tools/core-replay-scenarios/multiplayer-transactions-seed-8102.json"
    ))
    .expect("checked multiplayer transaction replay parses");

    let first = run_checked_multiplayer_transaction_replay(&manifest).await;
    let second = run_checked_multiplayer_transaction_replay(&manifest).await;

    if first != second {
        let paths = persist_minimal_replay_failures(
            &manifest,
            manifest
                .concurrent_groups
                .iter()
                .map(|group| group.id.as_str()),
        );
        panic!(
            "same seed did not normalize identically; replay manifests: {paths:?}\nfirst={first:?}\nsecond={second:?}"
        );
    }
    if let Err(groups) = replay_state_mismatches(&manifest, &first) {
        let paths = persist_minimal_replay_failures(&manifest, groups.iter().map(String::as_str));
        panic!(
            "multiplayer transaction conservation failed; replay manifests: {paths:?}\nobservations={first:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires local data/vanilla sidecars; degraded when absent"]
async fn duplicate_lethal_player_commands_drop_one_bundle_and_survive_restart() {
    run_player_death_restart_gate().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires local data/vanilla sidecars; 400-tick Prompt 02 preflight"]
async fn prompt02_multiplayer_transaction_soak_short_preflight() {
    tokio::time::timeout(
        Duration::from_secs(90),
        run_prompt02_multiplayer_transaction_soak(Prompt02SoakWorkload {
            target_ticks: 400,
            transaction_interval_ticks: 100,
        }),
    )
    .await
    .expect("400-tick Prompt 02 preflight exceeded its failure timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "36000-tick Prompt 02 fallback soak; requires local data/vanilla sidecars"]
async fn prompt02_four_active_one_slow_reader_transaction_soak_36000_ticks() {
    tokio::time::timeout(
        Duration::from_secs(45 * 60),
        run_prompt02_multiplayer_transaction_soak(Prompt02SoakWorkload {
            target_ticks: 36_000,
            transaction_interval_ticks: 1_200,
        }),
    )
    .await
    .expect("36000-tick Prompt 02 soak exceeded its failure timeout");
}

#[derive(Debug, Clone, Copy)]
struct Prompt02SoakWorkload {
    target_ticks: u64,
    transaction_interval_ticks: u64,
}

#[derive(Debug, Clone, Copy)]
enum Prompt02ClientCommand {
    Liveness,
    SummonZombie,
}

async fn run_prompt02_multiplayer_transaction_soak(workload: Prompt02SoakWorkload) {
    const ACTIVE_CLIENTS: usize = 4;
    const PRESSURE_INTERVAL_TICKS: u64 = 100;
    const LIVENESS_INTERVAL_TICKS: u64 = 20;
    const SUMMON_INTERVAL_TICKS: u64 = 600;

    let manifest = ReplayScenarioManifest::from_json(include_str!(
        "../../../tools/core-replay-scenarios/multiplayer-transactions-seed-8102.json"
    ))
    .expect("checked multiplayer transaction soak manifest parses");
    let world_dir = tempfile::tempdir().expect("create Prompt 02 soak world");
    let server = start_load_server_with_options(LoadServerOptions {
        disk_backed: true,
        existing_world_path: Some(world_dir.path().to_owned()),
        spawn_passive_entities: false,
        max_players: 8,
        ..LoadServerOptions::default()
    })
    .await;
    let addr = server.addr;
    let pressure_before = server.outbound_pressure_snapshot();
    let started = Instant::now();

    let mut active_commands = Vec::with_capacity(ACTIVE_CLIENTS);
    let mut active_tasks = Vec::with_capacity(ACTIVE_CLIENTS);
    for index in 0..ACTIVE_CLIENTS {
        let (mut client, _) = connect_to_play(addr, &format!("P02SoakA{index}")).await;
        let chunks = drain_unique_chunks(&mut client, 9).await;
        assert_eq!(chunks.len(), 9, "active soak client must finish VD1 stream");
        let (commands, task) = spawn_prompt02_active_client(client);
        active_commands.push(commands);
        active_tasks.push(task);
    }
    let mut slow_generation = 0usize;
    let mut paused_reader = connect_prompt02_slow_reader(addr, slow_generation).await;
    let mut pressure_probes = 0usize;
    let mut transaction_samples = 0usize;
    let mut simulation_ticks = server.runtime_telemetry.subscribe_simulation_ticks();
    let start_tick = *simulation_ticks.borrow_and_update();
    let target_tick = start_tick.saturating_add(workload.target_ticks);
    let mut current_tick = start_tick;
    let mut next_pressure = start_tick.saturating_add(1);
    let mut next_transaction = start_tick.saturating_add(1);
    let mut next_liveness = start_tick.saturating_add(1);
    let mut next_summon = start_tick.saturating_add(1);

    while current_tick < target_tick {
        simulation_ticks
            .changed()
            .await
            .expect("simulation tick sender remains active");
        current_tick = *simulation_ticks.borrow_and_update();
        let scheduled_through = current_tick.min(target_tick);
        let active_sessions = server.runtime_telemetry.snapshot().active_sessions;
        assert!(
            active_sessions >= ACTIVE_CLIENTS,
            "Prompt 02 soak lost an active client: active={active_sessions}"
        );
        if active_sessions == ACTIVE_CLIENTS {
            paused_reader.close();
            slow_generation += 1;
            paused_reader = connect_prompt02_slow_reader(addr, slow_generation).await;
        }

        while next_pressure <= scheduled_through {
            if paused_reader
                .try_trigger_outbound_pressure(DETERMINISTIC_OUTBOUND_PRESSURE_BURST)
                .await
            {
                pressure_probes += 1;
            }
            next_pressure = next_pressure.saturating_add(PRESSURE_INTERVAL_TICKS);
        }
        while next_liveness <= scheduled_through {
            for commands in &active_commands {
                commands
                    .send(Prompt02ClientCommand::Liveness)
                    .await
                    .expect("active Prompt 02 client task remains");
            }
            next_liveness = next_liveness.saturating_add(LIVENESS_INTERVAL_TICKS);
        }
        while next_summon <= scheduled_through {
            active_commands[0]
                .send(Prompt02ClientCommand::SummonZombie)
                .await
                .expect("primary Prompt 02 client task remains");
            next_summon = next_summon.saturating_add(SUMMON_INTERVAL_TICKS);
        }
        while next_transaction <= scheduled_through {
            run_prompt02_soak_transaction_sample(&server, &manifest, transaction_samples).await;
            transaction_samples += 1;
            next_transaction = next_transaction.saturating_add(workload.transaction_interval_ticks);
        }
    }

    let tick_percentiles = server
        .runtime_telemetry
        .snapshot()
        .tick_percentiles
        .expect("tick metrics worker must publish during a 400+ tick soak");
    assert!(
        (start_tick..=current_tick).contains(&tick_percentiles.source_tick),
        "tick percentile provenance must belong to this soak: start={start_tick} current={current_tick} snapshot={tick_percentiles:?}"
    );

    paused_reader.close();
    drop(active_commands);
    let mut active_entity_spawns = 0usize;
    for task in active_tasks {
        active_entity_spawns += task.await.expect("active Prompt 02 client task joins");
    }
    assert!(
        active_entity_spawns > 0,
        "healthy Prompt 02 soak clients must observe entity broadcasts"
    );
    assert!(
        transaction_samples >= 2,
        "Prompt 02 soak must include at least start/end transaction samples"
    );

    wait_for_active_sessions(&server, 0).await;
    let save_report = server.save_handle.save_all().await;
    assert!(
        save_report.is_ok(),
        "Prompt 02 soak final save failed: {save_report:?}"
    );
    let pressure_after = server.outbound_pressure_snapshot();
    assert_ne!(
        pressure_after, pressure_before,
        "Prompt 02 soak slow-reader pressure must be observable"
    );
    assert_eq!(
        pressure_after.reliable_command_drops, pressure_before.reliable_command_drops,
        "Prompt 02 soak must not lose reliable commands"
    );
    let chunk_snapshot = server.chunk_pipeline_metrics.snapshot();
    assert_eq!(
        chunk_snapshot.active_cpu, 0,
        "Prompt 02 soak must drain chunk CPU work"
    );
    assert_eq!(
        chunk_snapshot.active_io, 0,
        "Prompt 02 soak must drain chunk IO work"
    );
    let elapsed = started.elapsed();
    shutdown_load_server(server, "Prompt 02 tick soak").await;
    eprintln!(
        "Prompt 02 tick soak target_ticks={} observed_ticks={} elapsed_s={} active_clients={} slow_generations={} pressure_probes={} transaction_samples={} active_entity_spawns={} percentile_source_tick={} percentile_submit_us={} percentile_compute_us={} percentile_skipped={} outbound_before={:?} outbound_after={:?} saved_entities={} flushed_chunks={}",
        workload.target_ticks,
        current_tick.saturating_sub(start_tick),
        elapsed.as_secs(),
        ACTIVE_CLIENTS,
        slow_generation + 1,
        pressure_probes,
        transaction_samples,
        active_entity_spawns,
        tick_percentiles.source_tick,
        tick_percentiles.observer_submit_us,
        tick_percentiles.observer_compute_us,
        tick_percentiles.observer_skipped_windows,
        pressure_before,
        pressure_after,
        save_report.entities_saved,
        save_report.chunks_flushed,
    );
}

async fn connect_prompt02_slow_reader(
    addr: std::net::SocketAddr,
    generation: usize,
) -> PausedReaderClient {
    PausedReaderClient::connect(addr, &format!("P02Slow{:04}", generation % 10_000)).await
}

fn spawn_prompt02_active_client(
    mut client: Client,
) -> (
    tokio::sync::mpsc::Sender<Prompt02ClientCommand>,
    tokio::task::JoinHandle<usize>,
) {
    let (commands, mut command_rx) = tokio::sync::mpsc::channel(8);
    let task = tokio::spawn(async move {
        let mut entity_spawns = 0usize;
        loop {
            tokio::select! {
                command = command_rx.recv() => {
                    let Some(command) = command else {
                        return entity_spawns;
                    };
                    match command {
                        Prompt02ClientCommand::Liveness => {
                            client
                                .write_packet(&ServerboundMovePlayerStatusOnly {
                                    flags: MovePlayerFlags::new(true, false),
                                })
                                .await
                                .expect("send Prompt 02 soak liveness movement");
                        }
                        Prompt02ClientCommand::SummonZombie => {
                            client
                                .write_packet(&ServerboundChatCommand {
                                    command: "summon minecraft:zombie".into(),
                                })
                                .await
                                .expect("summon Prompt 02 soak visibility entity");
                        }
                    }
                }
                frame = client.read_frame() => {
                    let frame = frame.expect("read active Prompt 02 client frame");
                    if handle_keepalive(&mut client, frame.id, &frame.body).await {
                        continue;
                    }
                    entity_spawns += usize::from(frame.id == AddEntity::ID);
                }
            }
        }
    });
    (commands, task)
}

async fn run_prompt02_soak_transaction_sample(
    server: &LoadServer,
    manifest: &ReplayScenarioManifest,
    sample: usize,
) {
    for (offset, group) in manifest.concurrent_groups.iter().enumerate() {
        let observation_id = &manifest
            .state_expectations
            .iter()
            .find(|expectation| expectation.after_group == group.id)
            .expect("validated soak group expectation")
            .id;
        let baseline_sessions = server.runtime_telemetry.snapshot().active_sessions;
        let group_index = 100usize + sample * manifest.concurrent_groups.len() + offset;
        let (observation, probe) = match group.fixture {
            ReplayConcurrentFixture::SameTargetPlacement => {
                replay_same_target_placement(
                    server,
                    manifest.seed,
                    group_index,
                    group,
                    observation_id,
                )
                .await
            }
            ReplayConcurrentFixture::SharedChest { .. } => {
                replay_shared_chest_pickup(
                    server,
                    manifest.seed,
                    group_index,
                    group,
                    observation_id,
                )
                .await
            }
        };
        wait_for_active_sessions_at_most(server, baseline_sessions).await;
        assert_live_replay_observation(manifest, &observation);
        cleanup_transaction_replay_probe(server, probe).await;
    }
}

fn assert_live_replay_observation(
    manifest: &ReplayScenarioManifest,
    actual: &ReplayStateObservation,
) {
    let expected = manifest
        .state_expectations
        .iter()
        .find(|expectation| expectation.id == actual.id)
        .expect("live replay observation expectation");
    let actual_values = actual
        .values
        .iter()
        .map(|value| (value.key.as_str(), value.value))
        .collect::<std::collections::BTreeMap<_, _>>();
    for expected in &expected.values {
        if expected.key.starts_with("persisted_") {
            continue;
        }
        assert_eq!(
            actual_values.get(expected.key.as_str()),
            Some(&expected.value),
            "Prompt 02 soak state mismatch for {}/{}",
            actual.id,
            expected.key
        );
    }
}

async fn cleanup_transaction_replay_probe(
    server: &LoadServer,
    probe: TransactionReplayPersistenceProbe,
) {
    let position = match probe {
        TransactionReplayPersistenceProbe::Placement { position, .. }
        | TransactionReplayPersistenceProbe::SharedChest { position, .. } => position,
    };
    let air = server
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .expect("air block")
        .default;
    server
        .world
        .lock()
        .await
        .set_block_at(position, air)
        .expect("clean Prompt 02 soak transaction fixture");
}

#[derive(Debug, Default)]
struct PlayerDeathBundleObservation {
    health_zero: bool,
    xp_reset: bool,
    inventory_empty: bool,
    item_entity_ids: std::collections::BTreeSet<i32>,
    matching_item_entities: std::collections::BTreeSet<i32>,
    xp_values: Vec<i32>,
}

impl PlayerDeathBundleObservation {
    fn complete(&self) -> bool {
        self.health_zero
            && self.xp_reset
            && self.inventory_empty
            && self.matching_item_entities.len() == 1
            && self.xp_values.len() == 1
    }

    fn assert_exact(&self, expected_xp: i32) {
        assert!(self.health_zero, "death bundle must publish zero health");
        assert!(self.xp_reset, "death bundle must reset player XP");
        assert!(
            self.inventory_empty,
            "death bundle must clear authoritative inventory"
        );
        assert_eq!(
            self.matching_item_entities.len(),
            1,
            "death bundle must create one matching item entity: {self:?}"
        );
        assert_eq!(
            self.xp_values,
            vec![expected_xp],
            "death bundle must create one exact XP orb: {self:?}"
        );
    }
}

async fn run_player_death_restart_gate() {
    const XP_POINTS: i32 = 35;
    const APPLE_COUNT: i32 = 2;

    let world_dir = tempfile::tempdir().expect("create player death replay world");
    let world_path = world_dir.path().to_owned();
    let server = start_load_server_with_options(LoadServerOptions {
        disk_backed: true,
        existing_world_path: Some(world_path.clone()),
        spawn_passive_entities: false,
        ..LoadServerOptions::default()
    })
    .await;
    let item_entity_type = replay_entity_type_id(&server, "minecraft:item");
    let xp_entity_type = replay_entity_type_id(&server, "minecraft:experience_orb");
    let apple_id = server
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:apple").unwrap())
        .expect("apple item id");

    let victim_profile = "P02DeathVictim";
    let (mut victim, _) = connect_to_play(server.addr, victim_profile).await;
    let victim_chunks = drain_unique_chunks(&mut victim, 9).await;
    assert_eq!(victim_chunks.len(), 9, "death gate must finish VD1 stream");
    victim
        .write_packet(&ServerboundChatCommand {
            command: format!("debug give minecraft:apple {APPLE_COUNT} 0"),
        })
        .await
        .expect("seed player death inventory");
    wait_for_inventory_stack(&mut victim).await;
    victim
        .write_packet(&ServerboundChatCommand {
            command: format!("debug survival xp {XP_POINTS}"),
        })
        .await
        .expect("seed player death XP");
    wait_for_experience_total(&mut victim, XP_POINTS).await;

    for _ in 0..2 {
        victim
            .write_packet(&ServerboundChatCommand {
                command: "debug survival damage 100".into(),
            })
            .await
            .expect("queue duplicate lethal player command");
    }
    let before_restart = observe_player_death_bundle(
        &mut victim,
        item_entity_type,
        xp_entity_type,
        apple_id,
        APPLE_COUNT,
        true,
    )
    .await;
    before_restart.assert_exact(XP_POINTS);
    drop(victim);
    wait_for_active_sessions(&server, 0).await;
    let save_report = server.save_handle.save_all().await;
    eprintln!(
        "player death before_restart items={} xp={:?} saved_entities={} flushed_chunks={}",
        before_restart.matching_item_entities.len(),
        before_restart.xp_values,
        save_report.entities_saved,
        save_report.chunks_flushed,
    );
    assert!(
        save_report.is_ok(),
        "player death save-all failed: {save_report:?}"
    );
    shutdown_load_server(server, "player death pre-restart").await;

    let restarted = start_load_server_with_options(LoadServerOptions {
        disk_backed: true,
        existing_world_path: Some(world_path),
        spawn_passive_entities: false,
        ..LoadServerOptions::default()
    })
    .await;
    eprintln!(
        "player death restarted entities={}",
        restarted.runtime_telemetry.snapshot().server_entities
    );
    let (mut victim_rejoined, _) = connect_to_play(restarted.addr, victim_profile).await;
    let after_restart = observe_player_death_bundle(
        &mut victim_rejoined,
        item_entity_type,
        xp_entity_type,
        apple_id,
        APPLE_COUNT,
        true,
    )
    .await;
    after_restart.assert_exact(XP_POINTS);
    drop(victim_rejoined);
    wait_for_active_sessions(&restarted, 0).await;
    shutdown_load_server(restarted, "player death restarted").await;
}

fn replay_entity_type_id(server: &LoadServer, name: &str) -> i32 {
    server
        .entity_types
        .id_of(&mc_data::Identifier::parse(name).expect("checked entity identifier"))
        .and_then(|id| i32::try_from(id).ok())
        .unwrap_or_else(|| panic!("missing replay entity type {name}"))
}

async fn wait_for_experience_total(client: &mut Client, expected: i32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "timed out waiting for XP {expected}");
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .expect("experience sync frame");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetExperience::ID {
            let mut body = frame.body;
            let packet = ClientboundSetExperience::decode(&mut body)
                .expect("decode ClientboundSetExperience");
            if packet.total_experience == expected {
                return;
            }
        }
    }
}

async fn observe_player_death_bundle(
    client: &mut Client,
    item_entity_type: i32,
    xp_entity_type: i32,
    expected_item_id: u32,
    expected_item_count: i32,
    require_player_state: bool,
) -> PlayerDeathBundleObservation {
    let mut observation = PlayerDeathBundleObservation::default();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut completed_at = None;
    loop {
        let now = tokio::time::Instant::now();
        if completed_at.is_none()
            && ((!require_player_state
                && observation.matching_item_entities.len() == 1
                && observation.xp_values.len() == 1)
                || (require_player_state && observation.complete()))
        {
            completed_at = Some(now);
        }
        if completed_at
            .is_some_and(|completed| now.duration_since(completed) >= Duration::from_millis(500))
        {
            break;
        }
        assert!(
            now < deadline,
            "timed out observing death bundle: {observation:?}"
        );
        let remaining = deadline
            .saturating_duration_since(now)
            .min(Duration::from_millis(500));
        let frame = match client.read_frame_with_timeout(remaining).await {
            Ok(frame) => frame,
            Err(_) if completed_at.is_some() => break,
            Err(error) => panic!("death bundle frame failed: {error}; {observation:?}"),
        };
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        match frame.id {
            id if id == AddEntity::ID => {
                let mut body = frame.body;
                let packet = AddEntity::decode(&mut body).expect("decode death AddEntity");
                if packet.entity_type_id == item_entity_type {
                    observation.item_entity_ids.insert(packet.entity_id);
                } else if packet.entity_type_id == xp_entity_type {
                    observation.xp_values.push(packet.data);
                }
            }
            id if id == ClientboundSetEntityData::ID => {
                let mut body = frame.body;
                let packet =
                    ClientboundSetEntityData::decode(&mut body).expect("decode death entity data");
                if observation.item_entity_ids.contains(&packet.entity_id)
                    && packet.values.iter().any(|value| {
                        matches!(
                            value,
                            EntityDataValue::ItemStack { stack, .. }
                                if stack.item_id == expected_item_id
                                    && stack.count == expected_item_count
                        )
                    })
                {
                    observation.matching_item_entities.insert(packet.entity_id);
                }
            }
            id if id == ClientboundSetHealth::ID => {
                let mut body = frame.body;
                let packet = ClientboundSetHealth::decode(&mut body).expect("decode death health");
                observation.health_zero |= packet.health == 0.0;
            }
            id if id == ClientboundSetExperience::ID => {
                let mut body = frame.body;
                let packet =
                    ClientboundSetExperience::decode(&mut body).expect("decode death experience");
                observation.xp_reset |= packet.total_experience == 0;
            }
            id if id == ClientboundContainerSetContent::ID => {
                let mut body = frame.body;
                let packet = ClientboundContainerSetContent::decode(&mut body)
                    .expect("decode death inventory");
                if packet.container_id == 0 {
                    observation.inventory_empty |= packet
                        .items
                        .iter()
                        .all(|stack| stack.item_id != expected_item_id || stack.count == 0)
                        && (packet.carried_item.item_id != expected_item_id
                            || packet.carried_item.count == 0);
                }
            }
            _ => {}
        }
    }
    observation.xp_values.sort_unstable();
    observation
}

async fn run_checked_multiplayer_transaction_replay(
    manifest: &ReplayScenarioManifest,
) -> Vec<ReplayStateObservation> {
    let world_dir = tempfile::tempdir().expect("create transaction replay world");
    let world_path = world_dir.path().to_owned();
    let server = start_load_server_with_options(LoadServerOptions {
        disk_backed: true,
        existing_world_path: Some(world_path.clone()),
        ..LoadServerOptions::default()
    })
    .await;
    let mut observations = Vec::with_capacity(manifest.concurrent_groups.len());
    let mut persistence_probes = Vec::with_capacity(manifest.concurrent_groups.len());

    for (group_index, group) in manifest.concurrent_groups.iter().enumerate() {
        let observation_id = &manifest
            .state_expectations
            .iter()
            .find(|expectation| expectation.after_group == group.id)
            .expect("validated replay group has a state expectation")
            .id;
        let (observation, persistence_probe) = match &group.fixture {
            ReplayConcurrentFixture::SameTargetPlacement => {
                replay_same_target_placement(
                    &server,
                    manifest.seed,
                    group_index,
                    group,
                    observation_id,
                )
                .await
            }
            ReplayConcurrentFixture::SharedChest { .. } => {
                replay_shared_chest_pickup(
                    &server,
                    manifest.seed,
                    group_index,
                    group,
                    observation_id,
                )
                .await
            }
        };
        observations.push(observation);
        persistence_probes.push(persistence_probe);
        wait_for_active_sessions(&server, 0).await;
    }

    let save_report = server.save_handle.save_all().await;
    eprintln!(
        "transaction replay save entities={} chunks={} errors={}",
        save_report.entities_saved,
        save_report.chunks_flushed,
        save_report.errors.len(),
    );
    assert!(
        save_report.is_ok(),
        "transaction replay save-all failed: {save_report:?}"
    );
    shutdown_load_server(server, "transaction replay pre-restart").await;

    let restarted = start_load_server_with_options(LoadServerOptions {
        disk_backed: true,
        existing_world_path: Some(world_path),
        ..LoadServerOptions::default()
    })
    .await;
    apply_transaction_restart_observations(&restarted, &persistence_probes, &mut observations)
        .await;
    shutdown_load_server(restarted, "transaction replay restarted").await;
    for observation in &mut observations {
        observation
            .values
            .sort_by(|left, right| left.key.cmp(&right.key));
    }
    observations
}

#[derive(Debug)]
enum TransactionReplayPersistenceProbe {
    Placement {
        observation_id: String,
        position: mc_world::BlockPos,
        state: mc_world::BlockStateId,
    },
    SharedChest {
        observation_id: String,
        position: mc_world::BlockPos,
        item_id: u32,
        profiles: [String; 2],
    },
}

async fn shutdown_load_server(server: LoadServer, context: &str) {
    server.shutdown.request();
    let serve_result = tokio::time::timeout(Duration::from_secs(5), server.serve_task)
        .await
        .unwrap_or_else(|_| panic!("{context} server shutdown timed out"))
        .unwrap_or_else(|error| panic!("{context} server task failed: {error}"));
    serve_result.unwrap_or_else(|error| panic!("{context} server failed: {error}"));
}

async fn apply_transaction_restart_observations(
    server: &LoadServer,
    probes: &[TransactionReplayPersistenceProbe],
    observations: &mut [ReplayStateObservation],
) {
    for probe in probes {
        match probe {
            TransactionReplayPersistenceProbe::Placement {
                observation_id,
                position,
                state,
            } => {
                let persisted = server
                    .world
                    .lock()
                    .await
                    .get_block(*position)
                    .expect("read restarted placement block")
                    == Some(*state);
                replay_observation_mut(observations, observation_id)
                    .values
                    .push(ReplayStateValue {
                        key: "persisted_authoritative_blocks".into(),
                        value: i64::from(persisted),
                    });
            }
            TransactionReplayPersistenceProbe::SharedChest {
                observation_id,
                position,
                item_id,
                profiles,
            } => {
                let persisted_container_items = server
                    .world
                    .lock()
                    .await
                    .chest_block_entity(*position)
                    .expect("read restarted replay chest")
                    .expect("restarted replay chest exists")
                    .slots
                    .iter()
                    .filter(|stack| stack.item_id == *item_id)
                    .map(|stack| stack.count)
                    .sum::<i32>();
                let (mut left, _) = connect_to_play(server.addr, &profiles[0]).await;
                let (mut right, _) = connect_to_play(server.addr, &profiles[1]).await;
                let (left_inventory, right_inventory) = tokio::join!(
                    wait_for_load_container_content(&mut left, 0, |_| true),
                    wait_for_load_container_content(&mut right, 0, |_| true),
                );
                let persisted_player_items = [&left_inventory, &right_inventory]
                    .into_iter()
                    .flat_map(|inventory| inventory.items.iter().chain([&inventory.carried_item]))
                    .filter(|stack| stack.item_id == *item_id)
                    .map(|stack| stack.count)
                    .sum::<i32>();
                drop(left);
                drop(right);
                wait_for_active_sessions(server, 0).await;

                replay_observation_mut(observations, observation_id)
                    .values
                    .extend([
                        ReplayStateValue {
                            key: "persisted_container_items".into(),
                            value: i64::from(persisted_container_items),
                        },
                        ReplayStateValue {
                            key: "persisted_player_items".into(),
                            value: i64::from(persisted_player_items),
                        },
                        ReplayStateValue {
                            key: "persisted_total_items".into(),
                            value: i64::from(persisted_container_items + persisted_player_items),
                        },
                    ]);
            }
        }
    }
}

fn replay_observation_mut<'a>(
    observations: &'a mut [ReplayStateObservation],
    id: &str,
) -> &'a mut ReplayStateObservation {
    observations
        .iter_mut()
        .find(|observation| observation.id == id)
        .unwrap_or_else(|| panic!("missing transaction replay observation {id}"))
}

async fn replay_same_target_placement(
    server: &LoadServer,
    seed: u64,
    group_index: usize,
    group: &ReplayConcurrentGroup,
    observation_id: &str,
) -> (ReplayStateObservation, TransactionReplayPersistenceProbe) {
    assert_eq!(
        group.actions.len(),
        2,
        "placement replay requires two actors"
    );
    let mut items = Vec::with_capacity(2);
    for action in &group.actions {
        let ReplayConcurrentAction::PlaceBlock { item, .. } = action else {
            panic!("placement fixture contains non-placement action");
        };
        items.push(item.clone());
    }

    let addr = server.addr;
    let left_profile = format!("R{}G{group_index}A0", seed % 10_000);
    let right_profile = format!("R{}G{group_index}A1", seed % 10_000);
    let (mut left, left_sync) = connect_to_play(addr, &left_profile).await;
    let (mut right, _) = connect_to_play(addr, &right_profile).await;
    drain_until_chunk(&mut left, (0, 0)).await;
    drain_until_chunk(&mut right, (0, 0)).await;
    for client in [&mut left, &mut right] {
        client
            .write_packet(&ServerboundChatCommand {
                command: "gamemode creative".to_string(),
            })
            .await
            .expect("set replay placement actor creative");
    }

    let target_coords = find_placeable_target(server, 3, 0, left_sync.y.floor() as i32).await;
    let target = mc_world::BlockPos {
        x: target_coords.0,
        y: target_coords.1,
        z: target_coords.2,
    };
    let support = pack_block_pos(target.x, target.y - 1, target.z);
    let target_pos = pack_block_pos(target.x, target.y, target.z);
    let air = server
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .expect("air block")
        .default;
    let contender_states = items
        .iter()
        .map(|item| {
            server
                .blocks
                .block(&mc_data::Identifier::parse(item).expect("checked replay block id"))
                .unwrap_or_else(|| panic!("replay item has no block state: {item}"))
                .default
        })
        .collect::<Vec<_>>();

    let mut authoritative_blocks = 0_i64;
    let mut consumed_items = 0_i64;
    let mut conservation_failures = 0_i64;
    let mut final_authoritative_state = None;
    for round in 0..group.repetitions {
        server
            .world
            .lock()
            .await
            .set_block_at(target, air)
            .expect("reset replay placement target");
        left.write_packet(&ServerboundChatCommand {
            command: format!("debug give {} 0 0", items[0]),
        })
        .await
        .expect("clear left replay slot");
        right
            .write_packet(&ServerboundChatCommand {
                command: format!("debug give {} 0 0", items[1]),
            })
            .await
            .expect("clear right replay slot");
        let ((), ()) = tokio::join!(
            wait_for_inventory_empty(&mut left),
            wait_for_inventory_empty(&mut right),
        );
        left.write_packet(&ServerboundChatCommand {
            command: format!("debug give {} 1 0", items[0]),
        })
        .await
        .expect("seed left replay slot");
        right
            .write_packet(&ServerboundChatCommand {
                command: format!("debug give {} 1 0", items[1]),
            })
            .await
            .expect("seed right replay slot");
        let ((), ()) = tokio::join!(
            wait_for_inventory_stack(&mut left),
            wait_for_inventory_stack(&mut right),
        );

        let left_sequence = 800 + i32::from(round) * 2;
        let right_sequence = left_sequence + 1;
        let left_action = ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: support,
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: left_sequence,
        };
        let right_action = ServerboundUseItemOn {
            sequence: right_sequence,
            ..left_action
        };
        let (left_sent, right_sent) = tokio::join!(
            left.write_packet(&left_action),
            right.write_packet(&right_action),
        );
        left_sent.expect("send left replay placement");
        right_sent.expect("send right replay placement");
        let (left_consumed, right_consumed) = tokio::join!(
            wait_for_placement_consumption(&mut left, left_sequence, target_pos),
            wait_for_placement_consumption(&mut right, right_sequence, target_pos),
        );
        let round_consumed = i64::from(left_consumed) + i64::from(right_consumed);
        consumed_items += round_consumed;

        let final_state = server
            .world
            .lock()
            .await
            .get_block(target)
            .expect("read replay placement target");
        final_authoritative_state = final_state;
        let committed = contender_states.contains(&final_state.unwrap_or(air));
        authoritative_blocks += i64::from(committed);
        let winner_matches = (final_state == Some(contender_states[0]) && left_consumed)
            || (final_state == Some(contender_states[1]) && right_consumed);
        if round_consumed != 1 || !committed || !winner_matches {
            conservation_failures += 1;
        }
    }

    drop(left);
    drop(right);
    let observation = ReplayStateObservation {
        id: observation_id.to_string(),
        values: vec![
            ReplayStateValue {
                key: "authoritative_blocks".into(),
                value: authoritative_blocks,
            },
            ReplayStateValue {
                key: "conservation_failures".into(),
                value: conservation_failures,
            },
            ReplayStateValue {
                key: "consumed_items".into(),
                value: consumed_items,
            },
            ReplayStateValue {
                key: "rounds".into(),
                value: i64::from(group.repetitions),
            },
        ],
    };
    let persistence_probe = TransactionReplayPersistenceProbe::Placement {
        observation_id: observation_id.to_string(),
        position: target,
        state: final_authoritative_state.expect("placement replay committed final state"),
    };
    (observation, persistence_probe)
}

async fn replay_shared_chest_pickup(
    server: &LoadServer,
    seed: u64,
    group_index: usize,
    group: &ReplayConcurrentGroup,
    observation_id: &str,
) -> (ReplayStateObservation, TransactionReplayPersistenceProbe) {
    assert_eq!(group.repetitions, 1, "shared chest replay runs once");
    assert_eq!(
        group.actions.len(),
        2,
        "shared chest replay requires two actors"
    );
    let ReplayConcurrentFixture::SharedChest {
        item,
        initial_count,
    } = &group.fixture
    else {
        panic!("shared chest executor received another fixture");
    };
    let initial_count = i32::from(*initial_count);
    let mut slots = Vec::with_capacity(2);
    for action in &group.actions {
        let ReplayConcurrentAction::ChestPickup { slot, .. } = action else {
            panic!("shared chest fixture contains another action");
        };
        slots.push(*slot);
    }
    let slot = usize::from(slots[0]);
    let baseline_sessions = server.runtime_telemetry.snapshot().active_sessions;

    let addr = server.addr;
    let left_profile = format!("R{}G{group_index}A0", seed % 10_000);
    let right_profile = format!("R{}G{group_index}A1", seed % 10_000);
    let (mut left, left_sync) = connect_to_play(addr, &left_profile).await;
    let (mut right, _) = connect_to_play(addr, &right_profile).await;
    drain_until_chunk(&mut left, (0, 0)).await;
    drain_until_chunk(&mut right, (0, 0)).await;
    left.write_packet(&ServerboundChatCommand {
        command: "gamemode creative".to_string(),
    })
    .await
    .expect("set replay chest placer creative");
    left.write_packet(&ServerboundChatCommand {
        command: "debug give minecraft:chest 1 0".to_string(),
    })
    .await
    .expect("give replay chest");
    wait_for_inventory_stack(&mut left).await;

    let target_coords = find_placeable_target(server, 2, 0, left_sync.y.floor() as i32).await;
    let chest_pos = mc_world::BlockPos {
        x: target_coords.0,
        y: target_coords.1,
        z: target_coords.2,
    };
    let chest_wire_pos = pack_block_pos(chest_pos.x, chest_pos.y, chest_pos.z);
    let placement_sequence = 900;
    left.write_packet(&ServerboundUseItemOn {
        hand: InteractionHand::MainHand,
        position: pack_block_pos(chest_pos.x, chest_pos.y - 1, chest_pos.z),
        direction: Direction::Up,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside: false,
        world_border_hit: false,
        sequence: placement_sequence,
    })
    .await
    .expect("place replay chest");
    let _ = wait_for_placement_consumption(&mut left, placement_sequence, chest_wire_pos).await;
    let chest_state = server
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:chest").unwrap())
        .expect("chest block")
        .default;
    let placed_state = server
        .world
        .lock()
        .await
        .get_block(chest_pos)
        .expect("read placed replay chest");
    assert_eq!(
        placed_state,
        Some(chest_state),
        "replay chest fixture must place an authoritative chest block"
    );

    let item_id = server
        .items
        .id_of(&mc_data::Identifier::parse(item).expect("checked replay item id"))
        .unwrap_or_else(|| panic!("replay chest item is unavailable: {item}"));
    {
        let mut world = server.world.lock().await;
        let mut chest = mc_world::ChestBlockEntity::default();
        chest.slots[slot] = mc_world::FurnaceSlot {
            item_id,
            count: initial_count,
            damage: None,
            enchantments: Vec::new(),
        };
        world
            .set_chest_block_entity(chest_pos, chest)
            .expect("seed replay chest stack");
    }

    let left_open = ServerboundUseItemOn {
        hand: InteractionHand::MainHand,
        position: chest_wire_pos,
        direction: Direction::Up,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside: false,
        world_border_hit: false,
        sequence: 901,
    };
    let right_open = ServerboundUseItemOn {
        sequence: 902,
        ..left_open
    };
    let (left_sent, right_sent) = tokio::join!(
        left.write_packet(&left_open),
        right.write_packet(&right_open),
    );
    left_sent.expect("left opens replay chest");
    right_sent.expect("right opens replay chest");
    let (left_screen, right_screen) = tokio::join!(
        wait_for_load_open_screen(&mut left, 2),
        wait_for_load_open_screen(&mut right, 2),
    );
    let (left_initial, right_initial) = tokio::join!(
        wait_for_load_container_content(&mut left, left_screen.container_id, |packet| {
            packet.items[slot].item_id == item_id && packet.items[slot].count == initial_count
        }),
        wait_for_load_container_content(&mut right, right_screen.container_id, |packet| {
            packet.items[slot].item_id == item_id && packet.items[slot].count == initial_count
        }),
    );
    let cursor_count = (initial_count + 1) / 2;
    let container_count = initial_count / 2;
    let click = |container_id, state_id| ServerboundContainerClick {
        container_id,
        state_id,
        slot_num: i16::from(slots[0]),
        button_num: 1,
        container_input: ContainerInput::Pickup,
        changed_slots: vec![(
            i16::from(slots[0]),
            HashedStack::Actual {
                item_id,
                count: container_count,
                components: HashedStackComponentHashes::empty(),
            },
        )],
        carried_item: HashedStack::Actual {
            item_id,
            count: cursor_count,
            components: HashedStackComponentHashes::empty(),
        },
    };
    let left_click = click(left_screen.container_id, left_initial.state_id);
    let right_click = click(right_screen.container_id, right_initial.state_id);
    let (left_sent, right_sent) = tokio::join!(
        left.write_packet(&left_click),
        right.write_packet(&right_click),
    );
    left_sent.expect("send left replay chest click");
    right_sent.expect("send right replay chest click");
    let (left_result, right_result) = tokio::join!(
        wait_for_load_container_content(&mut left, left_screen.container_id, |packet| {
            packet.state_id > left_initial.state_id
                && packet.items[slot].item_id == item_id
                && packet.items[slot].count == container_count
        }),
        wait_for_load_container_content(&mut right, right_screen.container_id, |packet| {
            packet.state_id > right_initial.state_id
                && packet.items[slot].item_id == item_id
                && packet.items[slot].count == container_count
        }),
    );
    let left_cursor = left_result.carried_item.count;
    let right_cursor = right_result.carried_item.count;
    let stored = server
        .world
        .lock()
        .await
        .chest_block_entity(chest_pos)
        .expect("read replay chest")
        .expect("replay chest exists")
        .slots[slot]
        .count;

    drop(left);
    drop(right);
    wait_for_active_sessions_at_most(server, baseline_sessions).await;
    let (mut left_rejoined, _) = connect_to_play(addr, &left_profile).await;
    let (mut right_rejoined, _) = connect_to_play(addr, &right_profile).await;
    let (left_inventory, right_inventory) = tokio::join!(
        wait_for_load_container_content(&mut left_rejoined, 0, |_| true),
        wait_for_load_container_content(&mut right_rejoined, 0, |_| true),
    );
    let reconnected_player_items = [&left_inventory, &right_inventory]
        .into_iter()
        .flat_map(|inventory| inventory.items.iter().chain([&inventory.carried_item]))
        .filter(|stack| stack.item_id == item_id)
        .map(|stack| stack.count)
        .sum::<i32>();
    drop(left_rejoined);
    drop(right_rejoined);
    let observation = ReplayStateObservation {
        id: observation_id.to_string(),
        values: vec![
            ReplayStateValue {
                key: "container_items".into(),
                value: i64::from(stored),
            },
            ReplayStateValue {
                key: "cursor_items".into(),
                value: i64::from(left_cursor) + i64::from(right_cursor),
            },
            ReplayStateValue {
                key: "reconnected_player_items".into(),
                value: i64::from(reconnected_player_items),
            },
            ReplayStateValue {
                key: "total_items".into(),
                value: i64::from(stored) + i64::from(left_cursor) + i64::from(right_cursor),
            },
            ReplayStateValue {
                key: "winning_cursors".into(),
                value: i64::from(left_cursor > 0) + i64::from(right_cursor > 0),
            },
        ],
    };
    let persistence_probe = TransactionReplayPersistenceProbe::SharedChest {
        observation_id: observation_id.to_string(),
        position: chest_pos,
        item_id,
        profiles: [left_profile, right_profile],
    };
    (observation, persistence_probe)
}

fn replay_state_mismatches(
    manifest: &ReplayScenarioManifest,
    actual: &[ReplayStateObservation],
) -> Result<(), Vec<String>> {
    let mut mismatched_groups = Vec::new();
    for expected in &manifest.state_expectations {
        let Some(actual) = actual
            .iter()
            .find(|observation| observation.id == expected.id)
        else {
            mismatched_groups.push(expected.after_group.clone());
            continue;
        };
        let expected_values = expected
            .values
            .iter()
            .map(|value| (value.key.as_str(), value.value))
            .collect::<std::collections::BTreeMap<_, _>>();
        let actual_values = actual
            .values
            .iter()
            .map(|value| (value.key.as_str(), value.value))
            .collect::<std::collections::BTreeMap<_, _>>();
        if actual_values != expected_values {
            mismatched_groups.push(expected.after_group.clone());
        }
    }
    if actual.len() != manifest.state_expectations.len() && mismatched_groups.is_empty() {
        mismatched_groups.extend(
            manifest
                .concurrent_groups
                .iter()
                .map(|group| group.id.clone()),
        );
    }
    mismatched_groups.sort();
    mismatched_groups.dedup();
    if mismatched_groups.is_empty() {
        Ok(())
    } else {
        Err(mismatched_groups)
    }
}

fn persist_minimal_replay_failures<'a>(
    manifest: &ReplayScenarioManifest,
    groups: impl IntoIterator<Item = &'a str>,
) -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/core-replay-failures");
    std::fs::create_dir_all(&root).expect("create core replay failure directory");
    groups
        .into_iter()
        .map(|group_id| {
            let failure = manifest
                .minimal_concurrent_failure(group_id)
                .expect("shrink concurrent replay failure");
            let path = root.join(format!("{}-{group_id}.json", manifest.id));
            std::fs::write(
                &path,
                failure
                    .to_pretty_json()
                    .expect("serialize concurrent replay failure"),
            )
            .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
            path
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires local data/vanilla sidecars; degraded when absent"]
async fn bounded_multiplayer_survival_replay_covers_sequential_contention_and_slow_reader() {
    let server = start_load_server().await;
    let addr = server.addr;
    let started = Instant::now();

    let mut paused_reader = PausedReaderClient::connect(addr, "M96PausedReader").await;
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
    let cancellation_before_disconnect = server.chunk_pipeline_metrics.cancellation_snapshot();
    reconnecting
        .write_packet(&ServerboundMovePlayerPos {
            x: reconnect_sync.x + 48.0,
            y: reconnect_sync.y,
            z: reconnect_sync.z,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("start chunk work before reconnect");
    wait_for_center_chunk(&mut reconnecting, (3, 0)).await;
    drop(reconnecting);
    wait_for_active_sessions(&server, M96_REPLAY_CLIENTS).await;
    let cancellation_after_disconnect =
        wait_for_chunk_cancellation(&server, cancellation_before_disconnect).await;
    assert!(
        cancellation_after_disconnect.cancelled_requests
            > cancellation_before_disconnect.cancelled_requests,
        "disconnect after replan must cancel queued/in-flight chunk requests: before={cancellation_before_disconnect:?} after={cancellation_after_disconnect:?}"
    );
    assert!(
        cancellation_after_disconnect
            .cancelled_requests
            .saturating_sub(cancellation_before_disconnect.cancelled_requests)
            <= M96_CANCELLED_REQUEST_BUDGET,
        "disconnect cancellation must stay bounded: before={cancellation_before_disconnect:?} after={cancellation_after_disconnect:?} budget={M96_CANCELLED_REQUEST_BUDGET}"
    );
    let (mut rejoined, _) = connect_to_play(addr, "M96Soak3").await;
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

    paused_reader
        .trigger_outbound_pressure(DETERMINISTIC_OUTBOUND_PRESSURE_BURST)
        .await;

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
        "M96 bounded_replay clients={} sequential_same_block_updates={} spawns={} elapsed_ms={} outbound_before={:?} outbound_after={:?} chunk_before_disconnect={:?} chunk_cancellation_before={:?} chunk_cancellation_after={:?} chunk_pipeline={:?} session_lock={:?} world_lock={:?}",
        M96_REPLAY_CLIENTS + 1,
        observer_updates,
        spawns,
        elapsed.as_millis(),
        pressure_before,
        pressure_after,
        chunk_before_disconnect,
        cancellation_before_disconnect,
        cancellation_after_disconnect,
        chunk_snapshot,
        pressure.session_registry,
        pressure.world_storage,
    );

    paused_reader.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires local data/vanilla sidecars; degraded when absent"]
async fn concurrent_same_target_placements_consume_exactly_one_stack() {
    const ROUNDS: usize = 8;

    let server = start_load_server().await;
    let addr = server.addr;
    let (mut dirt_player, dirt_sync) = connect_to_play(addr, "Prompt02Dirt").await;
    let (mut stone_player, _) = connect_to_play(addr, "Prompt02Stone").await;
    drain_until_chunk(&mut dirt_player, (0, 0)).await;
    drain_until_chunk(&mut stone_player, (0, 0)).await;
    for client in [&mut dirt_player, &mut stone_player] {
        client
            .write_packet(&ServerboundChatCommand {
                command: "gamemode creative".to_string(),
            })
            .await
            .expect("set placement contender creative");
    }

    let target_coords = find_placeable_target(&server, 3, 0, dirt_sync.y.floor() as i32).await;
    let target = mc_world::BlockPos {
        x: target_coords.0,
        y: target_coords.1,
        z: target_coords.2,
    };
    let support = pack_block_pos(target.x, target.y - 1, target.z);
    let target_pos = pack_block_pos(target.x, target.y, target.z);
    let air = server
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .expect("air block")
        .default;
    let dirt = server
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt block")
        .default;
    let stone = server
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .expect("stone block")
        .default;

    for round in 0..ROUNDS {
        server
            .world
            .lock()
            .await
            .set_block_at(target, air)
            .expect("reset concurrent placement target");
        dirt_player
            .write_packet(&ServerboundChatCommand {
                command: "debug give minecraft:dirt 0 0".to_string(),
            })
            .await
            .expect("clear dirt contender slot");
        stone_player
            .write_packet(&ServerboundChatCommand {
                command: "debug give minecraft:stone 0 0".to_string(),
            })
            .await
            .expect("clear stone contender slot");
        let ((), ()) = tokio::join!(
            wait_for_inventory_empty(&mut dirt_player),
            wait_for_inventory_empty(&mut stone_player),
        );
        dirt_player
            .write_packet(&ServerboundChatCommand {
                command: "debug give minecraft:dirt 1 0".to_string(),
            })
            .await
            .expect("give dirt contender stack");
        stone_player
            .write_packet(&ServerboundChatCommand {
                command: "debug give minecraft:stone 1 0".to_string(),
            })
            .await
            .expect("give stone contender stack");
        let ((), ()) = tokio::join!(
            wait_for_inventory_stack(&mut dirt_player),
            wait_for_inventory_stack(&mut stone_player),
        );

        let dirt_sequence = 400 + i32::try_from(round * 2).expect("round sequence");
        let stone_sequence = dirt_sequence + 1;
        let dirt_action = ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: support,
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: dirt_sequence,
        };
        let stone_action = ServerboundUseItemOn {
            sequence: stone_sequence,
            ..dirt_action
        };
        let (dirt_sent, stone_sent) = tokio::join!(
            dirt_player.write_packet(&dirt_action),
            stone_player.write_packet(&stone_action),
        );
        dirt_sent.expect("send dirt contender placement");
        stone_sent.expect("send stone contender placement");

        let (dirt_consumed, stone_consumed) = tokio::join!(
            wait_for_placement_consumption(&mut dirt_player, dirt_sequence, target_pos),
            wait_for_placement_consumption(&mut stone_player, stone_sequence, target_pos),
        );
        assert_eq!(
            usize::from(dirt_consumed) + usize::from(stone_consumed),
            1,
            "round {round} must consume exactly one contender stack"
        );
        let final_state = server
            .world
            .lock()
            .await
            .get_block(target)
            .expect("read concurrent placement target");
        assert!(
            final_state == Some(dirt) || final_state == Some(stone),
            "round {round} must commit exactly one contender block: {final_state:?}"
        );
        assert_eq!(
            final_state == Some(dirt),
            dirt_consumed,
            "round {round} final block must belong to the one consumed stack"
        );
    }

    drop(dirt_player);
    drop(stone_player);
    server.shutdown.request();
    let serve_result = tokio::time::timeout(Duration::from_secs(5), server.serve_task)
        .await
        .expect("concurrent placement server shutdown")
        .expect("concurrent placement server joins");
    serve_result.expect("concurrent placement server exits cleanly");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires local data/vanilla sidecars; degraded when absent"]
async fn concurrent_shared_chest_same_state_commits_one_cursor_transaction() {
    let server = start_load_server().await;
    let addr = server.addr;
    let (mut actor, actor_sync) = connect_to_play(addr, "Prompt02ChestA").await;
    let (mut observer, _) = connect_to_play(addr, "Prompt02ChestB").await;
    drain_until_chunk(&mut actor, (0, 0)).await;
    drain_until_chunk(&mut observer, (0, 0)).await;
    actor
        .write_packet(&ServerboundChatCommand {
            command: "gamemode creative".to_string(),
        })
        .await
        .expect("set chest placer creative");
    actor
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:chest 1 0".to_string(),
        })
        .await
        .expect("give shared chest");
    wait_for_inventory_stack(&mut actor).await;

    let target_coords = find_placeable_target(&server, 2, 0, actor_sync.y.floor() as i32).await;
    let chest_pos = mc_world::BlockPos {
        x: target_coords.0,
        y: target_coords.1,
        z: target_coords.2,
    };
    let chest_wire_pos = pack_block_pos(chest_pos.x, chest_pos.y, chest_pos.z);
    let placement_sequence = 500;
    actor
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(chest_pos.x, chest_pos.y - 1, chest_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: placement_sequence,
        })
        .await
        .expect("place shared chest");
    let _ = wait_for_placement_consumption(&mut actor, placement_sequence, chest_wire_pos).await;

    let dirt_id = server
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item id");
    {
        let mut world = server.world.lock().await;
        let mut chest = mc_world::ChestBlockEntity::default();
        chest.slots[0] = mc_world::FurnaceSlot {
            item_id: dirt_id,
            count: 2,
            damage: None,
            enchantments: Vec::new(),
        };
        world
            .set_chest_block_entity(chest_pos, chest)
            .expect("seed shared chest stack");
    }

    let actor_open = ServerboundUseItemOn {
        hand: InteractionHand::MainHand,
        position: chest_wire_pos,
        direction: Direction::Up,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside: false,
        world_border_hit: false,
        sequence: 501,
    };
    let observer_open = ServerboundUseItemOn {
        sequence: 502,
        ..actor_open
    };
    let (actor_sent, observer_sent) = tokio::join!(
        actor.write_packet(&actor_open),
        observer.write_packet(&observer_open),
    );
    actor_sent.expect("actor opens shared chest");
    observer_sent.expect("observer opens shared chest");
    let (actor_opened, observer_opened) = tokio::join!(
        wait_for_load_open_screen(&mut actor, 2),
        wait_for_load_open_screen(&mut observer, 2),
    );
    let (actor_initial, observer_initial) = tokio::join!(
        wait_for_load_container_content(&mut actor, actor_opened.container_id, |packet| {
            packet.items[0].item_id == dirt_id && packet.items[0].count == 2
        }),
        wait_for_load_container_content(
            &mut observer,
            observer_opened.container_id,
            |packet| packet.items[0].item_id == dirt_id && packet.items[0].count == 2,
        ),
    );
    assert_eq!(actor_initial.state_id, observer_initial.state_id);

    let click = |container_id, state_id| ServerboundContainerClick {
        container_id,
        state_id,
        slot_num: 0,
        button_num: 1,
        container_input: ContainerInput::Pickup,
        changed_slots: vec![(
            0,
            HashedStack::Actual {
                item_id: dirt_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        )],
        carried_item: HashedStack::Actual {
            item_id: dirt_id,
            count: 1,
            components: HashedStackComponentHashes::empty(),
        },
    };
    let actor_click = click(actor_opened.container_id, actor_initial.state_id);
    let observer_click = click(observer_opened.container_id, observer_initial.state_id);
    let (actor_sent, observer_sent) = tokio::join!(
        actor.write_packet(&actor_click),
        observer.write_packet(&observer_click),
    );
    actor_sent.expect("send actor shared chest click");
    observer_sent.expect("send observer shared chest click");
    let (actor_result, observer_result) = tokio::join!(
        wait_for_load_container_content(&mut actor, actor_opened.container_id, |packet| {
            packet.state_id > actor_initial.state_id
                && packet.items[0].item_id == dirt_id
                && packet.items[0].count == 1
        }),
        wait_for_load_container_content(&mut observer, observer_opened.container_id, |packet| {
            packet.state_id > observer_initial.state_id
                && packet.items[0].item_id == dirt_id
                && packet.items[0].count == 1
        },),
    );
    let actor_cursor = actor_result.carried_item.count;
    let observer_cursor = observer_result.carried_item.count;
    assert_eq!(
        usize::from(actor_cursor == 1) + usize::from(observer_cursor == 1),
        1,
        "one shared state id must commit exactly one right-click cursor transaction"
    );
    assert!(
        (actor_cursor == 1 && observer_cursor == 0) || (actor_cursor == 0 && observer_cursor == 1),
        "winner cursor counts must be one and zero: actor={actor_cursor} observer={observer_cursor}"
    );
    let chest_count = server
        .world
        .lock()
        .await
        .chest_block_entity(chest_pos)
        .expect("read shared chest")
        .expect("shared chest exists")
        .slots[0]
        .count;
    assert_eq!(chest_count, 1);
    assert_eq!(
        chest_count + actor_cursor + observer_cursor,
        2,
        "shared chest transaction must conserve the seeded stack exactly"
    );
    let simulation = server.runtime_telemetry.snapshot();
    eprintln!(
        "Prompt 03 shared chest container_commits={} world_busy={} world_unavailable={} world_mutation={}",
        simulation.simulation_container_commits_processed,
        simulation.simulation_commands_rejected_world_busy,
        simulation.simulation_commands_rejected_world_unavailable,
        simulation.simulation_commands_rejected_world_mutation,
    );
    assert_eq!(simulation.simulation_container_commits_processed, 2);
    assert_eq!(simulation.simulation_commands_rejected_world_busy, 0);
    assert_eq!(simulation.simulation_commands_rejected_world_unavailable, 0);
    assert_eq!(simulation.simulation_commands_rejected_world_mutation, 0);
    assert_eq!(simulation.simulation_commands_rejected_stale_session, 0);

    drop(actor);
    drop(observer);
    server.shutdown.request();
    let serve_result = tokio::time::timeout(Duration::from_secs(5), server.serve_task)
        .await
        .expect("shared chest server shutdown")
        .expect("shared chest server joins");
    serve_result.expect("shared chest server exits cleanly");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires local data/vanilla sidecars; degraded when absent"]
async fn vd8_multi_client_stop_drains_and_flushes_disk_world_under_stream_load() {
    let server = start_load_server_with_options(LoadServerOptions {
        view_distance: O2_STOP_VIEW_DISTANCE,
        disk_backed: true,
        existing_world_path: None,
        spawn_passive_entities: true,
        runtime_control: true,
        max_players: 8,
        fixed_runtime_view_distance: false,
        chunk_result_queue_size: 8,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "requires local data/vanilla sidecars; O1/O2 concurrent VD8 gate"]
async fn vd8_twenty_same_spawn_clients_drain_full_window_and_stop_without_duplicate_pressure() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            "mc_net::lock_metrics=warn,mc_net::server=warn",
        ))
        .with_test_writer()
        .try_init();
    let pressure_before = mc_net::lock_pressure_snapshot();
    let server = start_load_server_with_options(LoadServerOptions {
        view_distance: O2_STOP_VIEW_DISTANCE,
        disk_backed: true,
        existing_world_path: None,
        spawn_passive_entities: true,
        runtime_control: true,
        max_players: O2_VD8_CONCURRENT_CLIENTS,
        fixed_runtime_view_distance: true,
        chunk_result_queue_size: O2_VD8_CHUNK_RESULT_QUEUE_SIZE,
    })
    .await;
    let addr = server.addr;
    let world_before = server.world.lock().await.stats();
    let outbound_before = server.outbound_pressure_snapshot();
    let started = Instant::now();

    let mut client_tasks = Vec::new();
    for idx in 0..O2_VD8_CONCURRENT_CLIENTS {
        client_tasks.push(tokio::spawn(async move {
            let name = format!("O2Vd8Load{idx}");
            let (mut client, _) = connect_to_play(addr, &name).await;
            let play_ready_ms = elapsed_ms(started);
            let chunks =
                try_drain_vd8_chunks_with_timing(&mut client, Duration::from_secs(90), &name).await;
            (idx, client, play_ready_ms, chunks)
        }));
    }

    let mut clients = Vec::new();
    let mut per_client_chunks = Vec::new();
    let mut per_client_latency = Vec::new();
    let mut streamed_chunks = std::collections::BTreeSet::new();
    let mut client_failures = Vec::new();
    for task in client_tasks {
        let (idx, client, play_ready_ms, chunks) = task.await.expect("O2 VD8 client task joins");
        match chunks {
            Ok(drain) => {
                assert_eq!(
                    drain.chunks.len(),
                    O2_VD8_WINDOW_CHUNKS,
                    "client {idx} should drain the full VD8 window"
                );
                assert!(
                    drain.latency.first_ms <= drain.latency.ring1_ms
                        && drain.latency.ring1_ms <= drain.latency.ring2_ms
                        && drain.latency.ring2_ms <= drain.latency.full_ms,
                    "client {idx} chunk milestones must be monotonic: {:?}",
                    drain.latency
                );
                streamed_chunks.extend(drain.chunks.iter().copied());
                per_client_chunks.push((idx, drain.chunks.len()));
                per_client_latency.push((idx, play_ready_ms, drain.latency));
            }
            Err(err) => client_failures.push(format!("client {idx}: {err}")),
        }
        clients.push(client);
    }
    if !client_failures.is_empty() {
        let chunk_snapshot = server.chunk_pipeline_metrics.snapshot();
        let stop_reason_counts = server.chunk_pipeline_metrics.stop_reason_counts();
        let outbound_pressure = server.outbound_pressure_snapshot();
        let runtime_snapshot = server
            .runtime_control
            .as_ref()
            .map(mc_net::RuntimeControlHandle::snapshot);
        for client in clients {
            drop(client);
        }
        server.shutdown.request();
        let _ = tokio::time::timeout(Duration::from_secs(5), server.serve_task).await;
        panic!(
            "O2 VD8 clients failed before full window: failures={client_failures:?} partial_chunks={per_client_chunks:?} shared_chunks={} chunk_pipeline={chunk_snapshot:?} stop_reason_counts={stop_reason_counts:?} outbound_pressure={outbound_pressure:?} runtime_snapshot={runtime_snapshot:?}",
            streamed_chunks.len()
        );
    }
    assert_eq!(
        clients.len(),
        O2_VD8_CONCURRENT_CLIENTS,
        "all O2 VD8 clients should reach Play"
    );
    assert_eq!(
        streamed_chunks.len(),
        O2_VD8_WINDOW_CHUNKS,
        "same-spawn VD8 clients should share one chunk window"
    );
    assert_eq!(
        per_client_latency.len(),
        O2_VD8_CONCURRENT_CLIENTS,
        "all O2 VD8 clients should emit chunk latency milestones"
    );

    let loaded_snapshot = server.chunk_pipeline_metrics.snapshot();
    let world_loaded = server.world.lock().await.stats();
    let telemetry_loaded = server.runtime_telemetry.snapshot();
    assert_eq!(
        telemetry_loaded.active_sessions, O2_VD8_CONCURRENT_CLIENTS,
        "runtime telemetry should observe every connected O2 VD8 client"
    );
    assert!(
        telemetry_loaded.ticketed_chunks >= O2_VD8_WINDOW_CHUNKS,
        "runtime telemetry should expose the shared VD8 ticket set: {telemetry_loaded:?}"
    );
    assert!(
        loaded_snapshot.max_result_queue_depth > 0
            && loaded_snapshot.max_result_queue_depth <= O2_VD8_CHUNK_RESULT_QUEUE_SIZE,
        "chunk result queue depth must be observed within its configured bound: {loaded_snapshot:?}"
    );
    assert!(
        loaded_snapshot.max_cpu_active <= server.chunk_worker_threads,
        "chunk CPU permits exceeded during 20-client VD8 load: {:?}",
        loaded_snapshot
    );

    let save_report = server.save_handle.save_all().await;
    assert!(
        save_report.is_ok(),
        "O2 VD8 explicit save should succeed: {save_report:?}"
    );
    assert!(
        save_report.chunks_flushed > 0,
        "O2 VD8 explicit save should flush generated chunks: {save_report:?}"
    );
    let world_after_explicit_save = server.world.lock().await.stats();
    assert!(
        loaded_snapshot.max_io_active <= server.chunk_io_threads,
        "chunk IO permits exceeded during 20-client VD8 load: {:?}",
        loaded_snapshot
    );

    let mut stopper = clients.remove(0);
    let mut drain_tasks = Vec::new();
    for client in clients {
        let shutdown = server.shutdown.clone();
        drain_tasks.push(tokio::spawn(async move {
            drain_client_until_shutdown(client, shutdown).await;
        }));
    }
    stopper
        .write_packet(&ServerboundChatCommand {
            command: "stop".to_string(),
        })
        .await
        .expect("send O2 VD8 stop command");
    let stopper_shutdown = server.shutdown.clone();
    let stopper_drain = tokio::spawn(async move {
        drain_client_until_shutdown(stopper, stopper_shutdown).await;
    });
    wait_for_shutdown_requested(&server.shutdown).await;

    let runtime_control = server
        .runtime_control
        .as_ref()
        .expect("O2 VD8 gate enables runtime control");
    let runtime_snapshot = runtime_control.snapshot();
    assert!(
        runtime_snapshot.draining,
        "player /stop should request runtime-control drain"
    );
    let outbound_pressure = server.outbound_pressure_snapshot();
    for task in drain_tasks {
        let _ = task.await;
    }
    let _ = stopper_drain.await;

    let serve_result = tokio::time::timeout(Duration::from_secs(15), server.serve_task)
        .await
        .expect("server should exit after O2 VD8 player /stop")
        .expect("server task should join");
    serve_result.expect("server serve should exit cleanly");

    let telemetry_final = server.runtime_telemetry.snapshot();
    let tick_percentiles = telemetry_final
        .tick_percentiles
        .expect("O2 VD8 workload should publish runtime tick percentiles");
    assert_eq!(
        tick_percentiles.entity_save.max_us, 0,
        "periodic persistence must run outside the simulation tick: {tick_percentiles:?}"
    );
    assert!(
        tick_percentiles.tick.p99_us <= O2_VD8_TICK_BUDGET_US,
        "O2 VD8 p99 tick latency exceeded the 50 ms target: {tick_percentiles:?}"
    );
    assert!(
        tick_percentiles.tick.max_us <= O2_VD8_TICK_BUDGET_US,
        "O2 VD8 workload had a rare tick stall above 50 ms: {tick_percentiles:?}"
    );
    assert!(
        tick_percentiles.entity_physics.max_us <= O2_VD8_ENTITY_PHYSICS_MAX_BUDGET_US,
        "O2 VD8 entity physics had a rare stall: {tick_percentiles:?}"
    );
    assert_eq!(
        tick_percentiles.observer_skipped_windows, 0,
        "O2 VD8 metrics observer skipped a percentile window: {tick_percentiles:?}"
    );
    assert!(
        tick_percentiles.tick.samples >= 100,
        "O2 VD8 workload should cover at least one full metrics cadence: {tick_percentiles:?}"
    );
    assert!(
        telemetry_final.memory_used_mb > 0 && telemetry_final.memory_limit_mb > 0,
        "O2 VD8 workload should report RSS and its effective limit: {telemetry_final:?}"
    );
    assert_eq!(
        telemetry_final.active_sessions, 0,
        "all O2 VD8 sessions should be removed after shutdown: {telemetry_final:?}"
    );
    let world_after_shutdown = server.world.lock().await.stats();
    assert_eq!(
        world_after_shutdown.dirty_chunks, 0,
        "shutdown save should leave no dirty chunks: {world_after_shutdown:?}"
    );

    let drained_snapshot = server.chunk_pipeline_metrics.snapshot();
    assert_eq!(
        drained_snapshot.active_cpu, 0,
        "chunk CPU work should be drained after O2 VD8 stop: {:?}",
        drained_snapshot
    );
    assert_eq!(
        drained_snapshot.active_io, 0,
        "chunk IO work should be drained after O2 VD8 stop: {:?}",
        drained_snapshot
    );
    assert!(
        drained_snapshot.max_cpu_active <= server.chunk_worker_threads,
        "chunk CPU permits exceeded during O2 VD8 load/stop: {:?}",
        drained_snapshot
    );
    assert!(
        drained_snapshot.max_io_active <= server.chunk_io_threads,
        "chunk IO permits exceeded during O2 VD8 load/stop: {:?}",
        drained_snapshot
    );

    let stop_reason_counts = server.chunk_pipeline_metrics.stop_reason_counts();
    let stop_reasons = stop_reason_counts.observed_reasons();
    assert!(
        !stop_reasons.is_empty(),
        "O2 VD8 gate should report chunk stop reasons"
    );
    assert!(
        stop_reason_counts.queue_empty <= O2_VD8_QUEUE_EMPTY_STOP_BUDGET,
        "unrelated play events repeatedly rescanned empty chunk queues: {stop_reason_counts:?}"
    );

    let pressure_after = mc_net::lock_pressure_snapshot();
    let world_storage_delta =
        lock_metric_delta(pressure_before.world_storage, pressure_after.world_storage);
    let session_registry_delta = lock_metric_delta(
        pressure_before.session_registry,
        pressure_after.session_registry,
    );
    let save_all_flush_delta = lock_metric_delta(
        pressure_before.save_all_flush,
        pressure_after.save_all_flush,
    );
    let chunk_prepare_delta =
        lock_metric_delta(pressure_before.chunk_prepare, pressure_after.chunk_prepare);
    let player_persistence_delta = lock_metric_delta(
        pressure_before.player_persistence,
        pressure_after.player_persistence,
    );
    assert_lock_metric_observed_within_budget(
        "world_storage",
        world_storage_delta,
        pressure_after.world_storage,
        O2_VD8_LOCK_MAX_HOLD_BUDGET_US,
    );
    assert_lock_metric_observed_within_budget(
        "session_registry",
        session_registry_delta,
        pressure_after.session_registry,
        O2_VD8_LOCK_MAX_HOLD_BUDGET_US,
    );
    assert_lock_metric_observed_within_budget(
        "save_all_flush",
        save_all_flush_delta,
        pressure_after.save_all_flush,
        O2_VD8_LOCK_MAX_HOLD_BUDGET_US,
    );
    assert_lock_metric_observed_within_budget(
        "chunk_prepare",
        chunk_prepare_delta,
        pressure_after.chunk_prepare,
        O2_VD8_LOCK_MAX_HOLD_BUDGET_US,
    );
    assert!(
        chunk_prepare_delta.hold_count <= O2_VD8_CHUNK_PREPARE_HOLD_COUNT_BUDGET,
        "same-spawn VD8 chunk preparation should stay near the shared window, not per-client duplication: delta={chunk_prepare_delta:?} budget={O2_VD8_CHUNK_PREPARE_HOLD_COUNT_BUDGET}"
    );

    let scenarios = [
        mc_net::AutoscaleSoakScenario::ChunkGenerationStorm,
        mc_net::AutoscaleSoakScenario::SaveDuringShutdown,
        mc_net::AutoscaleSoakScenario::DrainRestart,
    ];
    let autoscale_report =
        mc_net::AutoscaleSoakReport::from_snapshot(mc_net::AutoscaleSoakSnapshot {
            profile: mc_net::AutoscaleSoakProfile::Balanced,
            scenarios: &scenarios,
            chunk_policy: server.chunk_policy,
            chunk_resources: drained_snapshot,
            chunk_stop_reasons: &stop_reasons,
            outbound_pressure,
            save_all: Some(&save_report),
            memory_pressure_shed_chunks: 0,
            runtime_control: Some(&runtime_snapshot),
        });
    assert!(
        matches!(
            autoscale_report.worker_backpressure,
            mc_net::AutoscalePrimitiveStatus::Present
        ),
        "O2 VD8 worker metrics should stay within configured permits: {autoscale_report:?}"
    );
    assert!(
        matches!(
            autoscale_report.dynamic_autoscale,
            mc_net::AutoscalePrimitiveStatus::Degraded { .. }
        ) && autoscale_report
            .gaps
            .contains(&"dynamic runtime-control pressure/action decision was not observed"),
        "runtime-control drain snapshots must not be counted as autoscale pressure: runtime={runtime_snapshot:?} report={autoscale_report:?}"
    );

    let elapsed = started.elapsed();
    assert!(
        elapsed <= O2_VD8_JOIN_ELAPSED_BUDGET,
        "O2 VD8 20-client gate exceeded debug elapsed budget: elapsed={elapsed:?} budget={O2_VD8_JOIN_ELAPSED_BUDGET:?}"
    );

    per_client_chunks.sort_unstable_by_key(|(idx, _)| *idx);
    per_client_latency.sort_unstable_by_key(|(idx, _, _)| *idx);
    let first_chunk_latency = workload_latency_percentiles_ms(
        per_client_latency
            .iter()
            .map(|(_, _, latency)| latency.first_ms),
    );
    assert!(
        first_chunk_latency.p99_ms <= O2_VD8_FIRST_CHUNK_P99_BUDGET_MS,
        "O2 VD8 first-chunk p99 exceeded its debug budget: {first_chunk_latency:?}"
    );
    let ring1_latency = workload_latency_percentiles_ms(
        per_client_latency
            .iter()
            .map(|(_, _, latency)| latency.ring1_ms),
    );
    let ring2_latency = workload_latency_percentiles_ms(
        per_client_latency
            .iter()
            .map(|(_, _, latency)| latency.ring2_ms),
    );
    let full_window_latency = workload_latency_percentiles_ms(
        per_client_latency
            .iter()
            .map(|(_, _, latency)| latency.full_ms),
    );
    let (commit, dirty_worktree) = workload_git_provenance();
    let sidecar_version = workload_sidecar_version();
    let stop_reason_labels = stop_reasons
        .iter()
        .map(|reason| format!("{reason:?}"))
        .collect::<Vec<_>>();
    let report = serde_json::json!({
        "schema": "solaris.prompt01.workload.result.v1",
        "scenario_id": "prompt01-20-client-vd8-same-spawn",
        "status": "passed",
        "quality_label": "stabilization",
        "complete_metrics": true,
        "provenance": {
            "commit": commit,
            "dirty_worktree": dirty_worktree,
            "config_fingerprint": format!(
                "seed0-vd{}-players{}-io{}-cpu{}-q{}-disk",
                O2_STOP_VIEW_DISTANCE,
                O2_VD8_CONCURRENT_CLIENTS,
                server.chunk_io_threads,
                server.chunk_worker_threads,
                O2_VD8_CHUNK_RESULT_QUEUE_SIZE,
            ),
            "build_profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "target_os": std::env::consts::OS,
            "target_arch": std::env::consts::ARCH,
            "logical_cpus": std::thread::available_parallelism()
                .map(std::num::NonZeroUsize::get)
                .unwrap_or(1),
            "sidecar_version": sidecar_version,
        },
        "evidence_classes": {
            "unit": { "status": "not_run", "reason": "not part of this focused workload invocation" },
            "harness": { "status": "passed", "gate": "mc-test-harness/load_scenarios" },
            "oracle": { "status": "skipped", "reason": "protocol workload, no vanilla comparison" },
            "real_client": { "status": "skipped", "reason": "protocol workload" },
            "performance": { "status": "measured", "scope": "focused debug workload" },
            "soak": { "status": "skipped", "reason": "bounded workload, not a duration soak" },
        },
        "workload": {
            "clients": O2_VD8_CONCURRENT_CLIENTS,
            "view_distance": O2_STOP_VIEW_DISTANCE,
            "expected_chunks_per_client": O2_VD8_WINDOW_CHUNKS,
            "shared_unique_chunks": streamed_chunks.len(),
            "per_client_chunks": per_client_chunks,
            "elapsed_ms": elapsed_ms(started),
        },
        "tick_latency": {
            "total": runtime_latency_json(tick_percentiles.tick),
            "world_time": runtime_latency_json(tick_percentiles.world_time),
            "animal_breeding": runtime_latency_json(tick_percentiles.animal_breeding),
            "hostile_attacks": runtime_latency_json(tick_percentiles.hostile_attacks),
            "entity_goals": runtime_latency_json(tick_percentiles.entity_goals),
            "entity_physics": runtime_latency_json(tick_percentiles.entity_physics),
            "entity_dispatch": runtime_latency_json(tick_percentiles.entity_dispatch),
            "campfire_tick": runtime_latency_json(tick_percentiles.campfire_tick),
            "entity_save": runtime_latency_json(tick_percentiles.entity_save),
            "random_tick": runtime_latency_json(tick_percentiles.random_tick),
            "block_tick": runtime_latency_json(tick_percentiles.block_tick),
            "fluid_tick": runtime_latency_json(tick_percentiles.fluid_tick),
        },
        "chunk_latency": {
            "first": first_chunk_latency,
            "ring1": ring1_latency,
            "ring2": ring2_latency,
            "full_window": full_window_latency,
            "per_client": per_client_latency.iter().map(|(idx, play_ready_ms, latency)| {
                serde_json::json!({
                    "client_index": idx,
                    "play_ready_ms": play_ready_ms,
                    "first_chunk_absolute_ms": play_ready_ms.saturating_add(latency.first_ms),
                    "latency": latency,
                })
            }).collect::<Vec<_>>(),
        },
        "chunk_pipeline": {
            "result_queue_capacity_per_client": O2_VD8_CHUNK_RESULT_QUEUE_SIZE,
            "max_observed_result_queue_depth": drained_snapshot.max_result_queue_depth,
            "queue_full_stops": stop_reason_counts.queue_full,
            "active_io_after_shutdown": drained_snapshot.active_io,
            "max_io_active": drained_snapshot.max_io_active,
            "io_worker_capacity": server.chunk_io_threads,
            "active_cpu_after_shutdown": drained_snapshot.active_cpu,
            "max_cpu_active": drained_snapshot.max_cpu_active,
            "cpu_worker_capacity": server.chunk_worker_threads,
            "stop_reason_counts": {
                "batch_limit": stop_reason_counts.batch_limit,
                "time_budget": stop_reason_counts.time_budget,
                "send_budget": stop_reason_counts.send_budget,
                "load_budget": stop_reason_counts.load_budget,
                "generate_budget": stop_reason_counts.generate_budget,
                "memory_pressure": stop_reason_counts.memory_pressure,
                "queue_full": stop_reason_counts.queue_full,
                "queue_empty": stop_reason_counts.queue_empty,
                "complete": stop_reason_counts.complete,
            },
            "observed_stop_reasons": stop_reason_labels,
        },
        "locks": {
            "world_storage": lock_metric_json(world_storage_delta),
            "session_registry": lock_metric_json(session_registry_delta),
            "save_all_flush": lock_metric_json(save_all_flush_delta),
            "chunk_prepare": lock_metric_json(chunk_prepare_delta),
            "player_persistence": lock_metric_json(player_persistence_delta),
        },
        "memory": {
            "rss_mb": telemetry_final.memory_used_mb,
            "effective_limit_mb": telemetry_final.memory_limit_mb,
        },
        "world": {
            "before": world_storage_stats_json(world_before),
            "loaded": world_storage_stats_json(world_loaded),
            "after_explicit_save": world_storage_stats_json(world_after_explicit_save),
            "after_shutdown": world_storage_stats_json(world_after_shutdown),
        },
        "save": {
            "ok": save_report.is_ok(),
            "players_saved": save_report.players_saved,
            "entities_saved": save_report.entities_saved,
            "chunks_flushed": save_report.chunks_flushed,
            "world_metadata_saved": save_report.world_metadata_saved,
            "errors": save_report.errors,
            "timings_us": {
                "queued": save_report.timings.queued_us,
                "players": save_report.timings.players_us,
                "entities": save_report.timings.entities_us,
                "metadata": save_report.timings.metadata_us,
                "flush_plan": save_report.timings.flush_plan_us,
                "flush_write": save_report.timings.flush_write_us,
                "flush_commit": save_report.timings.flush_commit_us,
                "total": save_report.timings.total_us,
            },
        },
        "runtime_state_at_full_window": {
            "active_sessions": telemetry_loaded.active_sessions,
            "ticketed_chunks": telemetry_loaded.ticketed_chunks,
            "prepared_chunks": telemetry_loaded.prepared_chunks,
            "server_entities": telemetry_loaded.server_entities,
            "furnace_viewer_sets": telemetry_loaded.furnace_viewer_sets,
            "chest_viewer_sets": telemetry_loaded.chest_viewer_sets,
            "entity_dispatches": {
                "spawn": telemetry_loaded.entity_spawn_dispatches,
                "move": telemetry_loaded.entity_move_dispatches,
                "data": telemetry_loaded.entity_data_dispatches,
                "take": telemetry_loaded.entity_take_dispatches,
                "remove": telemetry_loaded.entity_remove_dispatches,
            },
        },
        "outbound_pressure": {
            "before": outbound_pressure_json(outbound_before),
            "after": outbound_pressure_json(outbound_pressure),
        },
        "runtime_control": {
            "draining": runtime_snapshot.draining,
            "limits": {
                "view_distance": runtime_snapshot.limits.view_distance,
                "chunk_send_rate": runtime_snapshot.limits.chunk_send_rate,
                "chunk_load_rate": runtime_snapshot.limits.chunk_load_rate,
                "chunk_generate_rate": runtime_snapshot.limits.chunk_generate_rate,
            },
            "last_action": format!("{:?}", runtime_snapshot.last_decision.action),
            "last_pressure": runtime_snapshot.last_decision.pressure.map(|pressure| format!("{pressure:?}")),
            "autoscale_gaps": autoscale_report.gaps,
        },
    });
    for pointer in [
        "/tick_latency/total/p99_us",
        "/chunk_latency/full_window/p99_ms",
        "/chunk_pipeline/max_observed_result_queue_depth",
        "/locks/session_registry/max_wait_us",
        "/memory/rss_mb",
        "/world/after_shutdown/dirty_chunks",
        "/save/timings_us/total",
        "/runtime_state_at_full_window/server_entities",
        "/outbound_pressure/after/slow_client_pressure_sheds",
    ] {
        assert!(
            report.pointer(pointer).is_some(),
            "Prompt 01 workload report is incomplete at {pointer}: {report}"
        );
    }
    eprintln!(
        "PROMPT01_20_CLIENT_METRICS {}",
        serde_json::to_string(&report).expect("serialize Prompt 01 workload report")
    );

    eprintln!(
        "O2 VD8 20-client same_spawn clients={} per_client_chunks={:?} shared_chunks={} elapsed_ms={} chunk_loaded={:?} chunk_drained={:?} stop_reason_counts={:?} stop_reasons={:?} runtime_snapshot={:?} autoscale_report={:?} locks(world={:?} session={:?} save_all={:?} chunk_prepare={:?})",
        O2_VD8_CONCURRENT_CLIENTS,
        per_client_chunks,
        streamed_chunks.len(),
        elapsed.as_millis(),
        loaded_snapshot,
        drained_snapshot,
        stop_reason_counts,
        stop_reasons,
        runtime_snapshot,
        autoscale_report,
        world_storage_delta,
        session_registry_delta,
        save_all_flush_delta,
        chunk_prepare_delta,
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

    let mut paused_reader = PausedReaderClient::connect(addr, "M52PausedReader").await;

    let (mut active_client, active_sync) = connect_to_play(addr, "M52ActiveReader").await;
    drain_until_chunk(&mut active_client, (0, 0)).await;

    let pressure_before = server.outbound_pressure_snapshot();
    paused_reader
        .trigger_outbound_pressure(DETERMINISTIC_OUTBOUND_PRESSURE_BURST)
        .await;
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

    let mut paused_reader = PausedReaderClient::connect(addr, "M52PauseObs").await;
    let (mut actor, actor_sync) = connect_to_play(addr, "M52PressureActor").await;
    drain_until_chunk(&mut actor, (0, 0)).await;
    let (mut observer_a, _) = connect_to_play(addr, "M52ObserverA").await;
    drain_until_chunk(&mut observer_a, (0, 0)).await;
    let (mut observer_b, _) = connect_to_play(addr, "M52ObserverB").await;
    drain_until_chunk(&mut observer_b, (0, 0)).await;

    let pressure_before = server.outbound_pressure_snapshot();
    paused_reader
        .trigger_outbound_pressure(DETERMINISTIC_OUTBOUND_PRESSURE_BURST)
        .await;
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
    items: Arc<mc_data::items::ItemRegistry>,
    entity_types: Arc<mc_data::entity_types::EntityTypeRegistry>,
    world: Arc<tokio::sync::Mutex<mc_world::WorldStorage>>,
    chunk_pipeline_metrics: mc_net::ChunkPipelineResourceMetrics,
    outbound_pressure: mc_net::OutboundPressureHandle,
    runtime_telemetry: mc_net::RuntimeTelemetryHandle,
    save_handle: mc_net::SaveHandle,
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
    existing_world_path: Option<PathBuf>,
    spawn_passive_entities: bool,
    runtime_control: bool,
    max_players: usize,
    fixed_runtime_view_distance: bool,
    chunk_result_queue_size: usize,
}

impl Default for LoadServerOptions {
    fn default() -> Self {
        Self {
            view_distance: VIEW_DISTANCE,
            disk_backed: false,
            existing_world_path: None,
            spawn_passive_entities: true,
            runtime_control: false,
            max_players: 8,
            fixed_runtime_view_distance: false,
            chunk_result_queue_size: 8,
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
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let world_dir = if options.disk_backed && options.existing_world_path.is_none() {
        let temp = tempfile::tempdir().expect("create disk-backed load world");
        Some(temp)
    } else {
        None
    };
    let world_path = if options.disk_backed {
        Some(options.existing_world_path.clone().unwrap_or_else(|| {
            world_dir
                .as_ref()
                .expect("owned disk world")
                .path()
                .to_owned()
        }))
    } else {
        None
    };
    if let Some(path) = world_path.as_ref() {
        std::fs::create_dir_all(path.join("region")).expect("create legacy region dir");
    }
    let chunk_capacity = ((2 * options.view_distance + 5) as usize).pow(2);
    let storage = if let Some(path) = world_path.as_ref() {
        mc_world::WorldStorage::open_with_capacity(path, Arc::clone(&blocks), chunk_capacity)
            .expect("open disk-backed load world")
    } else {
        mc_world::WorldStorage::in_memory_with_capacity(Arc::clone(&blocks), chunk_capacity)
    }
    .with_generator(generator)
    .with_item_registry(Arc::clone(&items));
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let block_light = mc_data::block_light::load(vanilla_dir.join("reports/block_light.json"))
        .ok()
        .map(Arc::new);
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("exact 26.1.2 entity registry"),
    );
    let biome_spawns = if options.spawn_passive_entities {
        mc_data::biomes::load_biome_spawn_rules(vanilla_dir.join("data/minecraft/worldgen/biome"))
            .map(Arc::new)
            .unwrap_or_default()
    } else {
        Arc::new(mc_data::biomes::BiomeSpawnRules::default())
    };

    let mut chunk_pipeline = mc_net::ChunkPipelinePolicy {
        chunk_prepare_batch_size: 2,
        chunk_io_threads: LOAD_CHUNK_IO_THREADS,
        chunk_worker_threads: LOAD_CHUNK_WORKER_THREADS,
        chunk_result_queue_size: options.chunk_result_queue_size,
        ..mc_net::ChunkPipelinePolicy::default()
    };
    if options.runtime_control {
        let initial_limits = mc_net::RuntimeControlLimits {
            view_distance: options.view_distance,
            chunk_send_rate: 8,
            chunk_load_rate: 16,
            chunk_generate_rate: 32,
        };
        let mut policy = mc_net::AutoscalePolicy::for_profile(mc_net::AutoscaleProfile::Balanced);
        if options.fixed_runtime_view_distance {
            policy.min_view_distance = options.view_distance;
            policy.max_view_distance = options.view_distance;
            policy.min_chunk_send_rate = initial_limits.chunk_send_rate;
            policy.max_chunk_send_rate = initial_limits.chunk_send_rate;
            policy.min_chunk_load_rate = initial_limits.chunk_load_rate;
            policy.max_chunk_load_rate = initial_limits.chunk_load_rate;
            policy.min_chunk_generate_rate = initial_limits.chunk_generate_rate;
            policy.max_chunk_generate_rate = initial_limits.chunk_generate_rate;
        }
        chunk_pipeline.runtime_control = Some(mc_net::RuntimeControlConfig {
            policy,
            initial_limits,
        });
    }

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M37 load scenarios".into(),
        max_players: u32::try_from(options.max_players).expect("load max_players fits u32"),
        view_distance: options.view_distance,
        data,
        blocks: Arc::clone(&blocks),
        world: Some(Arc::clone(&world)),
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light,
        items: Arc::clone(&items),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: Arc::clone(&entity_types),
        biome_spawns,
        chunk_pipeline,
        random_tick: mc_net::RandomTickPolicy {
            simulation_distance: options.view_distance,
            random_tick_speed: 3,
            chunk_budget: 8,
            fluid_tick_budget: 64,
            save_interval_ticks: 20,
            spawn_monsters: true,
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
    let runtime_telemetry = bound.runtime_telemetry_handle();
    let save_handle = bound.save_handle();
    let serve_task = tokio::spawn(async move { bound.serve().await });
    LoadServer {
        addr,
        blocks,
        items,
        entity_types,
        world,
        chunk_pipeline_metrics,
        outbound_pressure,
        runtime_telemetry,
        save_handle,
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
        let chunks = drain_unique_chunks(&mut client, 9).await;
        assert_eq!(chunks.len(), 9, "paused reader must finish VD1 stream");
        Self { client, sync }
    }

    fn close(self) {
        drop(self.client);
    }

    async fn trigger_outbound_pressure(&mut self, count: usize) {
        assert!(
            self.try_trigger_outbound_pressure(count).await,
            "send deterministic outbound pressure probe"
        );
    }

    async fn try_trigger_outbound_pressure(&mut self, count: usize) -> bool {
        self.client
            .write_packet(&ServerboundChatCommand {
                command: format!("debug outbound-pressure {count}"),
            })
            .await
            .is_ok()
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
    let mut observed = before;
    loop {
        let after = match tokio::time::timeout_at(
            deadline,
            server.outbound_pressure.wait_for_change(observed),
        )
        .await
        {
            Ok(after) => after,
            Err(_) => {
                let after = server.outbound_pressure_snapshot();
                panic!(
                    "paused reader did not produce an outbound pressure event: before={before:?} after={after:?}"
                );
            }
        };
        if after.reliable_command_retries > before.reliable_command_retries
            || after.best_effort_animation_drops > before.best_effort_animation_drops
            || after.reliable_command_drops > before.reliable_command_drops
            || after.slow_client_write_timeouts > before.slow_client_write_timeouts
            || after.slow_client_pressure_sheds > before.slow_client_pressure_sheds
        {
            return after;
        }
        observed = after;
    }
}

fn assert_slow_reader_retry_bounded(
    before: mc_net::OutboundPressureSnapshot,
    after: mc_net::OutboundPressureSnapshot,
) {
    assert_eq!(
        after.reliable_command_drops, before.reliable_command_drops,
        "paused reader pressure must not lose reliable commands: before={before:?} after={after:?}"
    );
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

fn lock_metric_delta(
    before: mc_net::LockMetricSnapshot,
    after: mc_net::LockMetricSnapshot,
) -> mc_net::LockMetricSnapshot {
    mc_net::LockMetricSnapshot {
        wait_count: after.wait_count.saturating_sub(before.wait_count),
        wait_us: after.wait_us.saturating_sub(before.wait_us),
        max_wait_us: after.max_wait_us,
        hold_count: after.hold_count.saturating_sub(before.hold_count),
        hold_us: after.hold_us.saturating_sub(before.hold_us),
        max_hold_us: after.max_hold_us,
    }
}

fn assert_lock_metric_observed_within_budget(
    name: &str,
    delta: mc_net::LockMetricSnapshot,
    after: mc_net::LockMetricSnapshot,
    max_hold_budget_us: u64,
) {
    assert!(
        delta.hold_count > 0,
        "{name} lock path should be exercised: delta={delta:?} after={after:?}"
    );
    assert!(
        after.max_hold_us <= max_hold_budget_us,
        "{name} lock hold exceeded O2 VD8 budget: after={after:?} budget_us={max_hold_budget_us}"
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

async fn wait_for_center_chunk(client: &mut Client, target: (i32, i32)) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for center chunk {target:?}"
        );
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .expect("center chunk frame");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == SetCenterChunk::ID {
            let mut body = frame.body;
            let packet = SetCenterChunk::decode(&mut body).expect("decode SetCenterChunk");
            if (packet.chunk_x, packet.chunk_z) == target {
                return;
            }
        }
    }
}

async fn wait_for_active_sessions(server: &LoadServer, expected: usize) {
    let mut active_sessions = server.runtime_telemetry.subscribe_active_sessions();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if *active_sessions.borrow_and_update() == expected {
                return;
            }
            active_sessions
                .changed()
                .await
                .expect("active session sender remains");
        }
    })
    .await;
    assert!(
        result.is_ok(),
        "timed out waiting for {expected} active sessions; observed {}",
        server.runtime_telemetry.snapshot().active_sessions
    );
}

async fn wait_for_active_sessions_at_most(server: &LoadServer, maximum: usize) {
    let mut active_sessions = server.runtime_telemetry.subscribe_active_sessions();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if *active_sessions.borrow_and_update() <= maximum {
                return;
            }
            active_sessions
                .changed()
                .await
                .expect("active session sender remains");
        }
    })
    .await;
    assert!(
        result.is_ok(),
        "timed out waiting for at most {maximum} active sessions; observed {}",
        server.runtime_telemetry.snapshot().active_sessions
    );
}

async fn wait_for_chunk_cancellation(
    server: &LoadServer,
    before: mc_net::ChunkPipelineCancellationSnapshot,
) -> mc_net::ChunkPipelineCancellationSnapshot {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let after = server.chunk_pipeline_metrics.cancellation_snapshot();
        if after.cancelled_streams > before.cancelled_streams {
            return after;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for chunk-stream cancellation: before={before:?} after={after:?}"
        );
        tokio::task::yield_now().await;
    }
}

async fn drain_unique_chunks(
    client: &mut Client,
    expected: usize,
) -> std::collections::BTreeSet<(i32, i32)> {
    drain_unique_chunks_with_timeout(client, expected, Duration::from_secs(30), "client").await
}

async fn drain_unique_chunks_with_timeout(
    client: &mut Client,
    expected: usize,
    timeout: Duration,
    label: &str,
) -> std::collections::BTreeSet<(i32, i32)> {
    try_drain_unique_chunks_with_timeout(client, expected, timeout, label)
        .await
        .unwrap_or_else(|err| panic!("{err}"))
}

async fn try_drain_unique_chunks_with_timeout(
    client: &mut Client,
    expected: usize,
    timeout: Duration,
    label: &str,
) -> Result<std::collections::BTreeSet<(i32, i32)>, String> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut seen = std::collections::BTreeSet::new();
    loop {
        if seen.len() >= expected {
            return Ok(seen);
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "timed out waiting for {expected} unique chunks for {label}; seen_count={} seen={seen:?}",
                seen.len()
            ));
        }
        let frame = match client.read_frame_with_timeout(remaining).await {
            Ok(frame) => frame,
            Err(err) => {
                return Err(format!(
                    "failed waiting for {expected} unique chunks for {label}; seen_count={} seen={seen:?}: {err}",
                    seen.len()
                ));
            }
        };
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

async fn try_drain_vd8_chunks_with_timing(
    client: &mut Client,
    timeout: Duration,
    label: &str,
) -> Result<TimedChunkDrain, String> {
    const RING1_CHUNKS: usize = 9;
    const RING2_CHUNKS: usize = 25;

    let started = Instant::now();
    let deadline = tokio::time::Instant::now() + timeout;
    let mut seen = std::collections::BTreeSet::new();
    let mut first_ms = None;
    let mut ring1_ms = None;
    let mut ring2_ms = None;
    loop {
        if seen.len() >= O2_VD8_WINDOW_CHUNKS {
            return Ok(TimedChunkDrain {
                chunks: seen,
                latency: ChunkWindowLatencyMs {
                    first_ms: first_ms.expect("full window includes first chunk"),
                    ring1_ms: ring1_ms.expect("full window includes ring 1"),
                    ring2_ms: ring2_ms.expect("full window includes ring 2"),
                    full_ms: elapsed_ms(started),
                },
            });
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "timed out waiting for {} unique chunks for {label}; seen_count={} seen={seen:?}",
                O2_VD8_WINDOW_CHUNKS,
                seen.len()
            ));
        }
        let frame = match client.read_frame_with_timeout(remaining).await {
            Ok(frame) => frame,
            Err(err) => {
                return Err(format!(
                    "failed waiting for {} unique chunks for {label}; seen_count={} seen={seen:?}: {err}",
                    O2_VD8_WINDOW_CHUNKS,
                    seen.len()
                ));
            }
        };
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            let mut body = frame.body;
            let pkt = LevelChunkWithLight::decode(&mut body).expect("decode chunk");
            if seen.insert((pkt.chunk_x, pkt.chunk_z)) {
                let latency_ms = elapsed_ms(started);
                first_ms.get_or_insert(latency_ms);
                if seen.len() >= RING1_CHUNKS {
                    ring1_ms.get_or_insert(latency_ms);
                }
                if seen.len() >= RING2_CHUNKS {
                    ring2_ms.get_or_insert(latency_ms);
                }
            }
        }
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn runtime_latency_json(percentiles: mc_net::RuntimeLatencyPercentiles) -> serde_json::Value {
    serde_json::json!({
        "samples": percentiles.samples,
        "p50_us": percentiles.p50_us,
        "p95_us": percentiles.p95_us,
        "p99_us": percentiles.p99_us,
        "max_us": percentiles.max_us,
    })
}

fn world_storage_stats_json(stats: mc_world::storage::WorldStorageStats) -> serde_json::Value {
    serde_json::json!({
        "chunk_cache_len": stats.chunk_cache_len,
        "chunk_cache_capacity": stats.chunk_cache_capacity,
        "region_cache_len": stats.region_cache_len,
        "region_cache_capacity": stats.region_cache_capacity,
        "dirty_chunks": stats.dirty_chunks,
        "dirty_chunk_cache_saturated": stats.dirty_chunk_cache_saturated,
    })
}

fn lock_metric_json(snapshot: mc_net::LockMetricSnapshot) -> serde_json::Value {
    serde_json::json!({
        "wait_count": snapshot.wait_count,
        "wait_us": snapshot.wait_us,
        "max_wait_us": snapshot.max_wait_us,
        "hold_count": snapshot.hold_count,
        "hold_us": snapshot.hold_us,
        "max_hold_us": snapshot.max_hold_us,
    })
}

fn outbound_pressure_json(snapshot: mc_net::OutboundPressureSnapshot) -> serde_json::Value {
    serde_json::json!({
        "best_effort_animation_drops": snapshot.best_effort_animation_drops,
        "reliable_command_drops": snapshot.reliable_command_drops,
        "reliable_command_retries": snapshot.reliable_command_retries,
        "reliable_command_retries_in_flight": snapshot.reliable_command_retries_in_flight,
        "max_reliable_command_retries_in_flight": snapshot.max_reliable_command_retries_in_flight,
        "slow_client_write_timeouts": snapshot.slow_client_write_timeouts,
        "slow_client_pressure_sheds": snapshot.slow_client_pressure_sheds,
    })
}

fn workload_git_provenance() -> (String, bool) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&root)
        .output()
        .expect("run git rev-parse for workload provenance");
    assert!(commit.status.success(), "git rev-parse should succeed");
    let commit = String::from_utf8(commit.stdout)
        .expect("git commit is UTF-8")
        .trim()
        .to_owned();
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=normal"])
        .current_dir(root)
        .output()
        .expect("run git status for workload provenance");
    assert!(status.status.success(), "git status should succeed");
    (commit, !status.stdout.is_empty())
}

fn workload_sidecar_version() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla/version.json");
    let value: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&path)
            .unwrap_or_else(|err| panic!("read workload sidecar {}: {err}", path.display())),
    )
    .unwrap_or_else(|err| panic!("parse workload sidecar {}: {err}", path.display()));
    value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| panic!("workload sidecar {} has no version id", path.display()))
        .to_owned()
}

async fn drain_client_until_shutdown(mut client: Client, shutdown: mc_net::ShutdownHandle) {
    loop {
        tokio::select! {
            biased;
            () = shutdown.wait_requested() => return,
            frame = client.read_frame() => {
                let frame = frame.expect("read client frame while waiting for shutdown");
                let _ = handle_keepalive(&mut client, frame.id, &frame.body).await;
            }
        }
    }
}

async fn wait_for_shutdown_requested(shutdown: &mc_net::ShutdownHandle) {
    tokio::time::timeout(Duration::from_secs(30), shutdown.wait_requested())
        .await
        .expect("timed out waiting for player /stop to request shutdown");
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

async fn wait_for_load_open_screen(client: &mut Client, menu_type: i32) -> ClientboundOpenScreen {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "timed out waiting for open screen");
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .expect("open screen frame");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundOpenScreen::ID {
            let mut body = frame.body;
            let packet = ClientboundOpenScreen::decode(&mut body).expect("decode OpenScreen");
            if packet.menu_type == menu_type {
                return packet;
            }
        }
    }
}

async fn wait_for_load_container_content(
    client: &mut Client,
    container_id: i32,
    predicate: impl Fn(&ClientboundContainerSetContent) -> bool,
) -> ClientboundContainerSetContent {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for container {container_id} content"
        );
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .expect("container content frame");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetContent::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetContent::decode(&mut body)
                .expect("decode ContainerSetContent");
            if packet.container_id == container_id && predicate(&packet) {
                return packet;
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

async fn wait_for_inventory_empty(client: &mut Client) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for empty authoritative hotbar slot"
        );
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .expect("empty inventory sync frame");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode empty inventory ContainerSetSlot");
            if packet.container_id == 0 && packet.slot == 36 && packet.item_stack.is_empty() {
                return;
            }
        }
    }
}

async fn wait_for_placement_consumption(
    client: &mut Client,
    expected_sequence: i32,
    target_pos: i64,
) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut ack_at: Option<tokio::time::Instant> = None;
    let mut saw_target = false;
    let mut consumed = None;
    loop {
        if let Some(ack_at) = ack_at
            && saw_target
            && (consumed.is_some() || ack_at.elapsed() >= Duration::from_millis(500))
        {
            return consumed.unwrap_or(false);
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for placement result sequence={expected_sequence} target={:?}",
            unpack_block_pos(target_pos)
        );
        let read_budget = ack_at
            .map(|ack_at: tokio::time::Instant| {
                (ack_at + Duration::from_millis(500))
                    .saturating_duration_since(tokio::time::Instant::now())
                    .min(remaining)
            })
            .unwrap_or(remaining);
        let frame = match client.read_frame_with_timeout(read_budget).await {
            Ok(frame) => frame,
            Err(_) if ack_at.is_some() && saw_target => return consumed.unwrap_or(false),
            Err(err) => panic!("placement result read failed: {err}"),
        };
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let packet = BlockUpdate::decode(&mut body).expect("decode placement BlockUpdate");
            saw_target |= packet.position == target_pos;
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode placement ContainerSetSlot");
            if packet.container_id == 0 && packet.slot == 36 {
                consumed = Some(packet.item_stack.is_empty());
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode placement ack");
            if packet.sequence == expected_sequence {
                ack_at = Some(tokio::time::Instant::now());
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
