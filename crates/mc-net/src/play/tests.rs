use super::block_placement::plan_block_placement;
use super::command_execution::{DebugCommandContext, debug_water_corridor_edits};
use super::containers::furnace_fuel_ticks;
use super::falling_blocks::{FallingBlockStart, LandedFallingBlock, plan_falling_block_starts};
use super::use_item_on_adapter::cursor_y_relative_to_target;
use super::*;
use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use crate::play::chunk_stream::{hostile_chunk_spawns, passive_chunk_spawns, prioritized_spiral};
use mc_data::blocks::{BlockReport, BlockStateReport, solaris_required_blocks_report};
use mc_data::items::ItemReport;
use mc_world::light::compute_chunk_light_in;
use tokio::io::{AsyncWrite, AsyncWriteExt};

fn no_script_player_context(session_id: SessionId) -> ScriptPlayerContext {
    ScriptPlayerContext::new(
        format!("test-player-{session_id}"),
        "TestPlayer",
        false,
        0.5,
        64.0,
        0.5,
    )
}

#[test]
fn session_owner_script_inventory_commit_updates_live_and_durable_state_together() {
    let items = Arc::new(mc_data::items::solaris_required_items());
    let mut state = interaction_state_for_items(Arc::clone(&items));
    state.item_facts = Arc::new(mc_data::item_components::solaris_required_item_facts());
    let apple = items
        .id_of(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let emerald = items
        .id_of(&Identifier::parse("minecraft:emerald").unwrap())
        .unwrap();
    state.inventory.slots[9] = ItemStack::new(apple, 3);
    let transaction = mc_script::ScriptPlayerInventoryTransaction::try_new(
        "owner-exchange",
        mc_script::ScriptPlayerId::new(state.session_id),
        vec![
            mc_script::ScriptInventoryResourceDelta::try_new("minecraft:apple", -2).unwrap(),
            mc_script::ScriptInventoryResourceDelta::try_new("minecraft:emerald", 4).unwrap(),
        ],
    )
    .unwrap();

    assert_eq!(
        commit_session_owner_script_player_inventory(&mut state, &transaction),
        Ok(())
    );
    assert_eq!(state.inventory.slots[9], ItemStack::new(apple, 1));
    assert!(
        state.inventory.slots[9..=44]
            .iter()
            .any(|stack| *stack == ItemStack::new(emerald, 4))
    );
    assert_eq!(
        state.player_persistence.lock().unwrap().inventory.slots,
        state.inventory.slots
    );
}

async fn run_scheduled_block_ticks(
    config: &ServerConfig,
    sessions: &SessionRegistry,
    world_tick: u64,
) -> ScheduledBlockTickReport {
    let (_handle, owner) = simulation_channel();
    let shared_world = config.world.as_ref().unwrap();
    let storage = shared_world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    owner
        .run_scheduled_block_ticks_with_budget(
            config,
            sessions,
            SimulationWorldAccess {
                read: Some(&world_read),
                mutation: Some(&world_mutation),
                ..SimulationWorldAccess::default()
            },
            world_tick,
            config.random_tick.normalized().fluid_tick_budget,
        )
        .await
}

async fn run_scheduled_block_ticks_with_protection(
    config: &ServerConfig,
    sessions: &SessionRegistry,
    protection: Arc<crate::script::ZoneProtectionSnapshot>,
    world_tick: u64,
) -> ScheduledBlockTickReport {
    let shared_world = config.world.as_ref().unwrap();
    let storage = shared_world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    run_scheduled_block_ticks_owned(
        config,
        sessions,
        SimulationWorldAccess {
            read: Some(&world_read),
            mutation: Some(&world_mutation),
            ..SimulationWorldAccess::default()
        },
        None,
        Some(protection),
        world_tick,
        config.random_tick.normalized().fluid_tick_budget,
    )
    .await
}

async fn run_scheduled_fluid_ticks(
    config: &ServerConfig,
    sessions: &SessionRegistry,
    world_tick: u64,
) -> ScheduledFluidTickReport {
    let (_handle, owner) = simulation_channel();
    let shared_world = config.world.as_ref().unwrap();
    let storage = shared_world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    owner
        .run_scheduled_fluid_ticks_with_budget(
            config,
            sessions,
            Some(&world_read),
            Some(&world_mutation),
            world_tick,
            config.random_tick.normalized().fluid_tick_budget,
        )
        .await
}

#[tokio::test]
async fn chunk_stream_wait_wakes_on_memory_sample_change() {
    let memory_pressure = crate::memory_pressure::MemoryPressureHandle::with_sample(
        crate::memory_pressure::MemoryPressureSnapshot {
            used_mb: 900,
            limit_mb: 1_000,
        },
    );
    let mut memory_changes = memory_pressure.subscribe();
    let sessions = SessionRegistry::new();
    let prepared_generation = sessions.prepared_change_generation();

    let wake = wait_for_chunk_stream_wake(
        Arc::new(tokio::sync::Notify::new()),
        &sessions,
        prepared_generation,
        Some(&mut memory_changes),
    );
    tokio::pin!(wake);

    memory_pressure.set_sample(crate::memory_pressure::MemoryPressureSnapshot {
        used_mb: 100,
        limit_mb: 1_000,
    });

    tokio::time::timeout(Duration::from_secs(1), wake)
        .await
        .expect("memory sample event must wake the chunk stream");
}

fn props(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

#[test]
fn player_pose_metadata_reports_swimming_and_shared_flags() {
    let mut pose = PlayerPose::new(0.5, 62.0, 0.5);
    pose.in_water = true;
    pose.swimming = true;
    pose.sprinting = true;

    assert_eq!(pose.entity_pose(), EntityPose::Swimming);
    assert_eq!(pose.shared_flags() & 0x08, 0x08);
    assert_eq!(pose.shared_flags() & 0x10, 0x10);
}

#[test]
fn survival_movement_exhaustion_tracks_sprint_and_sprint_jump_distance() {
    let mut old = PlayerPose::new(0.5, 72.0, 0.5);
    old.flags = MovePlayerFlags::new(true, false);
    let mut standing_jump = PlayerPose::new(0.5, 73.0, 0.5);
    standing_jump.flags = MovePlayerFlags::new(false, false);
    standing_jump.input.jump = true;

    assert_eq!(
        movement_exhaustion(old, standing_jump),
        SurvivalState::JUMP_EXHAUSTION
    );

    let mut walking = PlayerPose::new(4.5, 72.0, 0.5);

    assert_eq!(movement_exhaustion(old, walking), 0.0);

    walking.sprinting = true;
    let sprint_exhaustion = movement_exhaustion(old, walking);
    assert!(sprint_exhaustion > 0.0);

    let mut sprint_jump = walking;
    sprint_jump.y = 73.0;
    sprint_jump.flags = MovePlayerFlags::new(false, false);
    sprint_jump.input.jump = true;

    assert!(movement_exhaustion(old, sprint_jump) > sprint_exhaustion);
}

#[test]
fn player_movement_clamps_extreme_coordinates_and_rejects_non_finite_values() {
    let finite = AcceptedAbsoluteMovement {
        x: 1.0,
        y: 64.0,
        z: -2.0,
        yaw_pitch: Some((90.0, 15.0)),
        flags: MovePlayerFlags::new(true, false),
    };
    assert_eq!(
        normalize_absolute_player_movement(finite)
            .expect("finite movement is accepted")
            .x,
        1.0
    );

    let clamped = normalize_absolute_player_movement(AcceptedAbsoluteMovement {
        x: f64::MAX,
        y: -f64::MAX,
        z: -f64::MAX,
        ..finite
    })
    .expect("finite extreme movement is clamped");
    assert_eq!(clamped.x, 30_000_000.0);
    assert_eq!(clamped.y, -20_000_000.0);
    assert_eq!(clamped.z, -30_000_000.0);

    for movement in [
        AcceptedAbsoluteMovement {
            x: f64::NAN,
            ..finite
        },
        AcceptedAbsoluteMovement {
            y: f64::INFINITY,
            ..finite
        },
        AcceptedAbsoluteMovement {
            z: f64::NEG_INFINITY,
            ..finite
        },
        AcceptedAbsoluteMovement {
            yaw_pitch: Some((f32::NAN, 0.0)),
            ..finite
        },
        AcceptedAbsoluteMovement {
            yaw_pitch: Some((0.0, f32::INFINITY)),
            ..finite
        },
    ] {
        assert!(matches!(
            normalize_absolute_player_movement(movement),
            Err(ConnectionError::InvalidPlayerMovement)
        ));
    }

    assert!(matches!(
        validate_player_rotation(f32::NEG_INFINITY, 0.0),
        Err(ConnectionError::InvalidPlayerMovement)
    ));
}

#[test]
fn survival_food_update_saturates_extreme_input() {
    let mut state = SurvivalState::FULL;

    state.add_food(i32::MAX, f32::MAX);

    assert_eq!(state.food, SurvivalState::MAX_FOOD);
    assert_eq!(state.saturation, SurvivalState::MAX_FOOD as f32);
}

#[test]
fn survival_exhaustion_handles_extreme_input_in_bounded_work() {
    let mut state = SurvivalState::FULL;

    assert!(state.add_exhaustion(f32::MAX));

    assert_eq!(state.food, 0);
    assert_eq!(state.saturation, 0.0);
    assert!(state.exhaustion.is_finite());
    assert!((0.0..SurvivalState::EXHAUSTION_STEP).contains(&state.exhaustion));

    assert!(!state.add_exhaustion(f32::INFINITY));
    assert!(state.exhaustion.is_finite());
}

#[test]
fn clientbound_session_world_time_separates_monotonic_and_overworld_clocks() {
    let sessions = SessionRegistry::new();
    sessions.set_world_time(12_345);
    sessions.advance_world_time(7);

    let packet = clientbound_session_world_time(&sessions);
    assert_eq!(packet.game_time, 7);
    assert_eq!(
        packet.overworld_clock,
        Some(mc_protocol::packets::play::WorldClockUpdate {
            total_ticks: 12_352,
            partial_tick: 0.0,
            rate: 1.0,
        })
    );

    assert_eq!(clientbound_world_time(u64::MAX, 1).game_time, i64::MAX);
}

#[test]
fn text_component_nbt_reports_oversized_text_instead_of_panicking() {
    let oversized = "x".repeat(usize::from(u16::MAX) + 1);

    let err = text_component_nbt(&oversized).expect_err("oversized NBT string should fail");

    assert!(matches!(err, mc_protocol::CodecError::Nbt(_)));
}

#[test]
fn login_rejects_corrupt_player_state_without_overwriting_it() {
    let tmp = tempfile::tempdir().unwrap();
    let uuid = uuid::Uuid::from_u128(0x1234);
    let path = tmp.path().join(format!("playerdata/{uuid}.dat"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let corrupt = b"not gzip nbt";
    std::fs::write(&path, corrupt).unwrap();
    let items = ItemRegistry::from_report(&[]);

    let error = load_player_state_for_login(
        tmp.path(),
        uuid,
        &items,
        PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 0.5)),
    )
    .expect_err("corrupt playerdata must reject login");

    assert!(error.to_string().contains("player state load failed"));
    assert_eq!(std::fs::read(path).unwrap(), corrupt);
}

struct StalledWriter;

impl AsyncWrite for StalledWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Pending
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Pending
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

struct AllowThenStallWriter {
    remaining_ready_writes: usize,
}

impl AllowThenStallWriter {
    const fn new(remaining_ready_writes: usize) -> Self {
        Self {
            remaining_ready_writes,
        }
    }
}

impl AsyncWrite for AllowThenStallWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.remaining_ready_writes == 0 {
            return Poll::Pending;
        }
        self.remaining_ready_writes -= 1;
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn play_loop_slow_client_test_config() -> crate::server::ServerConfig {
    crate::server::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "slow-client-test".into(),
        max_players: 1,
        view_distance: 0,
        data: Arc::new(mc_data::testing::stub()),
        blocks: Arc::new(mc_world::BlockRegistry::from_report(&[]).unwrap()),
        world: None,
        tags: Arc::new(mc_data::tags::TagsData::default()),
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items: Arc::new(mc_data::items::ItemRegistry::from_report(&[])),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: ChunkPipelinePolicy::default(),
        random_tick: RandomTickPolicy::default(),
        command_permissions: crate::server::CommandPermissionConfig::new(
            Vec::<String>::new(),
            true,
        ),
        loader_manifest: None,
        shutdown: crate::server::ShutdownHandle::default(),
    }
}

#[tokio::test]
async fn initial_play_sync_sends_recipe_update_once_before_recipe_book_packets() {
    let mut config = play_loop_slow_client_test_config();
    config.items = stonecutter_test_items();
    config.recipes = Arc::new(vec![stonecutter_test_recipe()]);
    let sessions = Arc::new(SessionRegistry::new());
    let (simulation, _owner) = simulation_channel();
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("InitialRecipeSync"),
        name: "InitialRecipeSync".to_owned(),
    };
    let mut reader = tokio::io::empty();
    let mut writer = Vec::new();
    let mut buf = BytesMut::new();

    let result = handle(
        &mut reader,
        &mut writer,
        &mut buf,
        Compression::Disabled,
        &profile,
        &[],
        CommandPermissions { op: false },
        &config,
        crate::server::ConnectionWorld::default(),
        sessions,
        ChunkPipelineResources::with_limits(1, 1),
        None,
        None,
        simulation,
        Vec::new(),
        None,
        None,
        None,
        None,
    )
    .await;
    assert!(matches!(result, Err(ConnectionError::Eof)));

    let mut frames = bytes::BytesMut::from(writer.as_slice());
    let mut packet_ids = Vec::new();
    while let Some(frame) =
        mc_protocol::frame::try_decode_frame(&mut frames, Compression::Disabled).unwrap()
    {
        packet_ids.push(frame.id);
    }
    let update_positions = packet_ids
        .iter()
        .enumerate()
        .filter_map(|(index, id)| {
            (*id == mc_protocol::packets::play::ClientboundUpdateRecipes::ID).then_some(index)
        })
        .collect::<Vec<_>>();
    let settings = packet_ids
        .iter()
        .position(|id| *id == ClientboundRecipeBookSettings::ID)
        .expect("initial recipe book settings packet");
    let recipes = packet_ids
        .iter()
        .position(|id| *id == mc_protocol::packets::play::ClientboundRecipeBookAdd::ID)
        .expect("initial recipe book add packet");

    assert_eq!(update_positions.len(), 1);
    assert!(update_positions[0] < settings);
    assert!(settings < recipes);
}

#[test]
fn outbound_command_queue_capacity_scales_with_player_burst() {
    let mut config = play_loop_slow_client_test_config();
    config.max_players = 20;
    config.chunk_pipeline.chunk_result_queue_size = 8;

    assert_eq!(
        outbound_command_queue_capacity(&config),
        20 * OUTBOUND_COMMANDS_PER_PLAYER_BURST
    );
}

#[test]
fn outbound_command_queue_capacity_preserves_larger_configured_queue() {
    let mut config = play_loop_slow_client_test_config();
    config.max_players = 2;
    config.chunk_pipeline.chunk_result_queue_size = 512;

    assert_eq!(outbound_command_queue_capacity(&config), 512);
}

#[tokio::test]
async fn outbound_command_write_timeout_sheds_stalled_client() {
    let mut writer = StalledWriter;
    let (mut writer, blocked) = SlowClientWriteGuard::new(&mut writer);

    let outcome = slow_client_outbound_write_timeout(
        write_packet(
            &mut writer,
            &EntityEvent {
                entity_id: 1,
                event_id: 2,
            },
            Compression::Disabled,
        ),
        blocked,
        Duration::from_millis(1),
    )
    .await
    .expect("stalled outbound write timeout should close cleanly");

    assert_eq!(outcome, OutboundWriteOutcome::TimedOut);
}

#[tokio::test]
async fn outbound_command_timeout_starts_only_after_write_blocks() {
    let (domain_started, domain_started_rx) = oneshot::channel();
    let (release_domain, release_domain_rx) = oneshot::channel();
    let release_task = tokio::spawn(async move {
        domain_started_rx.await.expect("domain work should start");
        release_domain
            .send(())
            .expect("command should still await domain work");
    });
    let mut bytes = Vec::new();
    let (mut writer, blocked) = SlowClientWriteGuard::new(&mut bytes);

    let outcome = slow_client_outbound_write_timeout(
        async {
            domain_started.send(()).expect("release task should wait");
            release_domain_rx
                .await
                .expect("domain work should be released by its event");
            write_packet(
                &mut writer,
                &EntityEvent {
                    entity_id: 1,
                    event_id: 2,
                },
                Compression::Disabled,
            )
            .await
        },
        blocked,
        Duration::ZERO,
    )
    .await
    .expect("pre-write domain work must not trip a client write timeout");
    release_task.await.expect("release task should finish");

    assert_eq!(outcome, OutboundWriteOutcome::Sent);
    assert!(!bytes.is_empty());
}

#[tokio::test]
async fn chunk_stream_write_timeout_sheds_stalled_client() {
    let sessions = SessionRegistry::new();
    let start_timeouts = sessions.pressure_snapshot().slow_client_write_timeouts;

    let step = slow_client_chunk_stream_step_timeout(
        &sessions,
        91,
        std::future::pending::<Result<ChunkStreamStep, ConnectionError>>(),
        Duration::ZERO,
    )
    .await
    .expect("stalled chunk write should close cleanly");

    assert_eq!(step, None);
    assert_eq!(
        sessions.pressure_snapshot().slow_client_write_timeouts,
        start_timeouts + 1
    );
}

#[test]
fn keepalive_tracker_only_accepts_the_matching_echo() {
    let mut keepalive = KeepAliveTracker::new();
    let request_id = keepalive.record_request().expect("first request");

    assert!(!keepalive.record_response(request_id + 1));
    assert_eq!(keepalive.pending_id(), Some(request_id));
    assert!(keepalive.record_response(request_id));
    assert_eq!(keepalive.pending_id(), None);
}

#[test]
fn keepalive_tracker_never_replaces_an_unanswered_request() {
    let mut keepalive = KeepAliveTracker::new();
    let request_id = keepalive.record_request().expect("first request");

    assert_eq!(keepalive.record_request(), None);
    assert_eq!(keepalive.pending_id(), Some(request_id));
    assert!(keepalive.record_response(request_id));
    assert!(keepalive.record_request().is_some());
}

#[test]
fn keepalive_timeout_requires_the_whole_connection_to_be_idle() {
    let mut keepalive = KeepAliveTracker::new();
    keepalive.record_request().expect("first request");
    keepalive.pending_since = Some(Instant::now() - KEEPALIVE_TIMEOUT - Duration::from_secs(1));

    keepalive.record_inbound_activity();
    assert_eq!(keepalive.timed_out(KEEPALIVE_TIMEOUT), None);

    keepalive.last_inbound_at = Instant::now() - KEEPALIVE_TIMEOUT - Duration::from_secs(1);
    assert!(keepalive.timed_out(KEEPALIVE_TIMEOUT).is_some());
}

#[test]
fn dense_entity_movement_tracking_rotates_bounded_shards() {
    let entity_count = ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN * 10;
    let mut visits = vec![0; entity_count];

    for turn in 0..10 {
        let tick = turn * ENTITY_MOVE_SEND_INTERVAL_TICKS;
        let mut due = 0;
        for (ordinal, visits) in visits.iter_mut().enumerate() {
            if ordinary_entity_is_due_for_movement_tracking(
                ordinal,
                tick,
                entity_count,
                ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN,
            ) {
                *visits += 1;
                due += 1;
            }
        }
        assert_eq!(due, ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN);
    }
    assert!(visits.into_iter().all(|visits| visits == 1));
}

#[test]
fn movement_tracking_uses_the_runtime_publication_budget_without_gaps() {
    let publication_budget = ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN * 2;
    let entity_count = publication_budget * 4;
    let mut visits = vec![0; entity_count];

    for turn in 0..4 {
        let tick = turn * ENTITY_MOVE_SEND_INTERVAL_TICKS;
        let mut due = 0;
        for (ordinal, visits) in visits.iter_mut().enumerate() {
            if ordinary_entity_is_due_for_movement_tracking(
                ordinal,
                tick,
                entity_count,
                publication_budget,
            ) {
                *visits += 1;
                due += 1;
            }
        }
        assert_eq!(due, publication_budget);
    }
    assert!(visits.into_iter().all(|visits| visits == 1));
}

#[test]
fn dense_natural_movement_tracking_rotates_every_tick() {
    let entity_count = ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN * 10;
    let entities = (0..entity_count)
        .map(|id| EntityId(i32::try_from(id).unwrap()))
        .collect::<HashSet<_>>();
    let mut visits = vec![0; entity_count];

    for tick in 0..10 {
        let due = bounded_entity_ids_due_for_tick(
            &entities,
            tick,
            ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN,
        );
        assert_eq!(due.len(), ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN);
        for entity in due {
            visits[usize::try_from(entity.0).unwrap()] += 1;
        }
    }
    assert!(visits.into_iter().all(|visits| visits == 1));
}

#[test]
fn dense_entity_goal_updates_rotate_bounded_cohorts() {
    let entity_count = ENTITY_GOAL_UPDATES_PER_TICK * 10;
    let entities = (0..entity_count)
        .map(|id| EntityId(i32::try_from(id).unwrap()))
        .collect::<HashSet<_>>();
    let mut visits = vec![0; entity_count];

    for tick in 0..10 {
        let due = entity_goal_ids_due_for_tick(&entities, tick, true);
        assert_eq!(due.len(), ENTITY_GOAL_UPDATES_PER_TICK);
        for entity in due {
            visits[usize::try_from(entity.0).unwrap()] += 1;
        }
    }
    assert!(visits.into_iter().all(|visits| visits == 1));
}

#[test]
fn dense_entity_simulation_cohorts_are_stratified_across_regions() {
    const REGION_COUNT: usize = 16;
    const ENTITIES_PER_REGION: usize = 2_500;
    const LIMIT: usize = 1_000;
    let entities = (0..REGION_COUNT * ENTITIES_PER_REGION)
        .map(|id| EntityId(i32::try_from(id).unwrap()))
        .collect::<HashSet<_>>();

    let due = bounded_entity_ids_due_for_tick(&entities, 17, LIMIT);
    let mut per_region = [0usize; REGION_COUNT];
    for entity in due {
        let region = usize::try_from(entity.0).unwrap() / ENTITIES_PER_REGION;
        per_region[region] += 1;
    }

    assert_eq!(per_region.iter().sum::<usize>(), LIMIT);
    assert!(per_region.iter().all(|count| (62..=63).contains(count)));
}

#[test]
fn ordinary_entity_goal_updates_keep_full_tick_cadence() {
    let entity_count = ENTITY_GOAL_UPDATES_PER_TICK + 88;
    let entities = (0..entity_count)
        .map(|id| EntityId(i32::try_from(id).unwrap()))
        .collect::<HashSet<_>>();

    assert_eq!(entity_goal_ids_due_for_tick(&entities, 7, false), entities);
}

#[test]
fn dense_entity_simulation_rotates_lane_sized_cohorts() {
    let limit = 512;
    let entity_count = limit * 10;
    let entities = (0..entity_count)
        .map(|id| EntityId(i32::try_from(id).unwrap()))
        .collect::<HashSet<_>>();
    let mut visits = vec![0; entity_count];

    for tick in 0..10 {
        let due = bounded_entity_ids_due_for_tick(&entities, tick, limit);
        assert_eq!(due.len(), limit);
        for entity in due {
            visits[usize::try_from(entity.0).unwrap()] += 1;
        }
    }
    assert!(visits.into_iter().all(|visits| visits == 1));
}

#[test]
fn only_vanilla_outside_slot_sentinel_can_drop_the_cursor() {
    let click = |slot_num| ServerboundContainerClick {
        container_id: 0,
        state_id: 1,
        slot_num,
        button_num: 0,
        container_input: ContainerInput::Pickup,
        changed_slots: Vec::new(),
        carried_item: mc_protocol::packets::play::HashedStack::empty(),
    };

    assert!(matches!(
        classify_container_click(&click(-999)),
        ContainerClickAction::OutsidePickup { button: 0 }
    ));
    for malformed in [-1, -2, i16::MIN] {
        assert!(matches!(
            classify_container_click(&click(malformed)),
            ContainerClickAction::Unsupported
        ));
    }
}

#[tokio::test]
async fn rejected_inventory_drag_resyncs_without_mutation_or_owner_publication() {
    let item = Identifier::parse("minecraft:dirt").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: item,
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(items);
    state.carried_item = ItemStack::new(10, 3);
    let before_inventory = state.inventory.clone();
    let before_carried = state.carried_item.clone();
    let mut writer = Vec::new();
    let carried = || mc_protocol::packets::play::HashedStack::Actual {
        item_id: 10,
        count: 3,
        components: mc_protocol::packets::play::HashedStackComponentHashes::empty(),
    };
    let script_player_id = ScriptPlayerId::new(state.session_id);
    let script_context = no_script_player_context(state.session_id);
    let xp = XpState::default();

    for (button_num, slot_num) in [(0, -999), (2, -999)] {
        handle_container_click(
            &mut state,
            &mut writer,
            ContainerClickContext {
                game_mode: GameMode::Survival,
                survival_state: SurvivalState::FULL,
                xp_state: &xp,
                player_pose: PlayerPose::new(0.5, 64.0, 0.5),
                script_events: None,
                scripts: None,
                script_player_id,
                script_context: script_context.clone(),
            },
            ServerboundContainerClick {
                container_id: 0,
                state_id: 1,
                slot_num,
                button_num,
                container_input: ContainerInput::QuickCraft,
                changed_slots: Vec::new(),
                carried_item: carried(),
            },
        )
        .await
        .unwrap();
    }

    assert_eq!(state.inventory.slots, before_inventory.slots);
    assert_eq!(state.carried_item, before_carried);
    assert_eq!(state.simulation.snapshot().depth, 0);
    let packets = decode_container_set_content_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].items, before_inventory.as_wire_list());
    assert_eq!(packets[0].carried_item, before_carried);
}

#[tokio::test]
async fn stale_inventory_drag_resyncs_exact_owner_state_without_loss_or_publication() {
    let item = Identifier::parse("minecraft:dirt").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: item,
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(items);
    state.carried_item = ItemStack::new(10, 3);
    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("StaleInventoryDrag"),
        name: "StaleInventoryDrag".to_owned(),
    };
    let (tx, mut outbound) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.carried_item = state.carried_item.clone();
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let sessions = Arc::clone(&state.sessions);
    let (simulation, mut owner) = simulation_channel();
    let simulation_probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);
    let mut writer = Vec::new();
    let xp = XpState::default();
    let carried = mc_protocol::packets::play::HashedStack::Actual {
        item_id: 10,
        count: 3,
        components: mc_protocol::packets::play::HashedStackComponentHashes::empty(),
    };
    let script_player_id = ScriptPlayerId::new(state.session_id);
    let script_context = no_script_player_context(state.session_id);

    for (button_num, slot_num) in [(0, -999), (1, 9)] {
        handle_container_click(
            &mut state,
            &mut writer,
            ContainerClickContext {
                game_mode: GameMode::Survival,
                survival_state: SurvivalState::FULL,
                xp_state: &xp,
                player_pose: pose,
                script_events: None,
                scripts: None,
                script_player_id,
                script_context: script_context.clone(),
            },
            ServerboundContainerClick {
                container_id: 0,
                state_id: 1,
                slot_num,
                button_num,
                container_input: ContainerInput::QuickCraft,
                changed_slots: Vec::new(),
                carried_item: carried.clone(),
            },
        )
        .await
        .unwrap();
    }
    assert!(writer.is_empty());

    let mut end = Box::pin(handle_container_click(
        &mut state,
        &mut writer,
        ContainerClickContext {
            game_mode: GameMode::Survival,
            survival_state: SurvivalState::FULL,
            xp_state: &xp,
            player_pose: pose,
            script_events: None,
            scripts: None,
            script_player_id,
            script_context,
        },
        ServerboundContainerClick {
            container_id: 0,
            state_id: 1,
            slot_num: -999,
            button_num: 2,
            container_input: ContainerInput::QuickCraft,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    ));
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(end.as_mut(), cx).is_pending(),
            "drag must wait for its queued owner commit"
        );
        assert_eq!(simulation_probe.snapshot().depth, 1);
        Poll::Ready(())
    })
    .await;
    {
        let mut saved = saved.lock().unwrap();
        saved.inventory.slots[10] = ItemStack::new(10, 1);
        saved.carried_item = ItemStack::new(10, 2);
    }
    assert_eq!(owner.process_tick(&sessions, 1).processed, 1);
    end.await.unwrap();

    assert!(state.inventory.slots[9].is_empty());
    assert_eq!(state.inventory.slots[10], ItemStack::new(10, 1));
    assert_eq!(state.carried_item, ItemStack::new(10, 2));
    let total = state
        .inventory
        .slots
        .iter()
        .map(|stack| stack.count.max(0))
        .sum::<i32>()
        + state.carried_item.count;
    assert_eq!(total, 3);
    let packets = decode_container_set_content_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].state_id, 1);
    assert_eq!(packets[0].items, state.inventory.as_wire_list());
    assert_eq!(packets[0].carried_item, state.carried_item);
    assert!(outbound.try_recv().is_err());
    assert!(state.sessions.persisted_entity_records().is_empty());
}

#[test]
fn enchanting_selection_consumes_lapis_and_level_but_preserves_total_xp() {
    let items = mc_data::items::solaris_required_items();
    let item_facts = mc_data::item_components::solaris_required_item_facts();
    let pickaxe = items
        .id_of(&Identifier::parse("minecraft:stone_pickaxe").unwrap())
        .unwrap();
    let lapis = items
        .id_of(&Identifier::parse("minecraft:lapis_lazuli").unwrap())
        .unwrap();
    let pickaxe_name = items.name_of(pickaxe).expect("pickaxe registry name");
    assert!(item_is_efficiency_enchantable(&item_facts, pickaxe_name));
    let mut inputs = [ItemStack::new(pickaxe, 1), ItemStack::new(lapis, 2)];
    let mut xp = XpState {
        level: 1,
        progress: 0.5,
        total: 12,
        seed: 123,
    };

    assert!(enchant_item_candidate(
        &items,
        &item_facts,
        &mut inputs,
        &mut xp,
        enchanting_offer(0, 0).expect("base offer"),
    ));

    assert_eq!(
        inputs[0].enchantments,
        vec![mc_data::ItemEnchantment {
            id: Identifier::parse("minecraft:efficiency").unwrap(),
            level: 1,
        }]
    );
    assert_eq!(inputs[1], ItemStack::new(lapis, 1));
    assert_eq!(xp.level, 0);
    assert_eq!(xp.progress, 0.5);
    assert_eq!(xp.total, 12);
    assert_ne!(xp.seed, 123);
}

#[test]
fn enchanting_data_exposes_the_supported_efficiency_offer() {
    let items = mc_data::items::solaris_required_items();
    let item_facts = mc_data::item_components::solaris_required_item_facts();
    let pickaxe = items
        .id_of(&Identifier::parse("minecraft:stone_pickaxe").unwrap())
        .unwrap();
    let mut window = EnchantingTableWindow::at_position(7, mc_world::BlockPos { x: 0, y: 0, z: 0 });
    window.inputs[0] = ItemStack::new(pickaxe, 1);
    let xp = XpState {
        seed: 123,
        ..XpState::default()
    };

    assert_eq!(
        enchanting_data_values(&items, &item_facts, &window, &xp, 0),
        [
            (0, 1),
            (1, 0),
            (2, 0),
            (3, 123),
            (4, 8),
            (5, -1),
            (6, -1),
            (7, 1),
            (8, -1),
            (9, -1),
        ]
    );
}

#[test]
fn five_bookshelves_keep_efficiency_clue_and_add_fortune_to_pickaxes() {
    let items = mc_data::items::solaris_required_items();
    let item_facts = mc_data::item_components::solaris_required_item_facts();
    let pickaxe = items
        .id_of(&Identifier::parse("minecraft:stone_pickaxe").unwrap())
        .unwrap();
    let lapis = items
        .id_of(&Identifier::parse("minecraft:lapis_lazuli").unwrap())
        .unwrap();
    let fortune = Identifier::parse("minecraft:fortune").unwrap();
    let mut window = EnchantingTableWindow::at_position(7, mc_world::BlockPos { x: 0, y: 0, z: 0 });
    window.inputs[0] = ItemStack::new(pickaxe, 1);
    let mut xp = XpState {
        level: 10,
        progress: 0.25,
        total: 160,
        seed: 123,
    };

    let values = enchanting_data_values(&items, &item_facts, &window, &xp, 5);
    assert_eq!(values[1], (1, 10));
    assert_eq!(values[5], (5, 8));
    assert_eq!(values[8], (8, 2));

    let mut inputs = [ItemStack::new(pickaxe, 1), ItemStack::new(lapis, 2)];
    assert!(enchant_item_candidate(
        &items,
        &item_facts,
        &mut inputs,
        &mut xp,
        enchanting_offer(5, 1).expect("five-bookshelf offer"),
    ));
    assert_eq!(
        inputs[0].enchantments,
        vec![
            mc_data::ItemEnchantment {
                id: Identifier::parse("minecraft:efficiency").unwrap(),
                level: 2,
            },
            mc_data::ItemEnchantment {
                id: fortune,
                level: 2,
            },
        ]
    );
    assert!(inputs[1].is_empty());
    assert_eq!(xp.level, 8);
    assert_eq!(xp.progress, 0.25);
    assert_eq!(xp.total, 160);
    assert_ne!(xp.seed, 123);
}

#[test]
fn fifteen_bookshelves_keep_efficiency_clue_and_add_silk_touch_to_pickaxes() {
    let items = mc_data::items::solaris_required_items();
    let item_facts = mc_data::item_components::solaris_required_item_facts();
    let pickaxe = items
        .id_of(&Identifier::parse("minecraft:stone_pickaxe").unwrap())
        .unwrap();
    let lapis = items
        .id_of(&Identifier::parse("minecraft:lapis_lazuli").unwrap())
        .unwrap();
    let silk_touch = Identifier::parse("minecraft:silk_touch").unwrap();
    let mut window = EnchantingTableWindow::at_position(7, mc_world::BlockPos { x: 0, y: 0, z: 0 });
    window.inputs[0] = ItemStack::new(pickaxe, 1);
    let mut xp = XpState {
        level: 30,
        progress: 0.25,
        total: 1_395,
        seed: 123,
    };

    let values = enchanting_data_values(&items, &item_facts, &window, &xp, 15);
    assert_eq!(values[2], (2, 30));
    assert_eq!(values[6], (6, 8));
    assert_eq!(values[9], (9, 3));

    let mut inputs = [ItemStack::new(pickaxe, 1), ItemStack::new(lapis, 3)];
    assert!(enchant_item_candidate(
        &items,
        &item_facts,
        &mut inputs,
        &mut xp,
        enchanting_offer(15, 2).expect("fifteen-bookshelf offer"),
    ));
    assert_eq!(
        inputs[0].enchantments,
        vec![
            mc_data::ItemEnchantment {
                id: Identifier::parse("minecraft:efficiency").unwrap(),
                level: 3,
            },
            mc_data::ItemEnchantment {
                id: silk_touch,
                level: 1,
            },
        ]
    );
    assert!(inputs[1].is_empty());
    assert_eq!(xp.level, 27);
    assert_eq!(xp.progress, 0.25);
    assert_eq!(xp.total, 1_395);
    assert_ne!(xp.seed, 123);
}

#[test]
fn fifteen_bookshelves_keep_efficiency_as_the_first_pickaxe_offer() {
    let items = mc_data::items::solaris_required_items();
    let item_facts = mc_data::item_components::solaris_required_item_facts();
    let pickaxe = items
        .id_of(&Identifier::parse("minecraft:stone_pickaxe").unwrap())
        .unwrap();
    let lapis = items
        .id_of(&Identifier::parse("minecraft:lapis_lazuli").unwrap())
        .unwrap();
    let mut window = EnchantingTableWindow::at_position(7, mc_world::BlockPos { x: 0, y: 0, z: 0 });
    window.inputs[0] = ItemStack::new(pickaxe, 1);
    let mut xp = XpState {
        level: 30,
        progress: 0.25,
        total: 1_395,
        seed: 123,
    };

    assert_eq!(
        enchanting_data_values(&items, &item_facts, &window, &xp, 15),
        [
            (0, 1),
            (1, 10),
            (2, 30),
            (3, 123),
            (4, 8),
            (5, 8),
            (6, 8),
            (7, 1),
            (8, 2),
            (9, 3),
        ]
    );

    let mut inputs = [ItemStack::new(pickaxe, 1), ItemStack::new(lapis, 3)];
    assert!(enchant_item_candidate(
        &items,
        &item_facts,
        &mut inputs,
        &mut xp,
        enchanting_offer(15, 0).expect("first offer"),
    ));
    assert_eq!(
        inputs[0].enchantments,
        vec![mc_data::ItemEnchantment {
            id: Identifier::parse("minecraft:efficiency").unwrap(),
            level: 1,
        }]
    );
    assert_eq!(inputs[1], ItemStack::new(lapis, 2));
    assert_eq!(xp.level, 29);
    assert_eq!(xp.progress, 0.25);
    assert_eq!(xp.total, 1_395);
}

#[test]
fn fifteen_bookshelves_expose_and_apply_sharpness_to_swords() {
    let items = mc_data::items::solaris_required_items();
    let item_facts = mc_data::item_components::solaris_required_item_facts();
    let sword = items
        .id_of(&Identifier::parse("minecraft:stone_sword").unwrap())
        .unwrap();
    let lapis = items
        .id_of(&Identifier::parse("minecraft:lapis_lazuli").unwrap())
        .unwrap();
    let sharpness = Identifier::parse("minecraft:sharpness").unwrap();
    let sharpness_clue = i16::try_from(
        mc_data::required_registry_entry_id("minecraft:enchantment", &sharpness)
            .expect("sharpness registry id"),
    )
    .unwrap();
    let mut window = EnchantingTableWindow::at_position(7, mc_world::BlockPos { x: 0, y: 0, z: 0 });
    window.inputs[0] = ItemStack::new(sword, 1);
    let mut xp = XpState {
        level: 30,
        progress: 0.25,
        total: 1_395,
        seed: 123,
    };

    assert_eq!(
        enchanting_data_values(&items, &item_facts, &window, &xp, 15),
        [
            (0, 1),
            (1, 10),
            (2, 30),
            (3, 123),
            (4, sharpness_clue),
            (5, sharpness_clue),
            (6, sharpness_clue),
            (7, 1),
            (8, 2),
            (9, 3),
        ]
    );

    let mut inputs = [ItemStack::new(sword, 1), ItemStack::new(lapis, 3)];
    assert!(enchant_item_candidate(
        &items,
        &item_facts,
        &mut inputs,
        &mut xp,
        enchanting_offer(15, 2).expect("fifteen-bookshelf offer"),
    ));
    assert_eq!(
        inputs[0].enchantments,
        vec![mc_data::ItemEnchantment {
            id: sharpness,
            level: 3,
        }]
    );
    assert!(inputs[1].is_empty());
    assert_eq!(xp.level, 27);
}

#[test]
fn fifteen_bookshelves_expose_and_apply_protection_to_armor() {
    let items = mc_data::items::solaris_required_items();
    let item_facts = mc_data::item_components::solaris_required_item_facts();
    let chestplate = items
        .id_of(&Identifier::parse("minecraft:iron_chestplate").unwrap())
        .unwrap();
    let lapis = items
        .id_of(&Identifier::parse("minecraft:lapis_lazuli").unwrap())
        .unwrap();
    let protection = Identifier::parse("minecraft:protection").unwrap();
    let protection_clue = i16::try_from(
        mc_data::required_registry_entry_id("minecraft:enchantment", &protection)
            .expect("protection registry id"),
    )
    .unwrap();
    let mut window = EnchantingTableWindow::at_position(7, mc_world::BlockPos { x: 0, y: 0, z: 0 });
    window.inputs[0] = ItemStack::new(chestplate, 1);
    let mut xp = XpState {
        level: 30,
        progress: 0.25,
        total: 1_395,
        seed: 123,
    };

    assert_eq!(
        enchanting_data_values(&items, &item_facts, &window, &xp, 15),
        [
            (0, 1),
            (1, 10),
            (2, 30),
            (3, 123),
            (4, protection_clue),
            (5, protection_clue),
            (6, protection_clue),
            (7, 1),
            (8, 2),
            (9, 3),
        ]
    );

    let mut inputs = [ItemStack::new(chestplate, 1), ItemStack::new(lapis, 3)];
    assert!(enchant_item_candidate(
        &items,
        &item_facts,
        &mut inputs,
        &mut xp,
        enchanting_offer(15, 2).expect("fifteen-bookshelf offer"),
    ));
    assert_eq!(
        inputs[0].enchantments,
        vec![mc_data::ItemEnchantment {
            id: protection,
            level: 3,
        }]
    );
    assert!(inputs[1].is_empty());
    assert_eq!(xp.level, 27);
}

#[test]
fn held_sharpness_uses_the_vanilla_26_1_2_damage_formula() {
    let items = Arc::new(mc_data::items::solaris_required_items());
    let sword = items
        .id_of(&Identifier::parse("minecraft:stone_sword").unwrap())
        .unwrap();
    let mut state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    state.items = Arc::clone(&items);
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(sword, 1)
        .with_enchantment(Identifier::parse("minecraft:sharpness").unwrap(), 3);

    assert_eq!(
        attack_damage_for_item(&state.item_facts, &state.items, Some(sword)),
        5.0
    );
    assert_eq!(
        held_attack_damage(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
        ),
        7.0
    );
}

#[test]
fn enchanting_bookshelf_geometry_requires_clear_midpoints_and_caps_at_fifteen() {
    let table = mc_world::BlockPos {
        x: 10,
        y: 64,
        z: 20,
    };
    let first = mc_world::BlockPos {
        x: 12,
        y: 64,
        z: 20,
    };
    let second = mc_world::BlockPos { x: 8, y: 65, z: 21 };
    let first_gap = mc_world::BlockPos {
        x: 11,
        y: 64,
        z: 20,
    };
    let second_gap = mc_world::BlockPos { x: 9, y: 65, z: 20 };
    let providers = HashSet::from([first, second]);
    let clear_gaps = HashSet::from([first_gap, second_gap]);

    assert_eq!(
        count_valid_enchanting_bookshelves(
            table,
            |position| providers.contains(&position),
            |position| clear_gaps.contains(&position),
        ),
        2
    );
    assert_eq!(
        count_valid_enchanting_bookshelves(
            table,
            |position| providers.contains(&position),
            |position| position == first_gap,
        ),
        1
    );

    assert_eq!(
        count_valid_enchanting_bookshelves(table, |_| true, |_| true),
        15
    );
}

#[tokio::test]
async fn enchanting_button_commits_xp_through_owner_before_mutating_table_inputs() {
    let items = Arc::new(mc_data::items::solaris_required_items());
    let item_facts = Arc::new(mc_data::item_components::solaris_required_item_facts());
    let pickaxe = items
        .id_of(&Identifier::parse("minecraft:stone_pickaxe").unwrap())
        .unwrap();
    let lapis = items
        .id_of(&Identifier::parse("minecraft:lapis_lazuli").unwrap())
        .unwrap();
    let mut state = interaction_state_for_items(Arc::clone(&items));
    state.item_facts = item_facts;
    let (simulation, mut owner) = simulation_channel();

    let pose = PlayerPose::new(0.5, 65.0, 0.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("EnchantingOwner"),
        name: "EnchantingOwner".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    state.session_id = session_id;
    state.simulation = simulation.for_session(session_id);
    let mut persisted = PlayerPersistedState::new_default(pose);
    persisted.xp = XpState {
        level: 1,
        progress: 0.0,
        total: 7,
        seed: 123,
    };
    let persisted = Arc::new(Mutex::new(persisted));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&persisted));

    let mut window = EnchantingTableWindow::at_position(7, mc_world::BlockPos { x: 0, y: 0, z: 0 });
    window.inputs = [ItemStack::new(pickaxe, 1), ItemStack::new(lapis, 1)];
    persisted.lock().unwrap().enchanting_table_input = Some(Box::new(window.inputs.clone()));
    state.active_container = Some(ActiveContainer::EnchantingTable(window));
    let sessions = Arc::clone(&state.sessions);
    let world = Arc::clone(&state.world);
    let mut survival = SurvivalState::FULL;
    let mut xp = persisted.lock().unwrap().xp.clone();
    let mut writer = Vec::new();
    let mut request = Box::pin(handle_container_button_click(
        &mut state,
        &mut writer,
        GameMode::Survival,
        &mut survival,
        &mut xp,
        pose,
        ServerboundContainerButtonClick {
            container_id: 7,
            button_id: 0,
        },
    ));
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(request.as_mut(), cx).is_pending(),
            "enchanting must wait for its queued owner commit"
        );
        Poll::Ready(())
    })
    .await;

    assert_eq!(
        owner
            .process_commands_with_world(&sessions, Some(&world), None, 1)
            .await
            .processed,
        1
    );
    request.await.unwrap();

    assert_eq!(xp.level, 0);
    assert_eq!(xp.total, 7);
    let active = match state.active_container.as_ref().unwrap() {
        ActiveContainer::EnchantingTable(window) => window,
        other => panic!("unexpected active container: {other:?}"),
    };
    assert_eq!(active.state_id, 2);
    assert!(active.inputs[1].is_empty());
    assert_eq!(
        active.inputs[0].enchantments[0].id.as_str(),
        "minecraft:efficiency"
    );
    let persisted = persisted.lock().unwrap();
    assert_eq!(persisted.xp, xp);
    assert_eq!(
        persisted.enchanting_table_input.as_deref(),
        Some(&active.inputs),
        "XP and enchanting inputs must commit in one owner turn"
    );
    assert!(!writer.is_empty());
}

#[tokio::test]
async fn play_loop_closes_session_when_outbound_write_stalls() {
    let (_client, mut reader) = tokio::io::duplex(64);
    let mut writer = AllowThenStallWriter::new(3);
    let mut buf = BytesMut::new();
    let sessions = Arc::new(SessionRegistry::new());
    let (simulation, _simulation_owner) = simulation_channel();
    let start_timeouts = sessions.pressure_snapshot().slow_client_write_timeouts;
    let config = play_loop_slow_client_test_config();
    let (outbound_tx, outbound_rx) = mpsc::channel(1);
    outbound_tx
        .try_send(OutboundCommand::AnimatePlayer { entity_id: 1 })
        .expect("queue outbound command");
    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let respawn = ClientboundRespawn {
        dimension_type_id: 0,
        dimension_name: Identifier::parse("minecraft:overworld").unwrap(),
        hashed_seed: 0,
        game_mode: GameMode::Survival.id() as u8,
        previous_game_mode: -1,
        is_debug: false,
        is_flat: false,
        death_location: None,
        portal_cooldown: 0,
        sea_level: DEFAULT_SEA_LEVEL,
        data_to_keep: 0,
    };

    let result = tokio::time::timeout(
        Duration::from_millis(250),
        play_loop(
            &mut reader,
            &mut writer,
            &mut buf,
            Compression::Disabled,
            None,
            None,
            None,
            ChunkPipelineResources::with_limits(1, 1),
            Arc::clone(&sessions),
            simulation.for_session(1),
            &config,
            1,
            false,
            pose,
            pose,
            respawn,
            CommandPermissions::from_op(false),
            SurvivalState::FULL,
            XpState::default(),
            GameMode::Survival,
            outbound_rx,
            0,
            "SlowWriter".to_string(),
            "SlowWriter".to_string(),
            None,
            PlayerId::new(0),
            None,
            None,
        ),
    )
    .await
    .expect("slow outbound write should be bounded by play-loop timeout");

    result.expect("slow outbound writer should close session cleanly");
    assert_eq!(
        sessions.pressure_snapshot().slow_client_write_timeouts,
        start_timeouts + 1
    );
}

#[tokio::test]
async fn play_loop_closes_session_when_direct_response_write_stalls() {
    let (mut client, mut reader) = tokio::io::duplex(256);
    let request = ServerboundCommandSuggestion {
        id: 7,
        command: "/".to_string(),
    };
    let mut body = BytesMut::new();
    request.encode(&mut body).unwrap();
    let framed = encode_frame(
        ServerboundCommandSuggestion::ID,
        &body,
        Compression::Disabled,
    )
    .unwrap();
    client.write_all(&framed).await.unwrap();

    let mut writer = AllowThenStallWriter::new(3);
    let mut buf = BytesMut::new();
    let sessions = Arc::new(SessionRegistry::new());
    let (simulation, _simulation_owner) = simulation_channel();
    let start_timeouts = sessions.pressure_snapshot().slow_client_write_timeouts;
    let config = play_loop_slow_client_test_config();
    let (_outbound_tx, outbound_rx) = mpsc::channel(1);
    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let respawn = ClientboundRespawn {
        dimension_type_id: 0,
        dimension_name: Identifier::parse("minecraft:overworld").unwrap(),
        hashed_seed: 0,
        game_mode: GameMode::Survival.id() as u8,
        previous_game_mode: -1,
        is_debug: false,
        is_flat: false,
        death_location: None,
        portal_cooldown: 0,
        sea_level: DEFAULT_SEA_LEVEL,
        data_to_keep: 0,
    };

    let result = tokio::time::timeout(
        Duration::from_millis(250),
        play_loop(
            &mut reader,
            &mut writer,
            &mut buf,
            Compression::Disabled,
            None,
            None,
            None,
            ChunkPipelineResources::with_limits(1, 1),
            Arc::clone(&sessions),
            simulation.for_session(1),
            &config,
            1,
            false,
            pose,
            pose,
            respawn,
            CommandPermissions::from_op(false),
            SurvivalState::FULL,
            XpState::default(),
            GameMode::Survival,
            outbound_rx,
            0,
            "DirectWriter".to_string(),
            "DirectWriter".to_string(),
            None,
            PlayerId::new(0),
            None,
            None,
        ),
    )
    .await
    .expect("direct response write must be bounded by the packet writer");

    result.expect("direct response timeout should close the session cleanly");
    assert_eq!(
        sessions.pressure_snapshot().slow_client_write_timeouts,
        start_timeouts + 1
    );
}

#[tokio::test]
async fn play_loop_exits_when_outbound_channel_closes() {
    let (_client, mut reader) = tokio::io::duplex(64);
    let mut writer = tokio::io::sink();
    let mut buf = BytesMut::new();
    let sessions = Arc::new(SessionRegistry::new());
    let (simulation, _simulation_owner) = simulation_channel();
    let config = play_loop_slow_client_test_config();
    let (outbound_tx, outbound_rx) = mpsc::channel(1);
    drop(outbound_tx);
    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let respawn = ClientboundRespawn {
        dimension_type_id: 0,
        dimension_name: Identifier::parse("minecraft:overworld").unwrap(),
        hashed_seed: 0,
        game_mode: GameMode::Survival.id() as u8,
        previous_game_mode: -1,
        is_debug: false,
        is_flat: false,
        death_location: None,
        portal_cooldown: 0,
        sea_level: DEFAULT_SEA_LEVEL,
        data_to_keep: 0,
    };

    let result = tokio::time::timeout(
        Duration::from_millis(250),
        play_loop(
            &mut reader,
            &mut writer,
            &mut buf,
            Compression::Disabled,
            None,
            None,
            None,
            ChunkPipelineResources::with_limits(1, 1),
            sessions,
            simulation.for_session(1),
            &config,
            1,
            false,
            pose,
            pose,
            respawn,
            CommandPermissions::from_op(false),
            SurvivalState::FULL,
            XpState::default(),
            GameMode::Survival,
            outbound_rx,
            0,
            "ClosedOutbound".to_string(),
            "ClosedOutbound".to_string(),
            None,
            PlayerId::new(0),
            None,
            None,
        ),
    )
    .await
    .expect("closed outbound channel must wake and terminate play loop");

    result.expect("closed outbound channel should close session cleanly");
}

#[tokio::test]
async fn play_loop_drains_bounded_outbound_pressure_without_shedding() {
    let (_client, mut reader) = tokio::io::duplex(64);
    let mut writer = tokio::io::sink();
    let mut buf = BytesMut::new();
    let sessions = Arc::new(SessionRegistry::new());
    let (simulation, _simulation_owner) = simulation_channel();
    let start = sessions.pressure_snapshot();
    let config = play_loop_slow_client_test_config();
    let (outbound_tx, outbound_rx) = mpsc::channel(16);
    for entity_id in 1..=16 {
        outbound_tx
            .try_send(OutboundCommand::AnimatePlayer { entity_id })
            .expect("prefill outbound pressure queue");
    }
    let producer = tokio::spawn(async move {
        for entity_id in 17..=80 {
            outbound_tx
                .send(OutboundCommand::AnimatePlayer { entity_id })
                .await
                .expect("play loop remains available while draining");
        }
    });
    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let respawn = ClientboundRespawn {
        dimension_type_id: 0,
        dimension_name: Identifier::parse("minecraft:overworld").unwrap(),
        hashed_seed: 0,
        game_mode: GameMode::Survival.id() as u8,
        previous_game_mode: -1,
        is_debug: false,
        is_flat: false,
        death_location: None,
        portal_cooldown: 0,
        sea_level: DEFAULT_SEA_LEVEL,
        data_to_keep: 0,
    };

    let result = tokio::time::timeout(
        Duration::from_millis(250),
        play_loop(
            &mut reader,
            &mut writer,
            &mut buf,
            Compression::Disabled,
            None,
            None,
            None,
            ChunkPipelineResources::with_limits(1, 1),
            Arc::clone(&sessions),
            simulation.for_session(1),
            &config,
            1,
            false,
            pose,
            pose,
            respawn,
            CommandPermissions::from_op(false),
            SurvivalState::FULL,
            XpState::default(),
            GameMode::Survival,
            outbound_rx,
            0,
            "PressureWriter".to_string(),
            "PressureWriter".to_string(),
            None,
            PlayerId::new(0),
            None,
            None,
        ),
    )
    .await
    .expect("bounded outbound pressure should drain before the failure timeout");

    result.expect("closed outbound producer should close session cleanly");
    producer
        .await
        .expect("producer task should stop when receiver closes");
    let pressure = sessions.pressure_snapshot();
    assert_eq!(
        pressure.slow_client_write_timeouts, start.slow_client_write_timeouts,
        "bounded queue pressure should not be reported as a socket write timeout"
    );
    assert_eq!(
        pressure.slow_client_pressure_sheds, start.slow_client_pressure_sheds,
        "bounded queue pressure must not disconnect a client while writes progress"
    );
}

#[test]
fn entity_movement_write_turn_preserves_order_across_the_budget_boundary() {
    let movements = (0..=ENTITY_MOVEMENTS_PER_WRITE_TURN)
        .map(|index| ServerEntityMove {
            id: EntityId(index as i32),
            position: Vec3::new(index as f64, 64.0, 0.0),
            wire_move: Some(crate::play::wire_entities::ServerEntityWireMove::Absolute {
                position: Vec3::new(index as f64, 64.0, 0.0),
            }),
            velocity: Vec3::ZERO,
            rotation: Rotation::ZERO,
            on_ground: true,
            send_velocity: false,
            send_head_rotation: false,
        })
        .collect();

    let (current, remaining) = take_entity_movement_write_turn(movements);

    assert_eq!(current.len(), ENTITY_MOVEMENTS_PER_WRITE_TURN);
    assert_eq!(
        current.first().map(|movement| movement.id),
        Some(EntityId(0))
    );
    assert_eq!(
        current.last().map(|movement| movement.id),
        Some(EntityId(ENTITY_MOVEMENTS_PER_WRITE_TURN as i32 - 1))
    );
    let remaining = remaining.expect("one movement remains after the write-turn budget");
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining[0].id,
        EntityId(ENTITY_MOVEMENTS_PER_WRITE_TURN as i32)
    );
}

#[test]
fn container_title_nbt_reports_oversized_text_instead_of_panicking() {
    let oversized = "x".repeat(usize::from(u16::MAX) + 1);

    let err = chest_menu_title_nbt(&oversized).expect_err("oversized NBT title should fail");

    assert!(matches!(err, mc_protocol::CodecError::Nbt(_)));
}

#[test]
fn teleport_id_allocator_advances_and_wraps_to_positive_ids() {
    let mut next = 2;

    assert_eq!(next_player_teleport_id(&mut next), 2);
    assert_eq!(next_player_teleport_id(&mut next), 3);
    assert_eq!(next, 4);

    next = i32::MAX;
    assert_eq!(next_player_teleport_id(&mut next), i32::MAX);
    assert_eq!(next_player_teleport_id(&mut next), 1);
}

#[test]
fn pending_teleport_confirm_clears_only_matching_id() {
    let mut pending = Some(PendingTeleport::new(7, 0));

    assert_eq!(
        confirm_pending_teleport(&mut pending, 8),
        TeleportConfirmResult::Mismatched { expected: 7 }
    );
    assert!(pending.is_some());

    assert_eq!(
        confirm_pending_teleport(&mut pending, 7),
        TeleportConfirmResult::Confirmed
    );
    assert!(pending.is_none());
}

#[test]
fn pending_teleport_reports_unexpected_confirm_without_pending_state() {
    let mut pending = None;

    assert_eq!(
        confirm_pending_teleport(&mut pending, 1),
        TeleportConfirmResult::Unexpected
    );
    assert!(pending.is_none());
}

#[test]
fn pending_teleport_movement_gate_waits_without_duplicate_sync_packets() {
    let pending = Some(PendingTeleport::new(12, 0));

    for _ in 0..4 {
        assert!(guard_pending_teleport_movement(
            &pending,
            "ServerboundMovePlayerPos"
        ));
    }

    assert!(matches!(pending, Some(PendingTeleport { id: 12, .. })));
}

#[test]
fn pending_teleport_confirm_behaviour_after_unconfirmed_movement() {
    let mut pending = Some(PendingTeleport::new(7, 0));

    assert!(guard_pending_teleport_movement(
        &pending,
        "ServerboundMovePlayerPos"
    ));

    assert_eq!(
        confirm_pending_teleport(&mut pending, 8),
        TeleportConfirmResult::Mismatched { expected: 7 }
    );
    assert_eq!(pending.unwrap().id, 7);

    assert_eq!(
        confirm_pending_teleport(&mut pending, 7),
        TeleportConfirmResult::Confirmed
    );
    assert!(pending.is_none());
    assert!(!guard_pending_teleport_movement(
        &pending,
        "ServerboundMovePlayerPos"
    ));
}

#[tokio::test]
async fn pending_teleport_resends_after_vanilla_tick_window() {
    let pose = PlayerPose::new(12.5, 70.0, -3.25);
    let mut writer = Vec::new();
    let mut next_teleport_id = 8;
    let mut pending = Some(PendingTeleport::new(7, 100));

    assert!(
        !resend_pending_teleport_if_due(
            &mut writer,
            Compression::Disabled,
            &mut pending,
            &mut next_teleport_id,
            pose,
            120,
        )
        .await
        .unwrap()
    );
    assert!(writer.is_empty());

    assert!(
        resend_pending_teleport_if_due(
            &mut writer,
            Compression::Disabled,
            &mut pending,
            &mut next_teleport_id,
            pose,
            121,
        )
        .await
        .unwrap()
    );
    let packets = decode_player_position_sync_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].teleport_id, 8);
    assert_eq!(packets[0].x, pose.x);
    assert_eq!(packets[0].y, pose.y);
    assert_eq!(packets[0].z, pose.z);
    assert!(matches!(
        pending,
        Some(PendingTeleport {
            id: 8,
            sent_tick: 121
        })
    ));
    assert_eq!(next_teleport_id, 9);
}

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

#[test]
fn pending_teleport_movement_guard_returns_false_without_pending_teleport() {
    let pending = None;

    assert!(!guard_pending_teleport_movement(
        &pending,
        "ServerboundMovePlayerPos"
    ));
}

#[test]
fn stale_container_updates_do_not_discard_another_open_container() {
    let furnace_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let chest_pos = mc_world::BlockPos { x: 2, y: 64, z: 2 };
    let mut active = Some(ActiveContainer::Chest(ChestWindow::new(vec![chest_pos], 7)));

    assert!(active_furnace_window_at(&mut active, furnace_pos).is_none());
    assert!(matches!(
        active,
        Some(ActiveContainer::Chest(ChestWindow {
            container_id: 7,
            ..
        }))
    ));

    active = Some(ActiveContainer::Furnace(FurnaceWindow::new(
        furnace_pos,
        8,
        FurnaceKind::Furnace,
    )));
    assert!(active_chest_window_at(&mut active, chest_pos).is_none());
    assert!(matches!(
        active,
        Some(ActiveContainer::Furnace(FurnaceWindow {
            container_id: 8,
            ..
        }))
    ));
}

fn state(id: u32, default: bool, properties: &[(&str, &str)]) -> BlockStateReport {
    BlockStateReport {
        id,
        default,
        properties: props(properties),
    }
}

fn simple_block(id: u32, name: &str) -> BlockReport {
    BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: BTreeMap::new(),
        states: vec![state(id, true, &[])],
    }
}

fn furnace_block(unlit_id: u32, lit_id: u32) -> BlockReport {
    BlockReport {
        id: Identifier::parse("minecraft:furnace").unwrap(),
        properties: prop_schema(&[("facing", &["north"]), ("lit", &["false", "true"])]),
        states: vec![
            state(unlit_id, true, &[("facing", "north"), ("lit", "false")]),
            state(lit_id, false, &[("facing", "north"), ("lit", "true")]),
        ],
    }
}

fn staged_sapling_block(stage_zero_id: u32, stage_one_id: u32, name: &str) -> BlockReport {
    BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: prop_schema(&[("stage", &["0", "1"])]),
        states: vec![
            state(stage_zero_id, true, &[("stage", "0")]),
            state(stage_one_id, false, &[("stage", "1")]),
        ],
    }
}

fn axis_log_block(id_base: u32, name: &str) -> BlockReport {
    BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: prop_schema(&[("axis", &["x", "y"])]),
        states: vec![
            state(id_base, true, &[("axis", "x")]),
            state(id_base + 1, false, &[("axis", "y")]),
        ],
    }
}

fn tree_leaves_block(id: u32, name: &str) -> BlockReport {
    BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: prop_schema(&[("distance", &["1"]), ("persistent", &["false"])]),
        states: vec![state(
            id,
            true,
            &[("distance", "1"), ("persistent", "false")],
        )],
    }
}

fn leaf_distance_test_reports() -> Vec<BlockReport> {
    let distance_values = ["1", "2", "3", "4", "5", "6", "7"];
    let leaves = BlockReport {
        id: Identifier::parse("minecraft:oak_leaves").unwrap(),
        properties: prop_schema(&[("distance", &distance_values), ("persistent", &["false"])]),
        states: distance_values
            .iter()
            .enumerate()
            .map(|(offset, distance)| {
                state(
                    offset as u32 + 2,
                    offset == 0,
                    &[("distance", distance), ("persistent", "false")],
                )
            })
            .collect(),
    };
    vec![
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:oak_log"),
        leaves,
    ]
}

fn leaf_distance_test_registry() -> Arc<mc_world::BlockRegistry> {
    Arc::new(mc_world::BlockRegistry::from_report(&leaf_distance_test_reports()).unwrap())
}

fn prop_schema(entries: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    entries
        .iter()
        .map(|(key, values)| {
            (
                (*key).to_string(),
                values.iter().map(|value| (*value).to_string()).collect(),
            )
        })
        .collect()
}

fn crop_test_reports() -> Vec<BlockReport> {
    let mut farmland_properties = BTreeMap::new();
    farmland_properties.insert(
        "moisture".to_string(),
        (0..=7).map(|value| value.to_string()).collect(),
    );
    let mut crop_properties = BTreeMap::new();
    crop_properties.insert(
        "age".to_string(),
        (0..=7).map(|value| value.to_string()).collect(),
    );

    let mut reports = vec![
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:dirt"),
        simple_block(2, "minecraft:water"),
        simple_block(19, "minecraft:soul_sand"),
        BlockReport {
            id: Identifier::parse("minecraft:farmland").unwrap(),
            properties: farmland_properties,
            states: (0..=7)
                .map(|moisture| {
                    state(
                        3 + moisture,
                        moisture == 0,
                        &[("moisture", &moisture.to_string())],
                    )
                })
                .collect(),
        },
        BlockReport {
            id: Identifier::parse("minecraft:wheat").unwrap(),
            properties: crop_properties.clone(),
            states: (0..=7)
                .map(|age| state(11 + age, age == 0, &[("age", &age.to_string())]))
                .collect(),
        },
        BlockReport {
            id: Identifier::parse("minecraft:carrots").unwrap(),
            properties: crop_properties.clone(),
            states: (0..=7)
                .map(|age| state(20 + age, age == 0, &[("age", &age.to_string())]))
                .collect(),
        },
        BlockReport {
            id: Identifier::parse("minecraft:potatoes").unwrap(),
            properties: crop_properties.clone(),
            states: (0..=7)
                .map(|age| state(28 + age, age == 0, &[("age", &age.to_string())]))
                .collect(),
        },
        BlockReport {
            id: Identifier::parse("minecraft:beetroots").unwrap(),
            properties: crop_properties.clone(),
            states: (0..=7)
                .map(|age| state(36 + age, age == 0, &[("age", &age.to_string())]))
                .collect(),
        },
        BlockReport {
            id: Identifier::parse("minecraft:nether_wart").unwrap(),
            properties: crop_properties,
            states: (0..=7)
                .map(|age| state(44 + age, age == 0, &[("age", &age.to_string())]))
                .collect(),
        },
        BlockReport {
            id: Identifier::parse("minecraft:pumpkin_stem").unwrap(),
            properties: prop_schema(&[("age", &["0", "1"])]),
            states: (0..=1)
                .map(|age| state(52 + age, age == 0, &[("age", &age.to_string())]))
                .collect(),
        },
        BlockReport {
            id: Identifier::parse("minecraft:melon_stem").unwrap(),
            properties: prop_schema(&[("age", &["0", "1"])]),
            states: (0..=1)
                .map(|age| state(54 + age, age == 0, &[("age", &age.to_string())]))
                .collect(),
        },
        BlockReport {
            id: Identifier::parse("minecraft:sweet_berry_bush").unwrap(),
            properties: prop_schema(&[("age", &["0", "1", "2", "3"])]),
            states: (0..=3)
                .map(|age| state(56 + age, age == 0, &[("age", &age.to_string())]))
                .collect(),
        },
        BlockReport {
            id: Identifier::parse("minecraft:cocoa").unwrap(),
            properties: prop_schema(&[("age", &["0", "1", "2"]), ("facing", &["north"])]),
            states: (0..=2)
                .map(|age| {
                    state(
                        60 + age,
                        age == 0,
                        &[("age", &age.to_string()), ("facing", "north")],
                    )
                })
                .collect(),
        },
        simple_block(63, "minecraft:melon"),
        simple_block(64, "minecraft:pumpkin"),
        attached_stem_block(65, "minecraft:attached_melon_stem"),
        attached_stem_block(69, "minecraft:attached_pumpkin_stem"),
        simple_block(73, "minecraft:jungle_log"),
    ];
    reports.sort_by_key(|block| block.states.first().map(|state| state.id).unwrap_or(0));
    reports
}

fn attached_stem_block(first_id: u32, name: &str) -> BlockReport {
    BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: prop_schema(&[("facing", &["north", "south", "west", "east"])]),
        states: ["north", "south", "west", "east"]
            .into_iter()
            .enumerate()
            .map(|(offset, facing)| {
                state(first_id + offset as u32, offset == 0, &[("facing", facing)])
            })
            .collect(),
    }
}

fn crop_test_registry() -> mc_world::BlockRegistry {
    mc_world::BlockRegistry::from_report(&crop_test_reports()).unwrap()
}

const VANILLA_26_1_2_TREE_REPLACEABLES: [&str; 53] = [
    "minecraft:acacia_leaves",
    "minecraft:allium",
    "minecraft:azalea_leaves",
    "minecraft:azure_bluet",
    "minecraft:birch_leaves",
    "minecraft:blue_orchid",
    "minecraft:bush",
    "minecraft:cherry_leaves",
    "minecraft:closed_eyeblossom",
    "minecraft:cornflower",
    "minecraft:crimson_roots",
    "minecraft:dandelion",
    "minecraft:dark_oak_leaves",
    "minecraft:dead_bush",
    "minecraft:fern",
    "minecraft:firefly_bush",
    "minecraft:flowering_azalea_leaves",
    "minecraft:glow_lichen",
    "minecraft:golden_dandelion",
    "minecraft:hanging_roots",
    "minecraft:jungle_leaves",
    "minecraft:large_fern",
    "minecraft:leaf_litter",
    "minecraft:lilac",
    "minecraft:lily_of_the_valley",
    "minecraft:mangrove_leaves",
    "minecraft:nether_sprouts",
    "minecraft:oak_leaves",
    "minecraft:open_eyeblossom",
    "minecraft:orange_tulip",
    "minecraft:oxeye_daisy",
    "minecraft:pale_moss_carpet",
    "minecraft:pale_oak_leaves",
    "minecraft:peony",
    "minecraft:pink_tulip",
    "minecraft:pitcher_plant",
    "minecraft:poppy",
    "minecraft:red_tulip",
    "minecraft:rose_bush",
    "minecraft:seagrass",
    "minecraft:short_dry_grass",
    "minecraft:short_grass",
    "minecraft:spruce_leaves",
    "minecraft:sunflower",
    "minecraft:tall_dry_grass",
    "minecraft:tall_grass",
    "minecraft:tall_seagrass",
    "minecraft:torchflower",
    "minecraft:vine",
    "minecraft:warped_roots",
    "minecraft:water",
    "minecraft:white_tulip",
    "minecraft:wither_rose",
];

fn sapling_tree_test_reports() -> Vec<BlockReport> {
    let mut reports = vec![
        simple_block(0, "minecraft:air"),
        staged_sapling_block(1, 27, "minecraft:oak_sapling"),
        axis_log_block(2, "minecraft:oak_log"),
        tree_leaves_block(4, "minecraft:oak_leaves"),
        simple_block(5, "minecraft:stone"),
        staged_sapling_block(6, 28, "minecraft:cherry_sapling"),
        staged_sapling_block(7, 29, "minecraft:birch_sapling"),
        axis_log_block(8, "minecraft:birch_log"),
        tree_leaves_block(10, "minecraft:birch_leaves"),
        staged_sapling_block(11, 30, "minecraft:spruce_sapling"),
        axis_log_block(12, "minecraft:spruce_log"),
        tree_leaves_block(14, "minecraft:spruce_leaves"),
        staged_sapling_block(15, 31, "minecraft:jungle_sapling"),
        axis_log_block(16, "minecraft:jungle_log"),
        tree_leaves_block(18, "minecraft:jungle_leaves"),
        staged_sapling_block(19, 32, "minecraft:acacia_sapling"),
        axis_log_block(20, "minecraft:acacia_log"),
        tree_leaves_block(22, "minecraft:acacia_leaves"),
        staged_sapling_block(23, 33, "minecraft:dark_oak_sapling"),
        axis_log_block(24, "minecraft:dark_oak_log"),
        tree_leaves_block(26, "minecraft:dark_oak_leaves"),
        simple_block(34, "minecraft:short_grass"),
        simple_block(35, "minecraft:vine"),
    ];
    let mut next_state_id = 36;
    for name in VANILLA_26_1_2_TREE_REPLACEABLES {
        if reports.iter().any(|report| report.id.as_str() == name) {
            continue;
        }
        reports.push(simple_block(next_state_id, name));
        next_state_id += 1;
    }
    reports
}

fn sapling_tree_test_registry() -> Arc<mc_world::BlockRegistry> {
    Arc::new(mc_world::BlockRegistry::from_report(&sapling_tree_test_reports()).unwrap())
}

fn in_memory_tree_world(registry: Arc<mc_world::BlockRegistry>) -> mc_world::WorldStorage {
    let mut world = mc_world::WorldStorage::in_memory(registry);
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    world
}

fn wheat_drop_items() -> ItemRegistry {
    ItemRegistry::from_report(&[
        ItemReport {
            id: Identifier::parse("minecraft:wheat").unwrap(),
            protocol_id: 50,
        },
        ItemReport {
            id: Identifier::parse("minecraft:wheat_seeds").unwrap(),
            protocol_id: 51,
        },
    ])
}

fn carrot_slice_drop_items() -> ItemRegistry {
    ItemRegistry::from_report(&[
        ItemReport {
            id: Identifier::parse("minecraft:wheat").unwrap(),
            protocol_id: 50,
        },
        ItemReport {
            id: Identifier::parse("minecraft:wheat_seeds").unwrap(),
            protocol_id: 51,
        },
        ItemReport {
            id: Identifier::parse("minecraft:carrot").unwrap(),
            protocol_id: 52,
        },
        ItemReport {
            id: Identifier::parse("minecraft:pumpkin_stem").unwrap(),
            protocol_id: 53,
        },
        ItemReport {
            id: Identifier::parse("minecraft:potato").unwrap(),
            protocol_id: 54,
        },
        ItemReport {
            id: Identifier::parse("minecraft:beetroot").unwrap(),
            protocol_id: 55,
        },
        ItemReport {
            id: Identifier::parse("minecraft:beetroot_seeds").unwrap(),
            protocol_id: 56,
        },
        ItemReport {
            id: Identifier::parse("minecraft:nether_wart").unwrap(),
            protocol_id: 57,
        },
    ])
}

fn test_crop_state_with_age(blocks: &mc_world::BlockRegistry, crop: &str, age: u8) -> BlockStateId {
    blocks
        .by_name_and_props(
            &Identifier::parse(crop).unwrap(),
            &[("age".to_string(), age.to_string())],
        )
        .unwrap()
}

#[test]
fn wheat_crop_drop_mature_returns_wheat_and_seeds() {
    let blocks = crop_test_registry();
    let items = wheat_drop_items();
    let wheat = test_crop_state_with_age(&blocks, "minecraft:wheat", 7);

    let drops = block_drop_stacks_from(
        &mc_data::loot::LootTables::default(),
        &items,
        &blocks,
        wheat,
    );

    assert_eq!(drops, vec![ItemStack::new(50, 1), ItemStack::new(51, 1)]);
}

#[test]
fn wheat_crop_drop_young_returns_seeds_only() {
    let blocks = crop_test_registry();
    let items = wheat_drop_items();
    let wheat = test_crop_state_with_age(&blocks, "minecraft:wheat", 3);

    let drops = block_drop_stacks_from(
        &mc_data::loot::LootTables::default(),
        &items,
        &blocks,
        wheat,
    );

    assert_eq!(drops, vec![ItemStack::new(51, 1)]);
}

#[test]
fn block_drop_generic_non_crop_fallback_still_returns_block_item() {
    let blocks = crop_test_registry();
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:dirt").unwrap(),
        protocol_id: 52,
    }]);
    let dirt = blocks
        .block(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap()
        .default;

    let drops =
        block_drop_stacks_from(&mc_data::loot::LootTables::default(), &items, &blocks, dirt);

    assert_eq!(drops, vec![ItemStack::new(52, 1)]);
}

#[test]
fn block_drop_builtin_short_grass_returns_wheat_seeds() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:short_grass"),
    ])
    .unwrap();
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:wheat_seeds").unwrap(),
        protocol_id: 51,
    }]);
    let short_grass = blocks
        .block(&Identifier::parse("minecraft:short_grass").unwrap())
        .unwrap()
        .default;

    let drops = block_drop_stacks_from(
        &mc_data::loot::LootTables::default(),
        &items,
        &blocks,
        short_grass,
    );

    assert_eq!(drops, vec![ItemStack::new(51, 1)]);
}

#[test]
fn block_drop_configured_loot_count_reaches_runtime_stack() {
    let blocks = crop_test_registry();
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:carrot").unwrap(),
        protocol_id: 52,
    }]);
    let dirt = blocks
        .block(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap()
        .default;
    let loot = mc_data::loot::LootTables::from_drop_maps(
        BTreeMap::new(),
        BTreeMap::from([(
            Identifier::parse("minecraft:dirt").unwrap(),
            mc_data::loot::LootDrop {
                item: Identifier::parse("minecraft:carrot").unwrap(),
                count: mc_data::loot::LootCount::Fixed(3),
            },
        )]),
    );

    let drops = block_drop_stacks_from(&loot, &items, &blocks, dirt);

    assert_eq!(drops, vec![ItemStack::new(52, 3)]);
}

#[test]
fn wheat_crop_drop_rejects_incomplete_logical_drop() {
    let blocks = crop_test_registry();
    let wheat = test_crop_state_with_age(&blocks, "minecraft:wheat", 7);
    let seeds_only = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:wheat_seeds").unwrap(),
        protocol_id: 51,
    }]);
    let missing_all = ItemRegistry::default();

    assert!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &seeds_only,
            &blocks,
            wheat,
        )
        .is_empty()
    );
    assert!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &missing_all,
            &blocks,
            wheat,
        )
        .is_empty()
    );
}

#[test]
fn carrot_crop_drop_mature_returns_two_carrots() {
    let blocks = crop_test_registry();
    let items = carrot_slice_drop_items();
    let carrots = test_crop_state_with_age(&blocks, "minecraft:carrots", 7);

    let drops = block_drop_stacks_from(
        &mc_data::loot::LootTables::default(),
        &items,
        &blocks,
        carrots,
    );

    assert_eq!(drops, vec![ItemStack::new(52, 2)]);
}

#[test]
fn carrot_crop_drop_immature_returns_one_carrot() {
    let blocks = crop_test_registry();
    let items = carrot_slice_drop_items();

    for age in 0..=6 {
        let carrots = test_crop_state_with_age(&blocks, "minecraft:carrots", age);

        let drops = block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &items,
            &blocks,
            carrots,
        );

        assert_eq!(drops, vec![ItemStack::new(52, 1)], "age {age}");
    }
}

#[test]
fn carrot_slice_preserves_wheat_crop_drop_behavior() {
    let blocks = crop_test_registry();
    let items = carrot_slice_drop_items();
    let mature_wheat = test_crop_state_with_age(&blocks, "minecraft:wheat", 7);
    let young_wheat = test_crop_state_with_age(&blocks, "minecraft:wheat", 3);

    assert_eq!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &items,
            &blocks,
            mature_wheat,
        ),
        vec![ItemStack::new(50, 1), ItemStack::new(51, 1)]
    );
    assert_eq!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &items,
            &blocks,
            young_wheat,
        ),
        vec![ItemStack::new(51, 1)]
    );
}

#[test]
fn carrot_slice_unsupported_crop_state_uses_generic_fallback() {
    let blocks = crop_test_registry();
    let items = carrot_slice_drop_items();
    let pumpkin_stem = test_crop_state_with_age(&blocks, "minecraft:pumpkin_stem", 1);

    let drops = block_drop_stacks_from(
        &mc_data::loot::LootTables::default(),
        &items,
        &blocks,
        pumpkin_stem,
    );

    assert_eq!(drops, vec![ItemStack::new(53, 1)]);
}

#[test]
fn carrot_crop_drop_rejects_missing_item_id() {
    let blocks = crop_test_registry();
    let carrots = test_crop_state_with_age(&blocks, "minecraft:carrots", 7);
    let missing_carrot = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:wheat").unwrap(),
        protocol_id: 50,
    }]);

    assert!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &missing_carrot,
            &blocks,
            carrots,
        )
        .is_empty()
    );
}

#[test]
fn potato_crop_drop_mature_returns_two_potatoes() {
    let blocks = crop_test_registry();
    let items = carrot_slice_drop_items();
    let potatoes = test_crop_state_with_age(&blocks, "minecraft:potatoes", 7);

    let drops = block_drop_stacks_from(
        &mc_data::loot::LootTables::default(),
        &items,
        &blocks,
        potatoes,
    );

    assert_eq!(drops, vec![ItemStack::new(54, 2)]);
}

#[test]
fn potato_crop_drop_immature_returns_one_potato() {
    let blocks = crop_test_registry();
    let items = carrot_slice_drop_items();

    for age in 0..=6 {
        let potatoes = test_crop_state_with_age(&blocks, "minecraft:potatoes", age);

        let drops = block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &items,
            &blocks,
            potatoes,
        );

        assert_eq!(drops, vec![ItemStack::new(54, 1)], "age {age}");
    }
}

#[test]
fn potato_crop_drop_rejects_missing_item_id() {
    let blocks = crop_test_registry();
    let potatoes = test_crop_state_with_age(&blocks, "minecraft:potatoes", 7);
    let missing_potato = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:carrot").unwrap(),
        protocol_id: 52,
    }]);

    assert!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &missing_potato,
            &blocks,
            potatoes,
        )
        .is_empty()
    );
}

#[test]
fn beetroot_crop_drop_mature_returns_beetroot_and_seeds() {
    let blocks = crop_test_registry();
    let items = carrot_slice_drop_items();
    let beetroots = test_crop_state_with_age(&blocks, "minecraft:beetroots", 3);

    let drops = block_drop_stacks_from(
        &mc_data::loot::LootTables::default(),
        &items,
        &blocks,
        beetroots,
    );

    assert_eq!(drops, vec![ItemStack::new(55, 1), ItemStack::new(56, 1)]);
}

#[test]
fn beetroot_crop_drop_immature_returns_seeds_only() {
    let blocks = crop_test_registry();
    let items = carrot_slice_drop_items();

    for age in 0..=2 {
        let beetroots = test_crop_state_with_age(&blocks, "minecraft:beetroots", age);

        let drops = block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &items,
            &blocks,
            beetroots,
        );

        assert_eq!(drops, vec![ItemStack::new(56, 1)], "age {age}");
    }
}

#[test]
fn beetroot_crop_drop_rejects_incomplete_logical_drop() {
    let blocks = crop_test_registry();
    let beetroots = test_crop_state_with_age(&blocks, "minecraft:beetroots", 3);
    let seeds_only = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:beetroot_seeds").unwrap(),
        protocol_id: 56,
    }]);
    let missing_all = ItemRegistry::default();

    assert!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &seeds_only,
            &blocks,
            beetroots,
        )
        .is_empty()
    );
    assert!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &missing_all,
            &blocks,
            beetroots,
        )
        .is_empty()
    );
}

#[test]
fn nether_wart_crop_drop_mature_returns_two_warts() {
    let blocks = crop_test_registry();
    let items = carrot_slice_drop_items();
    let nether_wart = test_crop_state_with_age(&blocks, "minecraft:nether_wart", 3);

    let drops = block_drop_stacks_from(
        &mc_data::loot::LootTables::default(),
        &items,
        &blocks,
        nether_wart,
    );

    assert_eq!(drops, vec![ItemStack::new(57, 2)]);
}

#[test]
fn nether_wart_crop_drop_immature_returns_one_wart() {
    let blocks = crop_test_registry();
    let items = carrot_slice_drop_items();

    for age in 0..=2 {
        let nether_wart = test_crop_state_with_age(&blocks, "minecraft:nether_wart", age);

        let drops = block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &items,
            &blocks,
            nether_wart,
        );

        assert_eq!(drops, vec![ItemStack::new(57, 1)], "age {age}");
    }
}

#[test]
fn nether_wart_crop_drop_rejects_missing_item_id() {
    let blocks = crop_test_registry();
    let nether_wart = test_crop_state_with_age(&blocks, "minecraft:nether_wart", 3);
    let missing_wart = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:beetroot").unwrap(),
        protocol_id: 55,
    }]);

    assert!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &missing_wart,
            &blocks,
            nether_wart,
        )
        .is_empty()
    );
}

#[test]
fn cocoa_crop_drop_mature_returns_three_beans() {
    let blocks = crop_test_registry();
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:cocoa_beans").unwrap(),
        protocol_id: 58,
    }]);

    let drops = block_drop_stacks_from(
        &mc_data::loot::LootTables::default(),
        &items,
        &blocks,
        mc_world::BlockStateId(62),
    );

    assert_eq!(drops, vec![ItemStack::new(58, 3)]);
}

#[test]
fn cocoa_crop_drop_immature_returns_one_bean() {
    let blocks = crop_test_registry();
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:cocoa_beans").unwrap(),
        protocol_id: 58,
    }]);

    for state in [mc_world::BlockStateId(60), mc_world::BlockStateId(61)] {
        let drops = block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &items,
            &blocks,
            state,
        );

        assert_eq!(drops, vec![ItemStack::new(58, 1)], "state {state:?}");
    }
}

#[test]
fn cocoa_crop_drop_rejects_missing_item_id() {
    let blocks = crop_test_registry();
    let missing_beans = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:beetroot").unwrap(),
        protocol_id: 55,
    }]);

    assert!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &missing_beans,
            &blocks,
            mc_world::BlockStateId(62),
        )
        .is_empty()
    );
}

fn fluid_block(first_id: u32, name: &str, max_level: u8) -> BlockReport {
    let mut properties = BTreeMap::new();
    properties.insert(
        "level".to_string(),
        (0..=max_level).map(|level| level.to_string()).collect(),
    );
    BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties,
        states: (0..=max_level)
            .map(|level| {
                state(
                    first_id + u32::from(level),
                    level == 0,
                    &[("level", &level.to_string())],
                )
            })
            .collect(),
    }
}

fn fluid_test_reports() -> Vec<BlockReport> {
    vec![
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:stone"),
        fluid_block(2, "minecraft:water", 7),
        fluid_block(10, "minecraft:lava", 3),
        simple_block(14, "minecraft:obsidian"),
        simple_block(15, "minecraft:cobblestone"),
        simple_block(16, "minecraft:sand"),
        simple_block(17, "minecraft:gravel"),
        simple_block(18, "minecraft:anvil"),
        simple_block(19, "minecraft:cactus"),
        simple_block(20, "minecraft:bamboo"),
        simple_block(21, "minecraft:sugar_cane"),
    ]
}

pub(super) fn fluid_test_registry() -> mc_world::BlockRegistry {
    mc_world::BlockRegistry::from_report(&fluid_test_reports()).unwrap()
}

fn bamboo_test_registry() -> mc_world::BlockRegistry {
    let mut bamboo_states = Vec::new();
    let mut next_id = 3;
    for age in ["0", "1"] {
        for leaves in ["none", "small", "large"] {
            for stage in ["0", "1"] {
                bamboo_states.push(state(
                    next_id,
                    age == "0" && leaves == "none" && stage == "0",
                    &[("age", age), ("leaves", leaves), ("stage", stage)],
                ));
                next_id += 1;
            }
        }
    }
    mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:sand"),
        simple_block(2, "minecraft:bamboo_sapling"),
        BlockReport {
            id: Identifier::parse("minecraft:bamboo").unwrap(),
            properties: prop_schema(&[
                ("age", &["0", "1"]),
                ("leaves", &["none", "small", "large"]),
                ("stage", &["0", "1"]),
            ]),
            states: bamboo_states,
        },
    ])
    .unwrap()
}

fn bamboo_state(
    blocks: &mc_world::BlockRegistry,
    age: &str,
    leaves: &str,
    stage: &str,
) -> BlockStateId {
    blocks
        .by_name_and_props(
            &Identifier::parse("minecraft:bamboo").unwrap(),
            &[
                ("age".into(), age.into()),
                ("leaves".into(), leaves.into()),
                ("stage".into(), stage.into()),
            ],
        )
        .unwrap()
}

pub(super) fn fluid_test_facts() -> mc_data::block_facts::BlockFactsTable {
    mc_data::block_facts::BlockFactsTable::from_blocks_report(&fluid_test_reports())
}

pub(super) fn interaction_state_for_blocks(
    blocks: Arc<mc_world::BlockRegistry>,
) -> InteractionState {
    let items = Arc::new(ItemRegistry::from_report(&[]));
    let storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let world_read = storage.read_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let item_to_block = ItemToBlockTable::build(&items, &blocks);
    InteractionState {
        world,
        world_read,
        blocks,
        block_light: None,
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        water: None,
        sessions: Arc::new(SessionRegistry::new()),
        simulation: simulation_channel().0,
        session_id: 1,
        workspace: LightWorkspace::new(),
        light_cache: LightCache::new(),
        compression: Compression::Disabled,
        selected_hotbar_slot: 0,
        inventory: PlayerInventory::empty(),
        carried_item: ItemStack::EMPTY,
        player_persistence: Arc::new(Mutex::new(PlayerPersistedState::new_default(
            PlayerPose::new(0.5, 64.0, 0.5),
        ))),
        inventory_state_id: 1,
        inventory_quickcraft: QuickCraftState::default(),
        items,
        item_facts: Arc::new(ItemFactsTable::default()),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        item_to_block,
        tags: Arc::new(TagsData::default()),
        recipes: Vec::new(),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        script_zones: None,
        next_container_id: FURNACE_CONTAINER_ID_MIN,
        active_container: None,
        pending_break: None,
        delayed_break: None,
        pending_use: None,
        pending_sign_edit: None,
        shield_use: None,
        last_entity_attack_tick: None,
    }
}

pub(super) async fn insert_fluid_test_chunk(state: &InteractionState) {
    let cpos = ChunkPos { x: 0, z: 0 };
    state
        .world
        .lock()
        .await
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
}

#[tokio::test]
async fn collision_correction_does_not_teleport_back_into_existing_solid_overlap() {
    let state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    insert_fluid_test_chunk(&state).await;
    state
        .world
        .lock()
        .await
        .set_block_at(mc_world::BlockPos { x: 0, y: 64, z: 0 }, BlockStateId(1))
        .unwrap();
    let old_pose = PlayerPose::new(0.50, 64.0, 0.50);
    let new_pose = PlayerPose::new(0.55, 64.0, 0.50);
    let mut writer = Vec::new();
    let mut next_teleport_id = 2;
    let mut pending_teleport = None;

    let corrected = correct_player_collision(
        Some(&state),
        &mut writer,
        Compression::Disabled,
        old_pose,
        new_pose,
        0,
        &mut next_teleport_id,
        &mut pending_teleport,
    )
    .await
    .unwrap();

    assert!(
        !corrected,
        "an already-colliding authoritative pose must be allowed to escape"
    );
    assert!(writer.is_empty());
    assert!(pending_teleport.is_none());
    assert_eq!(next_teleport_id, 2);
}

#[tokio::test]
async fn collision_correction_still_rejects_entry_from_free_space_into_solid() {
    let state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    insert_fluid_test_chunk(&state).await;
    state
        .world
        .lock()
        .await
        .set_block_at(mc_world::BlockPos { x: 0, y: 64, z: 0 }, BlockStateId(1))
        .unwrap();
    let old_pose = PlayerPose::new(1.50, 64.0, 0.50);
    let new_pose = PlayerPose::new(0.50, 64.0, 0.50);
    let mut writer = Vec::new();
    let mut next_teleport_id = 2;
    let mut pending_teleport = None;

    let corrected = correct_player_collision(
        Some(&state),
        &mut writer,
        Compression::Disabled,
        old_pose,
        new_pose,
        0,
        &mut next_teleport_id,
        &mut pending_teleport,
    )
    .await
    .unwrap();

    assert!(corrected);
    assert_eq!(decode_player_position_sync_packets(&writer).len(), 1);
    assert_eq!(pending_teleport.unwrap().id, 2);
    assert_eq!(next_teleport_id, 3);
}

#[tokio::test]
async fn player_collision_uses_farmland_height_and_allows_wheat_overlap() {
    let state = interaction_state_for_blocks(Arc::new(crop_test_registry()));
    insert_fluid_test_chunk(&state).await;
    {
        let mut world = state.world.lock().await;
        world
            .set_block_at(mc_world::BlockPos { x: 0, y: 64, z: 0 }, BlockStateId(3))
            .unwrap();
        world
            .set_block_at(mc_world::BlockPos { x: 0, y: 65, z: 0 }, BlockStateId(18))
            .unwrap();
    }

    assert!(
        !player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.9375, 0.5),).await,
        "the vanilla client stands at 15/16 block height and overlaps non-colliding wheat"
    );
    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.90, 0.5)).await,
        "the farmland collision shape must still reject movement through its top surface"
    );
}

fn vanilla_collision_test_state() -> InteractionState {
    interaction_state_for_blocks(Arc::new(
        mc_world::BlockRegistry::from_report(&solaris_required_blocks_report())
            .expect("embedded vanilla registry builds"),
    ))
}

fn vanilla_collision_state_id(
    state: &InteractionState,
    block_name: &str,
    properties: &[(&str, &str)],
) -> BlockStateId {
    let block_name = Identifier::parse(block_name).expect("valid test block name");
    let properties = properties
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect::<Vec<_>>();
    state
        .blocks
        .by_name_and_props(&block_name, &properties)
        .unwrap_or_else(|| panic!("missing vanilla state {block_name} {properties:?}"))
}

async fn set_collision_test_block(state: &InteractionState, state_id: BlockStateId) {
    insert_fluid_test_chunk(state).await;
    state
        .world
        .lock()
        .await
        .set_block_at(mc_world::BlockPos { x: 0, y: 64, z: 0 }, state_id)
        .expect("collision test block is inside the loaded chunk");
}

fn synthetic_collision_overlap_test_state() -> (InteractionState, BlockStateId) {
    let mut reports = solaris_required_blocks_report();
    let slab = reports
        .iter_mut()
        .find(|block| block.id.as_str() == "minecraft:stone_slab")
        .expect("embedded registry contains stone slab");
    let overlapping_state = slab
        .states
        .iter()
        .find(|state| {
            state.properties.get("type").map(String::as_str) == Some("bottom")
                && state.properties.get("waterlogged").map(String::as_str) == Some("false")
        })
        .expect("embedded registry contains a dry bottom stone slab")
        .id;
    slab.id = Identifier::parse("solaris:synthetic_solid").unwrap();

    let blocks = mc_world::BlockRegistry::from_report(&reports)
        .expect("renamed synthetic registry retains dense vanilla state ids");
    (
        interaction_state_for_blocks(Arc::new(blocks)),
        BlockStateId(overlapping_state),
    )
}

fn minecraft_synthetic_slab_overlap_test_state() -> (InteractionState, BlockStateId) {
    let mut reports = solaris_required_blocks_report();
    let slab = reports
        .iter_mut()
        .find(|block| block.id.as_str() == "minecraft:stone_slab")
        .expect("embedded registry contains stone slab");
    let overlapping_state = slab
        .states
        .iter()
        .find(|state| {
            state.properties.get("type").map(String::as_str) == Some("bottom")
                && state.properties.get("waterlogged").map(String::as_str) == Some("false")
        })
        .expect("embedded registry contains a dry bottom stone slab")
        .id;
    slab.id = Identifier::parse("minecraft:synthetic_slab").unwrap();

    let blocks = mc_world::BlockRegistry::from_report(&reports)
        .expect("renamed synthetic registry retains dense vanilla state ids");
    (
        interaction_state_for_blocks(Arc::new(blocks)),
        BlockStateId(overlapping_state),
    )
}

fn fake_farmland_slab_overlap_test_state() -> (InteractionState, BlockStateId) {
    let mut reports = solaris_required_blocks_report();
    reports
        .iter_mut()
        .find(|block| block.id.as_str() == "minecraft:farmland")
        .expect("embedded registry contains farmland")
        .id = Identifier::parse("solaris:canonical_farmland").unwrap();
    let slab = reports
        .iter_mut()
        .find(|block| block.id.as_str() == "minecraft:stone_slab")
        .expect("embedded registry contains stone slab");
    let overlapping_state = slab
        .states
        .iter()
        .find(|state| {
            state.properties.get("type").map(String::as_str) == Some("bottom")
                && state.properties.get("waterlogged").map(String::as_str) == Some("false")
        })
        .expect("embedded registry contains a dry bottom stone slab")
        .id;
    slab.id = Identifier::parse("minecraft:farmland").unwrap();

    let blocks = mc_world::BlockRegistry::from_report(&reports)
        .expect("renamed synthetic registry retains dense vanilla state ids");
    (
        interaction_state_for_blocks(Arc::new(blocks)),
        BlockStateId(overlapping_state),
    )
}

fn wrong_property_slab_overlap_test_state() -> (InteractionState, BlockStateId) {
    let mut reports = solaris_required_blocks_report();
    let slab = reports
        .iter_mut()
        .find(|block| block.id.as_str() == "minecraft:stone_slab")
        .expect("embedded registry contains stone slab");
    let state = slab
        .states
        .iter_mut()
        .find(|state| {
            state.properties.get("type").map(String::as_str) == Some("bottom")
                && state.properties.get("waterlogged").map(String::as_str) == Some("false")
        })
        .expect("embedded registry contains a dry bottom stone slab");
    state
        .properties
        .insert("type".to_string(), "synthetic".to_string());
    let overlapping_state = state.id;

    let blocks = mc_world::BlockRegistry::from_report(&reports)
        .expect("altered synthetic registry retains dense vanilla state ids");
    (
        interaction_state_for_blocks(Arc::new(blocks)),
        BlockStateId(overlapping_state),
    )
}

fn low_id_exact_farmland_test_state() -> (InteractionState, BlockStateId) {
    let reports = vec![
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:farmland").unwrap(),
            properties: prop_schema(&[("moisture", &["0", "1", "2", "3", "4", "5", "6", "7"])]),
            states: vec![state(1, true, &[("moisture", "0")])],
        },
    ];
    let blocks = mc_world::BlockRegistry::from_report(&reports)
        .expect("low-id exact farmland registry builds");
    (
        interaction_state_for_blocks(Arc::new(blocks)),
        BlockStateId(1),
    )
}

#[tokio::test]
async fn player_collision_does_not_apply_vanilla_shape_to_unrelated_overlapping_state_id() {
    let (state, synthetic_solid) = synthetic_collision_overlap_test_state();
    assert!(
        mc_data::collision_shapes::vanilla_collision_shapes()
            .get(synthetic_solid.0)
            .is_some(),
        "the synthetic state must overlap a covered vanilla state id"
    );
    set_collision_test_block(&state, synthetic_solid).await;

    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.5, 0.5)).await,
        "an unrelated synthetic solid keeps full-cube collision despite its overlapping state id"
    );
}

#[tokio::test]
async fn player_collision_rejects_minecraft_synthetic_slab_identity_on_overlapping_id() {
    let (state, synthetic_slab) = minecraft_synthetic_slab_overlap_test_state();
    set_collision_test_block(&state, synthetic_slab).await;

    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.5, 0.5)).await,
        "a synthetic Minecraft slab name must not inherit the overlapping vanilla slab shape"
    );
}

#[tokio::test]
async fn player_collision_rejects_fake_farmland_identity_on_overlapping_slab_id() {
    let (state, fake_farmland) = fake_farmland_slab_overlap_test_state();
    set_collision_test_block(&state, fake_farmland).await;

    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.5, 0.5)).await,
        "fake farmland properties must neither inherit the slab table shape nor farmland height"
    );
}

#[tokio::test]
async fn player_collision_rejects_wrong_properties_under_canonical_slab_name_and_id() {
    let (state, altered_slab) = wrong_property_slab_overlap_test_state();
    set_collision_test_block(&state, altered_slab).await;

    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.5, 0.5)).await,
        "a canonical name and numeric id are insufficient when ordered properties differ"
    );
}

#[tokio::test]
async fn player_collision_uses_farmland_fallback_for_exact_low_id_semantics() {
    let (state, farmland) = low_id_exact_farmland_test_state();
    set_collision_test_block(&state, farmland).await;

    assert!(
        !player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.9375, 0.5)).await,
        "exact farmland semantics retain the direct 15/16 fallback on a noncanonical id"
    );
    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.90, 0.5)).await,
        "the exact farmland fallback still rejects overlap below its top"
    );
}

#[tokio::test]
async fn player_collision_uses_bottom_slab_box() {
    let state = vanilla_collision_test_state();
    let slab = vanilla_collision_state_id(
        &state,
        "minecraft:stone_slab",
        &[("type", "bottom"), ("waterlogged", "false")],
    );
    set_collision_test_block(&state, slab).await;

    assert!(
        !player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.5, 0.5)).await,
        "a player may stand on the bottom slab's half-block top"
    );
    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.49, 0.5)).await,
        "a player may not overlap the bottom slab box"
    );
}

#[tokio::test]
async fn player_collision_uses_oracle_aabb_deflation_boundary() {
    let state = vanilla_collision_test_state();
    let slab = vanilla_collision_state_id(
        &state,
        "minecraft:stone_slab",
        &[("type", "bottom"), ("waterlogged", "false")],
    );
    set_collision_test_block(&state, slab).await;
    let oracle_deflation = f64::from(1.0e-5_f32);

    assert!(
        !player_pose_collides_with_solid(
            Some(&state),
            PlayerPose::new(0.5, 64.5 - oracle_deflation / 2.0, 0.5),
        )
        .await,
        "an overlap below the oracle deflation remains non-colliding"
    );
    assert!(
        player_pose_collides_with_solid(
            Some(&state),
            PlayerPose::new(0.5, 64.5 - oracle_deflation * 2.0, 0.5),
        )
        .await,
        "an overlap beyond the oracle deflation collides"
    );
}

#[tokio::test]
async fn player_collision_uses_top_slab_box() {
    let state = vanilla_collision_test_state();
    let slab = vanilla_collision_state_id(
        &state,
        "minecraft:stone_slab",
        &[("type", "top"), ("waterlogged", "false")],
    );
    set_collision_test_block(&state, slab).await;

    assert!(
        !player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 62.7, 0.5)).await,
        "the lower half below a top slab is empty"
    );
    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 62.71, 0.5)).await,
        "the player's head may not enter the top slab box"
    );
}

#[tokio::test]
async fn player_collision_uses_oriented_stair_boxes() {
    let state = vanilla_collision_test_state();
    let stair = vanilla_collision_state_id(
        &state,
        "minecraft:oak_stairs",
        &[
            ("facing", "north"),
            ("half", "bottom"),
            ("shape", "straight"),
            ("waterlogged", "false"),
        ],
    );
    set_collision_test_block(&state, stair).await;

    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.5, 0.15)).await,
        "the north stair's upper step occupies its north half"
    );
    assert!(
        !player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.5, 0.85)).await,
        "the south half above a north stair's lower step is empty"
    );
}

#[tokio::test]
async fn player_collision_uses_tall_narrow_fence_box() {
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

    assert!(
        !player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.05, 64.0, 0.5)).await,
        "space beside an isolated fence post is empty"
    );
    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 65.25, 0.5)).await,
        "the isolated fence post collision extends to 1.5 blocks"
    );
}

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

#[tokio::test]
async fn player_collision_uses_exact_full_cube_shape_for_stone() {
    let state = vanilla_collision_test_state();
    let stone = vanilla_collision_state_id(&state, "minecraft:stone", &[]);
    set_collision_test_block(&state, stone).await;

    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.0, 0.5)).await,
        "the exact stone shape remains a full cube"
    );
}

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

#[tokio::test]
async fn powder_snow_collision_uses_player_equipment_and_movement_context() {
    let mut state = vanilla_collision_test_state();
    let powder_snow = vanilla_collision_state_id(&state, "minecraft:powder_snow", &[]);
    set_collision_test_block(&state, powder_snow).await;

    let above = PlayerPose::new(0.5, 65.0, 0.5);
    let entering = PlayerPose::new(0.5, 64.99, 0.5);
    assert!(
        !player_pose_collides_with_solid_using_context(Some(&state), entering, above).await,
        "a player without leather boots sinks into powder snow"
    );

    const LEATHER_BOOTS_ID: u32 = 1;
    state.items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:leather_boots").unwrap(),
        protocol_id: LEATHER_BOOTS_ID,
    }]));
    state.inventory.slots[PlayerInventory::FEET_ARMOR_SLOT] = ItemStack::new(LEATHER_BOOTS_ID, 1);
    assert!(
        player_pose_collides_with_solid_using_context(Some(&state), entering, above).await,
        "leather boots support a player entering powder snow from above"
    );

    let mut descending = above;
    descending.shifting = true;
    assert!(
        !player_pose_collides_with_solid_using_context(Some(&state), entering, descending).await,
        "holding Shift lets a leather-booted player descend through powder snow"
    );
    assert!(
        !player_pose_collides_with_solid_using_context(
            Some(&state),
            PlayerPose::new(0.5, 64.4, 0.5),
            PlayerPose::new(0.5, 64.5, 0.5),
        )
        .await,
        "boots do not turn powder snow solid after the player is already inside it"
    );
}

#[tokio::test]
async fn powder_snow_uses_falling_collision_shape_after_long_fall() {
    let state = vanilla_collision_test_state();
    let powder_snow = vanilla_collision_state_id(&state, "minecraft:powder_snow", &[]);
    set_collision_test_block(&state, powder_snow).await;

    let mut above_shape = PlayerPose::new(0.5, 64.9, 0.5);
    above_shape.fall_start_y = 68.0;
    assert!(
        !player_pose_collides_with_solid(Some(&state), above_shape).await,
        "the falling collision shape ends at the exact 0.9F boundary"
    );

    let mut inside_shape = PlayerPose::new(0.5, 64.89, 0.5);
    inside_shape.fall_start_y = 68.0;
    assert!(
        player_pose_collides_with_solid(Some(&state), inside_shape).await,
        "a fall longer than 2.5 blocks collides with powder snow's 0.9F shape"
    );
}

#[tokio::test]
async fn powder_snow_dynamic_shape_requires_exact_vanilla_state_identity() {
    let mut reports = solaris_required_blocks_report();
    let powder_snow = reports
        .iter_mut()
        .find(|block| block.id.as_str() == "minecraft:powder_snow")
        .expect("embedded registry contains powder snow");
    let state_id = powder_snow.states[0].id;
    powder_snow
        .properties
        .insert("solaris_test".to_string(), vec!["mismatch".to_string()]);
    powder_snow.states[0]
        .properties
        .insert("solaris_test".to_string(), "mismatch".to_string());
    let blocks = mc_world::BlockRegistry::from_report(&reports)
        .expect("altered powder snow registry retains dense vanilla state ids");
    let mut state = interaction_state_for_blocks(Arc::new(blocks));
    set_collision_test_block(&state, BlockStateId(state_id)).await;

    const LEATHER_BOOTS_ID: u32 = 1;
    state.items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:leather_boots").unwrap(),
        protocol_id: LEATHER_BOOTS_ID,
    }]));
    state.inventory.slots[PlayerInventory::FEET_ARMOR_SLOT] = ItemStack::new(LEATHER_BOOTS_ID, 1);
    assert!(
        player_pose_collides_with_solid_using_context(
            Some(&state),
            PlayerPose::new(0.5, 64.5, 0.5),
            PlayerPose::new(0.5, 64.5, 0.5),
        )
        .await,
        "a fingerprint mismatch must use conservative custom-block fallback"
    );
}

#[tokio::test]
async fn collision_correction_applies_powder_snow_movement_context() {
    let mut state = vanilla_collision_test_state();
    let powder_snow = vanilla_collision_state_id(&state, "minecraft:powder_snow", &[]);
    set_collision_test_block(&state, powder_snow).await;
    const LEATHER_BOOTS_ID: u32 = 1;
    state.items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:leather_boots").unwrap(),
        protocol_id: LEATHER_BOOTS_ID,
    }]));
    state.inventory.slots[PlayerInventory::FEET_ARMOR_SLOT] = ItemStack::new(LEATHER_BOOTS_ID, 1);

    let mut writer = Vec::new();
    let mut next_teleport_id = 1;
    let mut pending_teleport = None;
    assert!(
        correct_player_collision(
            Some(&state),
            &mut writer,
            Compression::Disabled,
            PlayerPose::new(0.5, 65.0, 0.5),
            PlayerPose::new(0.5, 64.99, 0.5),
            10,
            &mut next_teleport_id,
            &mut pending_teleport,
        )
        .await
        .unwrap(),
        "leather boots must correct entry through powder snow from above"
    );

    writer.clear();
    pending_teleport = None;
    let mut descending = PlayerPose::new(0.5, 65.0, 0.5);
    descending.shifting = true;
    assert!(
        !correct_player_collision(
            Some(&state),
            &mut writer,
            Compression::Disabled,
            descending,
            PlayerPose::new(0.5, 64.99, 0.5),
            11,
            &mut next_teleport_id,
            &mut pending_teleport,
        )
        .await
        .unwrap(),
        "Shift descent must pass through the correction path"
    );

    let mut falling = PlayerPose::new(0.5, 64.91, 0.5);
    falling.fall_start_y = 68.0;
    let mut landing = PlayerPose::new(0.5, 64.89, 0.5);
    landing.fall_start_y = 68.0;
    assert!(
        correct_player_collision(
            Some(&state),
            &mut writer,
            Compression::Disabled,
            falling,
            landing,
            12,
            &mut next_teleport_id,
            &mut pending_teleport,
        )
        .await
        .unwrap(),
        "a long fall must collide with the 0.9F landing shape"
    );
}

#[tokio::test]
async fn movement_block_reads_do_not_wait_for_world_writer() {
    let mut state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    state.block_facts = Arc::new(fluid_test_facts());
    insert_fluid_test_chunk(&state).await;
    {
        let mut storage = state.world.lock().await;
        storage
            .set_block_at(mc_world::BlockPos { x: 0, y: 64, z: 0 }, BlockStateId(1))
            .unwrap();
        storage
            .set_block_at(mc_world::BlockPos { x: 2, y: 64, z: 0 }, BlockStateId(2))
            .unwrap();
    }

    let world = Arc::clone(&state.world);
    let world_writer = world.lock().await;

    let mut collision = Box::pin(player_pose_collides_with_solid(
        Some(&state),
        PlayerPose::new(0.5, 64.0, 0.5),
    ));
    std::future::poll_fn(|cx| {
        assert!(
            matches!(
                std::future::Future::poll(collision.as_mut(), cx),
                Poll::Ready(true)
            ),
            "collision over a loaded chunk must not wait for the world writer"
        );
        Poll::Ready(())
    })
    .await;

    let mut water = Box::pin(player_water_overlap(
        &state,
        PlayerPose::new(2.5, 64.0, 0.5),
    ));
    std::future::poll_fn(|cx| {
        assert!(
            matches!(
                std::future::Future::poll(water.as_mut(), cx),
                Poll::Ready((true, false))
            ),
            "water overlap over a loaded chunk must not wait for the world writer"
        );
        Poll::Ready(())
    })
    .await;

    drop(world_writer);
}

#[tokio::test]
async fn crafting_table_open_does_not_wait_for_world_writer() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:crafting_table"),
        ])
        .unwrap(),
    );
    let mut state = interaction_state_for_blocks(blocks);
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = state.world.lock().await;
        let chunk = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage.set_block_at(position, BlockStateId(1)).unwrap();
    }

    let world = Arc::clone(&state.world);
    let world_writer = world.lock().await;
    let mut writer = Vec::new();
    let mut open = Box::pin(open_crafting_table_container(
        &mut state,
        &mut writer,
        PlayerPose::new(1.5, 64.0, 1.5),
        7,
        position.x,
        position.y,
        position.z,
    ));
    std::future::poll_fn(|cx| {
        assert!(
            matches!(
                std::future::Future::poll(open.as_mut(), cx),
                Poll::Ready(Ok(true))
            ),
            "opening a loaded crafting table must use the published world view"
        );
        Poll::Ready(())
    })
    .await;

    drop(world_writer);
}

#[tokio::test]
async fn stonecutter_open_uses_proved_menu_type_and_published_world_view() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:stonecutter"),
        ])
        .unwrap(),
    );
    let mut state = interaction_state_for_blocks(blocks);
    state.items = stonecutter_test_items();
    state.recipes.push(stonecutter_test_recipe());
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = state.world.lock().await;
        let chunk = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage.set_block_at(position, BlockStateId(1)).unwrap();
    }

    let world = Arc::clone(&state.world);
    let world_writer = world.lock().await;
    let mut writer = Vec::new();
    assert!(
        open_stonecutter_container(
            &mut state,
            &mut writer,
            PlayerPose::new(1.5, 64.0, 1.5),
            7,
            position,
        )
        .await
        .unwrap()
    );
    let Some(ActiveContainer::Stonecutter(window)) = state.active_container.as_ref() else {
        panic!("stonecutter window must become active");
    };
    assert_eq!(window.state_id, 1);
    assert_eq!(STONECUTTER_MENU_TYPE_ID, 24);
    let mut frames = bytes::BytesMut::from(writer.as_slice());
    let first = mc_protocol::frame::try_decode_frame(&mut frames, Compression::Disabled)
        .unwrap()
        .unwrap();
    assert_eq!(
        first.id,
        mc_protocol::packets::play::ClientboundOpenScreen::ID
    );
    while let Some(frame) =
        mc_protocol::frame::try_decode_frame(&mut frames, Compression::Disabled).unwrap()
    {
        assert_ne!(
            frame.id,
            mc_protocol::packets::play::ClientboundUpdateRecipes::ID,
            "stonecutter open must not resend the initial recipe update",
        );
    }

    drop(world_writer);
}

#[tokio::test]
async fn block_placement_planning_does_not_wait_for_world_writer() {
    let state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    insert_fluid_test_chunk(&state).await;
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };

    let world = Arc::clone(&state.world);
    let world_writer = world.lock().await;
    let mut planning = Box::pin(plan_place_block_edits(
        &state,
        position,
        BlockStateId(1),
        PlayerPose::new(1.5, 64.0, 1.5),
        Direction::Up,
        0.5,
    ));
    std::future::poll_fn(
        |cx| match std::future::Future::poll(planning.as_mut(), cx) {
            Poll::Ready(Some(plan)) => {
                assert_eq!(
                    plan.edits,
                    vec![BlockEdit {
                        pos: position,
                        new_state: BlockStateId(1),
                    }]
                );
                Poll::Ready(())
            }
            Poll::Ready(None) => panic!("valid loaded stone placement was rejected"),
            Poll::Pending => panic!("placement planning waited for the world writer"),
        },
    )
    .await;

    drop(world_writer);
}

#[tokio::test]
async fn toggle_planning_does_not_wait_for_world_writer() {
    let state = interaction_state_for_blocks(Arc::new(button_test_registry()));
    insert_fluid_test_chunk(&state).await;
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let expected_token = {
        let mut storage = state.world.lock().await;
        storage
            .set_block_at(position, BlockStateId(1))
            .expect("place unpowered button");
        storage
            .block_mutation_token(position)
            .expect("button mutation token")
    };

    let world = Arc::clone(&state.world);
    let world_writer = world.lock().await;
    let plan = plan_loaded_toggle_block_interaction(&state, position, 100)
        .expect("published button should be interactive");

    assert_eq!(
        plan.edits,
        vec![BlockEdit {
            pos: position,
            new_state: BlockStateId(2),
        }]
    );
    assert_eq!(plan.preconditions.len(), 1);
    assert_eq!(plan.preconditions[0].expected_token, expected_token);
    assert_eq!(plan.scheduled_block_ticks[0].trigger_tick, 120);
    drop(world_writer);
}

#[test]
fn entity_tick_cadence_matches_vanilla_cow_tracking() {
    assert_eq!(ENTITY_TICK_PERIOD, Duration::from_millis(50));
    assert_eq!(mc_physics::TICK_SECONDS, 0.05);
    assert_eq!(ENTITY_MOVE_SEND_INTERVAL_TICKS, 3);
}

#[test]
fn arrow_launch_uses_player_look_direction_and_draw_power() {
    let pose = PlayerPose {
        yaw: 90.0,
        pitch: -30.0,
        ..PlayerPose::new(1.0, 64.0, 2.0)
    };

    let spawn = arrow_spawn_position(pose);
    let velocity = arrow_velocity(pose, 0.5);

    assert!((spawn.x - 1.0).abs() < 0.000_001);
    assert!((spawn.y - 65.62).abs() < 0.000_001);
    assert!((spawn.z - 2.0).abs() < 0.000_001);
    assert!((velocity.x + 1.299_038_105_676_658).abs() < 0.000_001);
    assert!((velocity.y - 0.75).abs() < 0.000_001);
    assert!(velocity.z.abs() < 0.000_001);
}

#[test]
fn gamemode_command_parses_names_and_numeric_modes() {
    assert_eq!(
        parse_gamemode_command("gamemode survival"),
        Some(GameMode::Survival)
    );
    assert_eq!(
        parse_gamemode_command("gamemode creative"),
        Some(GameMode::Creative)
    );
    assert_eq!(
        parse_gamemode_command("gamemode adventure"),
        Some(GameMode::Adventure)
    );
    assert_eq!(
        parse_gamemode_command("gamemode spectator"),
        Some(GameMode::Spectator)
    );
    assert_eq!(
        parse_gamemode_command("gamemode 1"),
        Some(GameMode::Creative)
    );
}

#[test]
fn gamemode_command_rejects_unknown_or_extra_args() {
    assert_eq!(parse_gamemode_command("time set day"), None);
    assert_eq!(parse_gamemode_command("gamemode nope"), None);
    assert_eq!(parse_gamemode_command("gamemode creative other"), None);
}

#[test]
fn client_view_distance_is_clamped_to_server_policy() {
    assert_eq!(clamp_client_view_distance(12, 8), 8);
    assert_eq!(clamp_client_view_distance(6, 10), 6);
    assert_eq!(clamp_client_view_distance(0, 10), 2);
    assert_eq!(clamp_client_view_distance(-8, 1), 2);
    assert_eq!(clamp_client_view_distance(i8::MAX, i32::MAX), 32);
}

#[test]
fn oversized_play_custom_payload_is_rejected_before_decode() {
    let body = Bytes::from(vec![0x80; DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES + 1]);

    let action = classify_play_custom_payload(body).unwrap();

    assert_eq!(
        action,
        PlayCustomPayloadAction::Oversized {
            len: DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES + 1
        }
    );
}

#[test]
fn loader_interaction_channel_is_claimed_before_extension_forwarding() {
    let channel = b"solaris:loader/interaction";
    let payload = b"action";
    let mut body = Vec::with_capacity(1 + channel.len() + payload.len());
    body.push(channel.len() as u8);
    body.extend_from_slice(channel);
    body.extend_from_slice(payload);

    assert_eq!(
        classify_play_custom_payload(Bytes::from(body)).unwrap(),
        PlayCustomPayloadAction::LoaderInteraction(Bytes::from_static(payload))
    );
}

fn test_use_item_on(position: i64) -> ServerboundUseItemOn {
    ServerboundUseItemOn {
        hand: mc_protocol::packets::play::InteractionHand::MainHand,
        position,
        direction: mc_protocol::packets::play::Direction::Up,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside: false,
        world_border_hit: false,
        sequence: 4,
    }
}

#[test]
fn use_item_on_preflight_reports_dead_survival_player() {
    let mut survival = SurvivalState::FULL;
    survival.health = 0.0;
    let action = test_use_item_on(pack_block_pos(0, 64, 0));

    assert_eq!(
        classify_use_item_on_preflight(
            GameMode::Survival,
            survival,
            PlayerPose::new(0.5, 64.0, 0.5),
            &action,
        ),
        UseItemOnOutcome::NoOp {
            reason: UseItemOnNoOpReason::DeadPlayer,
        }
    );
}

#[test]
fn use_item_on_preflight_reports_unsupported_game_mode() {
    let action = test_use_item_on(pack_block_pos(0, 64, 0));

    assert_eq!(
        classify_use_item_on_preflight(
            GameMode::Adventure,
            SurvivalState::FULL,
            PlayerPose::new(0.5, 64.0, 0.5),
            &action,
        ),
        UseItemOnOutcome::NoOp {
            reason: UseItemOnNoOpReason::UnsupportedGameMode,
        }
    );
}

#[test]
fn use_item_on_preflight_reports_out_of_reach_survival_target() {
    let action = test_use_item_on(pack_block_pos(128, 64, 0));

    assert_eq!(
        classify_use_item_on_preflight(
            GameMode::Survival,
            SurvivalState::FULL,
            PlayerPose::new(0.5, 64.0, 0.5),
            &action,
        ),
        UseItemOnOutcome::NoOp {
            reason: UseItemOnNoOpReason::OutOfReach,
        }
    );
}

#[test]
fn use_item_on_preflight_rejects_out_of_reach_creative_and_allows_reachable_targets() {
    let action = test_use_item_on(pack_block_pos(0, 64, 0));

    assert_eq!(
        classify_use_item_on_preflight(
            GameMode::Creative,
            SurvivalState::FULL,
            PlayerPose::new(100.5, 64.0, 100.5),
            &action,
        ),
        UseItemOnOutcome::NoOp {
            reason: UseItemOnNoOpReason::OutOfReach,
        }
    );
    assert_eq!(
        classify_use_item_on_preflight(
            GameMode::Creative,
            SurvivalState::FULL,
            PlayerPose::new(0.5, 64.0, 0.5),
            &action,
        ),
        UseItemOnOutcome::PlaceBlock
    );
    assert_eq!(
        classify_use_item_on_preflight(
            GameMode::Survival,
            SurvivalState::FULL,
            PlayerPose::new(0.5, 64.0, 0.5),
            &action,
        ),
        UseItemOnOutcome::PlaceBlock
    );
}

#[test]
fn recoverable_death_xp_uses_level_cap() {
    let mut xp = XpState {
        total: 1_000,
        level: 40,
        ..XpState::default()
    };

    assert_eq!(recoverable_death_xp(&xp), 100);

    xp.level = 3;
    assert_eq!(recoverable_death_xp(&xp), 21);
}

#[test]
fn debug_commands_parse_survival_mutations_and_give() {
    assert_eq!(
        parse_debug_command("debug survival damage 7.5"),
        Some(DebugCommand::Survival(SurvivalCommand::Damage(7.5)))
    );
    assert_eq!(
        parse_debug_command("debug survival heal"),
        Some(DebugCommand::Survival(SurvivalCommand::Heal(20.0)))
    );
    assert_eq!(
        parse_debug_command("debug survival feed 2 0.5"),
        Some(DebugCommand::Survival(SurvivalCommand::Feed {
            food: 2,
            saturation: 0.5
        }))
    );
    assert_eq!(
        parse_debug_command("debug survival exhaust 4"),
        Some(DebugCommand::Survival(SurvivalCommand::Exhaust(4.0)))
    );
    assert_eq!(
        parse_debug_command("debug survival xp 35"),
        Some(DebugCommand::Survival(SurvivalCommand::Experience(35)))
    );
    assert_eq!(
        parse_debug_command("debug give minecraft:dirt 64 1"),
        Some(DebugCommand::Give {
            item: mc_data::Identifier::parse("minecraft:dirt").unwrap(),
            count: 64,
            hotbar_slot: 1,
        })
    );
    assert_eq!(
        parse_debug_command("debug outbound-pressure 192"),
        Some(DebugCommand::OutboundPressure { count: 192 })
    );
    assert_eq!(
        parse_debug_command("debug water-corridor 4 96 0"),
        Some(DebugCommand::WaterCorridor { x: 4, y: 96, z: 0 })
    );
    assert_eq!(parse_debug_command("debug water-corridor"), None);
    assert_eq!(parse_debug_command("debug water-corridor 4 317 0"), None);
    assert_eq!(
        parse_debug_command("debug water-corridor 4 96 0 extra"),
        None
    );
    assert_eq!(parse_debug_command("debug outbound-pressure 0"), None);
    assert_eq!(parse_debug_command("debug outbound-pressure 257"), None);
    assert_eq!(parse_debug_command("damage 7.5"), None);
    assert_eq!(parse_debug_command("debug survival damage bad"), None);
    assert_eq!(parse_debug_command("debug survival damage NaN"), None);
    assert_eq!(parse_debug_command("debug survival heal inf"), None);
    assert_eq!(parse_debug_command("debug survival feed 2 -inf"), None);
    assert_eq!(parse_debug_command("debug survival exhaust NaN"), None);
}

#[test]
fn debug_water_corridor_fixture_is_closed_unique_and_source_filled() {
    let state = interaction_state_for_blocks(Arc::new(
        mc_world::BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap(),
    ));
    let water = state
        .blocks
        .block(&Identifier::parse("minecraft:water").unwrap())
        .expect("fixture registry has water")
        .default;
    let stone = state
        .blocks
        .block(&Identifier::parse("minecraft:stone").unwrap())
        .expect("fixture registry has stone")
        .default;
    let edits = debug_water_corridor_edits(
        &state.blocks,
        Some(water),
        mc_world::BlockPos { x: 4, y: 66, z: 0 },
    )
    .expect("water corridor plan");

    assert_eq!(edits.len(), 68);
    let unique = edits.iter().map(|edit| edit.pos).collect::<HashSet<_>>();
    assert_eq!(unique.len(), edits.len(), "fixture edits must be unique");
    for z in 0..=4 {
        assert!(edits.contains(&BlockEdit {
            pos: mc_world::BlockPos { x: 4, y: 66, z },
            new_state: water,
        }));
        assert!(edits.contains(&BlockEdit {
            pos: mc_world::BlockPos { x: 4, y: 67, z },
            new_state: water,
        }));
        assert!(edits.contains(&BlockEdit {
            pos: mc_world::BlockPos { x: 4, y: 65, z },
            new_state: stone,
        }));
    }
}

#[tokio::test]
async fn debug_give_zero_count_clears_hotbar_slot_before_item_lookup() {
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: dirt,
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(Arc::clone(&items));
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(10, 1);
    let session_id = register_interaction_player(&mut state, "DebugGiveClear");
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    state.simulation = simulation.for_session(session_id);
    let mut writer = Vec::new();
    let mut survival_state = SurvivalState::FULL;
    let mut xp_state = XpState::default();

    apply_debug_command(
        &mut writer,
        Compression::Disabled,
        DebugCommand::Give {
            item: Identifier::parse("minecraft:air").unwrap(),
            count: 0,
            hotbar_slot: 0,
        },
        DebugCommandContext {
            survival_state: &mut survival_state,
            xp_state: &mut xp_state,
            interaction: Some(&mut state),
            player_pose: PlayerPose::new(0.0, 64.0, 0.0),
            permissions: CommandPermissions { op: true },
        },
    )
    .await
    .unwrap();

    stop.send(()).unwrap();
    task.await.unwrap();

    assert_eq!(
        state.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::EMPTY
    );
    let packets = decode_container_set_slot_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].item_stack, ItemStack::EMPTY);
}

#[test]
fn chest_quick_move_places_player_stack_in_first_empty_storage_slot() {
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: dirt,
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(Arc::clone(&items));
    let dirt_id = items
        .id_of(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(dirt_id, 1);
    let mut view = ChestView {
        chests: vec![ChestBlockEntity::default()],
    };

    assert!(apply_chest_quick_move_click(
        &mut state,
        &mut view,
        SINGLE_CHEST_STORAGE_SLOTS + 27,
    ));
    assert_eq!(
        furnace_slot_to_stack(&view.chests[0].slots[0]),
        ItemStack::new(dirt_id, 1)
    );
    assert_eq!(
        state.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::EMPTY
    );
}

#[test]
fn chest_quick_move_from_storage_uses_vanilla_reverse_player_range() {
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: dirt,
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(Arc::clone(&items));
    let dirt_id = items
        .id_of(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    let mut chest = ChestBlockEntity::default();
    chest.slots[0] = mc_world::FurnaceSlot {
        item_id: dirt_id,
        count: 2,
        damage: None,
        enchantments: Vec::new(),
    };
    let mut view = ChestView {
        chests: vec![chest],
    };

    assert!(apply_chest_quick_move_click(&mut state, &mut view, 0));
    assert!(view.chests[0].slots[0].is_empty());
    assert_eq!(
        state.inventory.slots[44],
        ItemStack::new(dirt_id, 2),
        "vanilla fills the reverse player range before earlier main-inventory slots"
    );
    assert!(state.inventory.slots[9..44].iter().all(ItemStack::is_empty));
}

#[test]
fn chest_actions_respect_item_specific_stack_limits() {
    let bucket = Identifier::parse("minecraft:bucket").unwrap();
    let snowball = Identifier::parse("minecraft:snowball").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: bucket.clone(),
            protocol_id: 10,
        },
        ItemReport {
            id: snowball.clone(),
            protocol_id: 11,
        },
    ]);
    let item_facts = ItemFactsTable::from_entries([
        (
            bucket,
            mc_data::item_components::ItemFacts {
                max_stack_size: Some(1),
                ..mc_data::item_components::ItemFacts::default()
            },
        ),
        (
            snowball,
            mc_data::item_components::ItemFacts {
                max_stack_size: Some(16),
                ..mc_data::item_components::ItemFacts::default()
            },
        ),
    ]);
    let new_window = || ChestWindow::new(vec![mc_world::BlockPos { x: 0, y: 64, z: 0 }], 7);
    let empty_view = || ChestView {
        chests: vec![ChestBlockEntity::default()],
    };

    let mut bucket_view = empty_view();
    bucket_view.chests[0].slots[0] = stack_to_furnace_slot(&ItemStack::new(10, 1));
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(10, 1);
    let bucket_quick_move = plan_chest_click(ChestClickInput {
        items: &items,
        item_facts: &item_facts,
        window: new_window(),
        view: bucket_view,
        inventory,
        carried_item: ItemStack::EMPTY,
        action: ChestClickAction::QuickMove {
            slot: SINGLE_CHEST_STORAGE_SLOTS + 27,
        },
    });
    assert!(bucket_quick_move.changed);
    assert!(bucket_quick_move.inventory.slots[PlayerInventory::HOTBAR_BASE].is_empty());
    assert_eq!(
        furnace_slot_to_stack(&bucket_quick_move.view.chests[0].slots[0]),
        ItemStack::new(10, 1)
    );
    assert_eq!(
        furnace_slot_to_stack(&bucket_quick_move.view.chests[0].slots[1]),
        ItemStack::new(10, 1)
    );
    assert!(
        bucket_quick_move.view.chests[0].slots[2..]
            .iter()
            .all(mc_world::FurnaceSlot::is_empty)
    );

    let mut full_bucket = ChestBlockEntity::default();
    full_bucket.slots[0] = stack_to_furnace_slot(&ItemStack::new(10, 1));
    let bucket_pickup = plan_chest_click(ChestClickInput {
        items: &items,
        item_facts: &item_facts,
        window: new_window(),
        view: ChestView {
            chests: vec![full_bucket],
        },
        inventory: PlayerInventory::empty(),
        carried_item: ItemStack::new(10, 1),
        action: ChestClickAction::Pickup { slot: 0, button: 1 },
    });
    assert!(!bucket_pickup.changed);
    assert_eq!(bucket_pickup.carried_item, ItemStack::new(10, 1));
    assert_eq!(
        furnace_slot_to_stack(&bucket_pickup.view.chests[0].slots[0]),
        ItemStack::new(10, 1)
    );

    let mut snowball_view = empty_view();
    snowball_view.chests[0].slots[0] = stack_to_furnace_slot(&ItemStack::new(11, 15));
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(11, 16);
    let snowball_quick_move = plan_chest_click(ChestClickInput {
        items: &items,
        item_facts: &item_facts,
        window: new_window(),
        view: snowball_view,
        inventory,
        carried_item: ItemStack::EMPTY,
        action: ChestClickAction::QuickMove {
            slot: SINGLE_CHEST_STORAGE_SLOTS + 27,
        },
    });
    assert!(snowball_quick_move.changed);
    assert_eq!(
        furnace_slot_to_stack(&snowball_quick_move.view.chests[0].slots[0]),
        ItemStack::new(11, 16)
    );
    assert_eq!(
        furnace_slot_to_stack(&snowball_quick_move.view.chests[0].slots[1]),
        ItemStack::new(11, 15)
    );

    let mut view = empty_view();
    view.chests[0].slots[0] = stack_to_furnace_slot(&ItemStack::new(11, 15));
    let mut window = new_window();
    let mut inventory = PlayerInventory::empty();
    let mut carried_item = ItemStack::new(11, 3);
    let mut changed = false;
    for click in [
        QuickCraftClick {
            header: 0,
            kind: 1,
            slot: None,
        },
        QuickCraftClick {
            header: 1,
            kind: 1,
            slot: Some(0),
        },
        QuickCraftClick {
            header: 1,
            kind: 1,
            slot: Some(1),
        },
        QuickCraftClick {
            header: 2,
            kind: 1,
            slot: None,
        },
    ] {
        let plan = plan_chest_click(ChestClickInput {
            items: &items,
            item_facts: &item_facts,
            window,
            view,
            inventory,
            carried_item,
            action: ChestClickAction::QuickCraft(click),
        });
        window = plan.window;
        view = plan.view;
        inventory = plan.inventory;
        carried_item = plan.carried_item;
        changed = plan.changed;
    }
    assert!(changed);
    assert_eq!(
        furnace_slot_to_stack(&view.chests[0].slots[0]),
        ItemStack::new(11, 16)
    );
    assert_eq!(
        furnace_slot_to_stack(&view.chests[0].slots[1]),
        ItemStack::new(11, 1)
    );
    assert_eq!(carried_item, ItemStack::new(11, 1));
}

#[test]
fn chest_menu_revision_counts_source_and_destination_slot_changes() {
    let mut before_chest = ChestBlockEntity::default();
    before_chest.slots[0] = mc_world::FurnaceSlot {
        item_id: 10,
        count: 2,
        damage: None,
        enchantments: Vec::new(),
    };
    let before_view = ChestView {
        chests: vec![before_chest],
    };
    let after_view = ChestView {
        chests: vec![ChestBlockEntity::default()],
    };
    let before_inventory = PlayerInventory::empty();
    let mut after_inventory = PlayerInventory::empty();
    after_inventory.slots[44] = ItemStack::new(10, 2);

    assert_eq!(
        chest_menu_state_change_count(
            &before_view,
            &after_view,
            &before_inventory,
            &after_inventory,
            &ItemStack::EMPTY,
            &ItemStack::EMPTY,
        ),
        2
    );
}

#[test]
fn crafting_menu_revision_counts_result_input_and_destination_changes() {
    let mut before_window = CraftingTableWindow::new(7);
    before_window.input[0] = ItemStack::new(10, 1);
    before_window.result = ItemStack::new(11, 4);
    let after_window = CraftingTableWindow::new(7);
    let before_inventory = PlayerInventory::empty();
    let mut after_inventory = PlayerInventory::empty();
    after_inventory.slots[44] = ItemStack::new(11, 4);

    assert_eq!(
        crafting_menu_state_change_count(
            &before_window,
            &after_window,
            &before_inventory,
            &after_inventory,
            &ItemStack::EMPTY,
            &ItemStack::EMPTY,
        ),
        3
    );
}

#[test]
fn persistent_container_claim_check_covers_furnace_and_both_chest_halves() {
    let first = mc_world::BlockPos { x: 1, y: 64, z: 2 };
    let second = mc_world::BlockPos { x: 2, y: 64, z: 2 };
    let chest = ActiveContainer::Chest(ChestWindow::new(vec![first, second], 7));
    assert!(!persistent_container_claim_allowed(&chest, |position| {
        position != second
    }));
    assert!(persistent_container_claim_allowed(&chest, |_| true));

    let furnace = ActiveContainer::Furnace(FurnaceWindow::new(first, 8, FurnaceKind::Furnace));
    assert!(!persistent_container_claim_allowed(&furnace, |_| false));
    assert!(persistent_container_claim_allowed(&furnace, |_| true));
}

#[test]
fn admin_dispatcher_parses_slash_commands_and_permissions() {
    let op = CommandPermissions { op: true };
    let not_op = CommandPermissions { op: false };

    assert_eq!(
        parse_admin_command("/gamemode creative", op),
        Ok(AdminCommand::GameMode(GameMode::Creative))
    );
    assert_eq!(
        parse_admin_command("give minecraft:dirt 12", op),
        Ok(AdminCommand::Give {
            item: mc_data::Identifier::parse("minecraft:dirt").unwrap(),
            count: 12,
        })
    );
    assert_eq!(
        parse_admin_command("/tp 1.5 70 -2", op),
        Ok(AdminCommand::Teleport {
            x: 1.5,
            y: 70.0,
            z: -2.0,
        })
    );
    assert_eq!(
        parse_admin_command("/summon minecraft:zombie", op),
        Ok(AdminCommand::Summon {
            entity: mc_data::Identifier::parse("minecraft:zombie").unwrap(),
            x: None,
            y: None,
            z: None,
        })
    );
    assert_eq!(parse_admin_command("/kill", op), Ok(AdminCommand::Kill));
    assert_eq!(parse_admin_command("/status", op), Ok(AdminCommand::Status));
    assert_eq!(
        parse_admin_command("/gamerule players_sleeping_percentage", op),
        Ok(AdminCommand::PlayersSleepingPercentage(None))
    );
    assert_eq!(
        parse_admin_command("/gamerule players_sleeping_percentage 50", op),
        Ok(AdminCommand::PlayersSleepingPercentage(Some(50)))
    );
    assert_eq!(
        parse_admin_command("/gamemode creative", not_op),
        Err(CommandError::PermissionDenied)
    );
    assert_eq!(
        parse_admin_command("/status extra", op),
        Err(CommandError::Usage("Usage: /status"))
    );
    assert_eq!(
        parse_admin_command("/gamemode", op),
        Err(CommandError::Usage(
            "Usage: /gamemode <survival|creative|adventure|spectator>"
        ))
    );
    assert_eq!(
        parse_admin_command("/doesnotexist", op),
        Err(CommandError::Unknown)
    );
    assert_eq!(
        parse_admin_command("/tp NaN 70 0", op),
        Err(CommandError::Usage("Usage: /tp <x> <y> <z>"))
    );
    assert_eq!(
        parse_admin_command("/tp 0 inf 0", op),
        Err(CommandError::Usage("Usage: /tp <x> <y> <z>"))
    );
    assert_eq!(
        parse_admin_command("/summon minecraft:zombie 0 70 -inf", op),
        Err(CommandError::Usage("Usage: /summon <entity> [x y z]"))
    );
    assert_eq!(
        parse_admin_command("/gamerule players_sleeping_percentage -1", op),
        Err(CommandError::Usage(
            "Usage: /gamerule <keep_inventory|players_sleeping_percentage> [value]"
        ))
    );
}

#[test]
fn command_tree_and_suggestions_are_permission_aware() {
    let op = CommandPermissions { op: true };
    let not_op = CommandPermissions { op: false };

    let tree = command_tree_packet(op);
    assert_eq!(tree.root_index, 0);
    assert_eq!(
        tree.nodes[0].children,
        vec![1, 6, 8, 10, 11, 12, 13, 15, 17, 19, 20]
    );
    assert_eq!(
        command_tree_packet(not_op).nodes[0].children,
        Vec::<i32>::new()
    );

    let root = command_suggestions("/g", op);
    assert_eq!(root.start, 1);
    assert_eq!(root.length, 1);
    assert_eq!(
        root.suggestions,
        vec![
            "gamemode".to_string(),
            "gamerule".to_string(),
            "give".to_string()
        ]
    );

    let modes = command_suggestions("/gamemode c", op);
    assert_eq!(modes.start, 10);
    assert_eq!(modes.length, 1);
    assert_eq!(modes.suggestions, vec!["creative".to_string()]);

    let gamerules = command_suggestions("/gamerule p", op);
    assert_eq!(gamerules.start, 10);
    assert_eq!(gamerules.length, 1);
    assert_eq!(
        gamerules.suggestions,
        vec!["players_sleeping_percentage".to_string()]
    );

    let status = command_suggestions("/st", op);
    assert_eq!(status.start, 1);
    assert_eq!(status.length, 2);
    assert_eq!(
        status.suggestions,
        vec!["status".to_string(), "stop".to_string()]
    );

    assert!(command_suggestions("/g", not_op).suggestions.is_empty());
}

#[test]
fn runtime_control_status_message_reports_disabled_and_drain_snapshot() {
    assert_eq!(
        runtime_control_status_message(None),
        "Runtime control: disabled"
    );

    let control = crate::RuntimeControlHandle::new(crate::RuntimeControlConfig {
        policy: crate::AutoscalePolicy {
            min_view_distance: 2,
            max_view_distance: 8,
            min_chunk_send_rate: 1,
            max_chunk_send_rate: 16,
            min_chunk_load_rate: 2,
            max_chunk_load_rate: 64,
            min_chunk_generate_rate: 3,
            max_chunk_generate_rate: 32,
            ..crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced)
        },
        initial_limits: crate::RuntimeControlLimits {
            view_distance: 8,
            chunk_send_rate: 16,
            chunk_load_rate: 64,
            chunk_generate_rate: 32,
        },
    });
    control.request_drain();

    assert_eq!(
        runtime_control_status_message(Some(&control)),
        "Runtime control: draining=true action=scale_down pressure=none limits=view_distance:2,send:1,load:2,generate:3 pressure_ticks=0 healthy_ticks=0 reason=drain requested; clamped to minimum chunk throughput"
    );
}

#[test]
fn local_dev_profiles_are_op_capable_for_now() {
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "op_probe".to_string(),
    };

    let permissions = crate::server::CommandPermissionConfig::new(Vec::<String>::new(), true)
        .permissions_for(&profile, "127.0.0.1:40000".parse().unwrap());

    assert!(permissions.can_change_game_mode());
    assert!(permissions.can_use_admin_commands());
}

#[test]
fn item_to_block_table_is_registry_derived() {
    use std::collections::BTreeMap;

    use mc_data::blocks::{BlockReport, BlockStateReport};
    use mc_data::items::ItemReport;

    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:dirt").unwrap(),
            protocol_id: 42,
        },
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:apple").unwrap(),
            protocol_id: 43,
        },
    ]);
    let blocks = mc_world::BlockRegistry::from_report(&[
        BlockReport {
            id: mc_data::Identifier::parse("minecraft:air").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 0,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: mc_data::Identifier::parse("minecraft:dirt").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 1,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
    ])
    .unwrap();

    let table = ItemToBlockTable::build(&items, &blocks);

    assert_eq!(table.resolve(42), Some(mc_world::BlockStateId(1)));
    assert_eq!(table.resolve(43), None);
}

#[test]
fn stonecutter_item_maps_to_placeable_stonecutter_block() {
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:stonecutter").unwrap(),
        protocol_id: 42,
    }]);
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:stonecutter"),
    ])
    .unwrap();

    assert_eq!(
        ItemToBlockTable::build(&items, &blocks).resolve(42),
        Some(mc_world::BlockStateId(1)),
    );
}

#[test]
fn item_to_block_table_maps_torch_item_to_standing_torch() {
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:torch").unwrap(),
        protocol_id: 44,
    }]);
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:dirt"),
        simple_block(2, "minecraft:torch"),
    ])
    .unwrap();
    let table = ItemToBlockTable::build(&items, &blocks);
    let dirt_state = blocks
        .block(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap()
        .default;

    assert_eq!(table.resolve(44), Some(mc_world::BlockStateId(2)));
    assert_eq!(
        table.resolve_for_use_on(&items, 44, dirt_state, Direction::Up, &blocks),
        Some(mc_world::BlockStateId(2))
    );
}

#[test]
fn wheat_seeds_place_wheat_on_farmland_only() {
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:wheat_seeds").unwrap(),
        protocol_id: 50,
    }]);
    let blocks = crop_test_registry();
    let table = ItemToBlockTable::build(&items, &blocks);
    let farmland = Identifier::parse("minecraft:farmland").unwrap();
    let farmland_state = blocks
        .by_name_and_props(&farmland, &[("moisture".to_string(), "0".to_string())])
        .unwrap();
    let dirt_state = blocks
        .block(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap()
        .default;

    assert_eq!(table.resolve(50), None);
    assert_eq!(
        table.resolve_for_use_on(&items, 50, farmland_state, Direction::Up, &blocks),
        Some(mc_world::BlockStateId(11))
    );
    assert_eq!(
        table.resolve_for_use_on(&items, 50, farmland_state, Direction::North, &blocks),
        None
    );
    assert_eq!(
        table.resolve_for_use_on(&items, 50, dirt_state, Direction::Up, &blocks),
        None
    );
}

#[test]
fn common_crop_items_place_on_their_required_soil_only() {
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: Identifier::parse("minecraft:carrot").unwrap(),
            protocol_id: 51,
        },
        ItemReport {
            id: Identifier::parse("minecraft:potato").unwrap(),
            protocol_id: 52,
        },
        ItemReport {
            id: Identifier::parse("minecraft:beetroot_seeds").unwrap(),
            protocol_id: 53,
        },
        ItemReport {
            id: Identifier::parse("minecraft:nether_wart").unwrap(),
            protocol_id: 54,
        },
    ]);
    let blocks = crop_test_registry();
    let table = ItemToBlockTable::build(&items, &blocks);
    let farmland = Identifier::parse("minecraft:farmland").unwrap();
    let farmland_state = blocks
        .by_name_and_props(&farmland, &[("moisture".to_string(), "0".to_string())])
        .unwrap();
    let soul_sand = blocks
        .block(&Identifier::parse("minecraft:soul_sand").unwrap())
        .unwrap()
        .default;

    assert_eq!(
        table.resolve_for_use_on(&items, 51, farmland_state, Direction::Up, &blocks),
        Some(mc_world::BlockStateId(20))
    );
    assert_eq!(
        table.resolve_for_use_on(&items, 52, farmland_state, Direction::Up, &blocks),
        Some(mc_world::BlockStateId(28))
    );
    assert_eq!(
        table.resolve_for_use_on(&items, 53, farmland_state, Direction::Up, &blocks),
        Some(mc_world::BlockStateId(36))
    );
    assert_eq!(
        table.resolve_for_use_on(&items, 54, soul_sand, Direction::Up, &blocks),
        Some(mc_world::BlockStateId(44))
    );
    assert_eq!(
        table.resolve_for_use_on(&items, 54, farmland_state, Direction::Up, &blocks),
        None
    );
}

#[test]
fn cocoa_beans_place_cocoa_on_jungle_log_sides() {
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:cocoa_beans").unwrap(),
        protocol_id: 58,
    }]);
    let blocks = crop_test_registry();
    let table = ItemToBlockTable::build(&items, &blocks);
    let jungle_log = blocks
        .block(&Identifier::parse("minecraft:jungle_log").unwrap())
        .unwrap()
        .default;
    let dirt = blocks
        .block(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap()
        .default;

    assert_eq!(
        table.resolve_for_use_on(&items, 58, jungle_log, Direction::North, &blocks),
        Some(mc_world::BlockStateId(60))
    );
    assert_eq!(
        table.resolve_for_use_on(&items, 58, jungle_log, Direction::Up, &blocks),
        None
    );
    assert_eq!(
        table.resolve_for_use_on(&items, 58, dirt, Direction::North, &blocks),
        None
    );
}

#[test]
fn sign_items_choose_floor_or_wall_sign_for_clicked_face() {
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:oak_sign").unwrap(),
        protocol_id: 70,
    }]);
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:oak_sign").unwrap(),
            properties: prop_schema(&[("rotation", &["0"])]),
            states: vec![state(1, true, &[("rotation", "0")])],
        },
        BlockReport {
            id: Identifier::parse("minecraft:oak_wall_sign").unwrap(),
            properties: prop_schema(&[("facing", &["north"])]),
            states: vec![state(2, true, &[("facing", "north")])],
        },
    ])
    .unwrap();

    assert_eq!(
        ItemToBlockTable::build(&items, &blocks).resolve_for_use_on(
            &items,
            70,
            mc_world::BlockStateId(0),
            Direction::Up,
            &blocks,
        ),
        Some(mc_world::BlockStateId(1))
    );
    assert_eq!(
        ItemToBlockTable::build(&items, &blocks).resolve_for_use_on(
            &items,
            70,
            mc_world::BlockStateId(0),
            Direction::North,
            &blocks,
        ),
        Some(mc_world::BlockStateId(2))
    );
    assert_eq!(
        ItemToBlockTable::build(&items, &blocks).resolve_for_use_on(
            &items,
            70,
            mc_world::BlockStateId(0),
            Direction::Down,
            &blocks,
        ),
        None
    );
}

#[test]
fn bucket_items_resolve_fluid_sources() {
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: Identifier::parse("minecraft:bucket").unwrap(),
            protocol_id: 60,
        },
        ItemReport {
            id: Identifier::parse("minecraft:water_bucket").unwrap(),
            protocol_id: 61,
        },
        ItemReport {
            id: Identifier::parse("minecraft:lava_bucket").unwrap(),
            protocol_id: 62,
        },
    ]);
    let blocks = fluid_test_registry();
    let table = ItemToBlockTable::build(&items, &blocks);

    assert_eq!(table.empty_bucket_item(), Some(60));
    assert_eq!(table.bucket_fluid_kind(61), Some(FluidKind::Water));
    assert_eq!(table.bucket_fluid_kind(62), Some(FluidKind::Lava));
    assert_eq!(
        table.fluid_source_state(FluidKind::Water),
        Some(BlockStateId(2))
    );
    assert_eq!(
        table.fluid_source_state(FluidKind::Lava),
        Some(BlockStateId(10))
    );
}

#[test]
fn bucket_replacement_updates_single_held_stack_only() {
    let mut inventory = PlayerInventory::empty();
    inventory
        .set_hotbar(
            0,
            ItemStack {
                item_id: 61,
                count: 1,
                damage: None,
                enchantments: Vec::new(),
                custom_name: None,
                item_model: None,
            },
        )
        .unwrap();

    let (next, changed) =
        plan_bucket_replacement(&inventory, PlayerInventory::HOTBAR_BASE, 60, 16).unwrap();

    assert_eq!(next.held(0).unwrap().item_id, 60);
    assert_eq!(next.held(0).unwrap().count, 1);
    assert_eq!(
        changed,
        vec![(PlayerInventory::HOTBAR_BASE, next.held(0).unwrap().clone())]
    );

    inventory
        .set_hotbar(
            0,
            ItemStack {
                item_id: 60,
                count: 2,
                damage: None,
                enchantments: Vec::new(),
                custom_name: None,
                item_model: None,
            },
        )
        .unwrap();
    let (next, changed) =
        plan_bucket_replacement(&inventory, PlayerInventory::HOTBAR_BASE, 61, 1).unwrap();
    assert_eq!(next.held(0).unwrap().item_id, 60);
    assert_eq!(next.held(0).unwrap().count, 1);
    assert_eq!(next.slots[9].item_id, 61);
    assert_eq!(next.slots[9].count, 1);
    assert_eq!(
        changed,
        vec![
            (PlayerInventory::HOTBAR_BASE, next.held(0).unwrap().clone(),),
            (9, next.slots[9].clone())
        ]
    );

    let mut full_inventory = inventory.clone();
    for slot in 9..=44 {
        if slot != PlayerInventory::HOTBAR_BASE {
            full_inventory.slots[slot] = ItemStack::new(99, 64);
        }
    }
    assert!(
        plan_bucket_replacement(&full_inventory, PlayerInventory::HOTBAR_BASE, 61, 1).is_none()
    );

    inventory.slots[45] = ItemStack {
        item_id: 60,
        count: 1,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
    };
    let (next, changed) = plan_bucket_replacement(&inventory, 45, 61, 1).unwrap();
    assert_eq!(next.slots[45].item_id, 61);
    assert_eq!(next.slots[45].count, 1);
    assert_eq!(changed, vec![(45, next.slots[45].clone())]);
}

#[tokio::test]
async fn bucket_precondition_reads_published_state_while_world_writer_is_held() {
    let state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    insert_fluid_test_chunk(&state).await;
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let expected_token = state.world.lock().await.block_mutation_token(pos).unwrap();
    let writer = state.world.lock().await;

    let precondition = published_block_precondition(&state, pos).unwrap();

    assert_eq!(precondition.pos, pos);
    assert_eq!(precondition.expected_state, BlockStateId(0));
    assert_eq!(precondition.expected_token, expected_token);
    drop(writer);
}

#[test]
fn fluid_tick_flows_sideways_when_blocked_below() {
    let facts = fluid_test_facts();
    let registry = Arc::new(fluid_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(2)).unwrap();
    world
        .set_block_at(mc_world::BlockPos { y: 63, ..pos }, BlockStateId(1))
        .unwrap();

    let edits = fluid_tick_edits(
        blocks,
        &facts,
        &world,
        pos,
        BlockStateId(2),
        facts.fluid(2).unwrap(),
    );

    assert_eq!(edits.len(), 4);
    assert!(edits.iter().all(|edit| edit.new_state == BlockStateId(3)));
}

#[test]
fn fluid_tick_does_not_materialize_neighbour_chunks() {
    struct CountingAirGenerator {
        calls: Arc<AtomicUsize>,
    }

    impl mc_world::ChunkGenerator for CountingAirGenerator {
        fn generate(&self, pos: ChunkPos) -> Chunk {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let mut chunk = Chunk::empty(
                pos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            );
            chunk.status = "minecraft:full".into();
            chunk.dirty = true;
            chunk
        }
    }

    let facts = fluid_test_facts();
    let registry = Arc::new(fluid_test_registry());
    let generated_chunks = Arc::new(AtomicUsize::new(0));
    let mut world = mc_world::WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 16)
        .with_generator(Arc::new(CountingAirGenerator {
            calls: Arc::clone(&generated_chunks),
        }));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let source = mc_world::BlockPos {
        x: 15,
        y: 64,
        z: 15,
    };
    world.set_block_at(source, BlockStateId(2)).unwrap();
    world
        .set_block_at(mc_world::BlockPos { y: 63, ..source }, BlockStateId(1))
        .unwrap();

    let _ = fluid_tick_edits(
        registry.as_ref(),
        &facts,
        &world,
        source,
        BlockStateId(2),
        facts.fluid(2).unwrap(),
    );

    assert_eq!(generated_chunks.load(Ordering::Relaxed), 0);
    assert_eq!(world.cache_len(), 1);
}

#[test]
fn unsupported_flow_decays_to_air() {
    let facts = fluid_test_facts();
    let registry = Arc::new(fluid_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(4)).unwrap();
    world
        .set_block_at(mc_world::BlockPos { y: 63, ..pos }, BlockStateId(1))
        .unwrap();

    let edits = fluid_tick_edits(
        blocks,
        &facts,
        &world,
        pos,
        BlockStateId(4),
        facts.fluid(4).unwrap(),
    );

    assert_eq!(
        edits,
        vec![BlockEdit {
            pos,
            new_state: BlockStateId(0)
        }]
    );
}

#[test]
fn removed_bucket_source_drains_own_spread_from_source_cell() {
    let facts = fluid_test_facts();
    let registry = Arc::new(fluid_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let source = mc_world::BlockPos { x: 8, y: 64, z: 8 };
    seed_fluid_test_floor(&mut world, 1..=15, source.y - 1, 1..=15);
    world.set_block_at(source, BlockStateId(2)).unwrap();

    for _ in 0..6 {
        run_fluid_test_step(blocks, &facts, &mut world, 1..=15, source.y, 1..=15);
    }

    world.set_block_at(source, BlockStateId(0)).unwrap();
    let mut source_refilled = false;
    for _ in 0..16 {
        run_fluid_test_step(blocks, &facts, &mut world, 1..=15, source.y, 1..=15);
        let state = world.get_block(source).unwrap().unwrap();
        if facts.fluid(state.0).is_some() {
            source_refilled = true;
            break;
        }
    }

    assert!(
        !source_refilled,
        "removed bucket source cell must not be repopulated by its own stale flowing water"
    );
}

fn seed_fluid_test_floor(
    world: &mut mc_world::WorldStorage,
    xs: std::ops::RangeInclusive<i32>,
    y: i32,
    zs: std::ops::RangeInclusive<i32>,
) {
    for x in xs {
        for z in zs.clone() {
            world
                .set_block_at(mc_world::BlockPos { x, y, z }, BlockStateId(1))
                .unwrap();
        }
    }
}

fn run_fluid_test_step(
    blocks: &mc_world::BlockRegistry,
    facts: &mc_data::block_facts::BlockFactsTable,
    world: &mut mc_world::WorldStorage,
    xs: std::ops::RangeInclusive<i32>,
    y: i32,
    zs: std::ops::RangeInclusive<i32>,
) {
    let mut positions = Vec::new();
    for x in xs {
        for z in zs.clone() {
            let pos = mc_world::BlockPos { x, y, z };
            if world
                .get_block(pos)
                .ok()
                .flatten()
                .is_some_and(|state| facts.fluid(state.0).is_some())
            {
                positions.push(pos);
            }
        }
    }

    let mut outcome = BlockEditBatchOutcome::default();
    for pos in positions {
        let Some(state) = world.get_cached_block(pos) else {
            continue;
        };
        let Some(fluid) = facts.fluid(state.0) else {
            continue;
        };
        for edit in fluid_tick_edits(blocks, facts, world, pos, state, fluid) {
            apply_block_edit_to_storage(world, None, &edit, &mut outcome);
        }
    }
}

#[test]
fn scheduling_fluid_edits_uses_current_tick_delay() {
    let facts = fluid_test_facts();
    let registry = Arc::new(fluid_test_registry());
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(2)).unwrap();

    schedule_fluid_ticks_near_applied(
        &mut world,
        &facts,
        100,
        &[AppliedBlockEdit {
            pos,
            previous: BlockStateId(0),
            new_state: BlockStateId(2),
        }],
    );

    let ticks = world.scheduled_fluid_ticks(cpos).unwrap().unwrap();
    assert_eq!(ticks.len(), 1);
    assert_eq!(ticks[0].pos, pos);
    assert_eq!(ticks[0].trigger_tick, 100 + WATER_FLOW_DELAY_TICKS);
}

#[tokio::test]
async fn interaction_fluid_scheduling_uses_shared_simulation_tick() {
    let mut state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    let (simulation, mut simulation_owner) = simulation_channel();
    state.simulation = simulation;
    state.block_facts = Arc::new(fluid_test_facts());
    insert_fluid_test_chunk(&state).await;
    state.sessions.advance_world_time(100);

    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    {
        let mut world = state.world.lock().await;
        world.set_block_at(pos, BlockStateId(2)).unwrap();
    }

    schedule_fluid_ticks_for_interaction(
        &state,
        &[AppliedBlockEdit {
            pos,
            previous: BlockStateId(0),
            new_state: BlockStateId(2),
        }],
    )
    .await;

    {
        let mut world = state.world.lock().await;
        assert!(
            world
                .scheduled_fluid_ticks(ChunkPos { x: 0, z: 0 })
                .unwrap()
                .unwrap()
                .is_empty(),
            "network task must not schedule fluid ticks directly"
        );
    }
    assert_eq!(
        simulation_owner
            .process_tick_with_world(&state.sessions, Some(&state.world), None, 1)
            .processed,
        1
    );

    let ticks = {
        let mut world = state.world.lock().await;
        world
            .scheduled_fluid_ticks(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .unwrap()
            .to_vec()
    };
    assert_eq!(ticks.len(), 1);
    assert_eq!(ticks[0].pos, pos);
    assert_eq!(ticks[0].trigger_tick, 100 + WATER_FLOW_DELAY_TICKS);
}

#[test]
fn water_lava_interactions_make_solid_blocks() {
    let facts = fluid_test_facts();
    let registry = Arc::new(fluid_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let water_pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let lava_source_pos = mc_world::BlockPos { x: 5, y: 64, z: 4 };
    world.set_block_at(water_pos, BlockStateId(2)).unwrap();
    world
        .set_block_at(lava_source_pos, BlockStateId(10))
        .unwrap();

    let edits = fluid_tick_edits(
        blocks,
        &facts,
        &world,
        water_pos,
        BlockStateId(2),
        facts.fluid(2).unwrap(),
    );
    assert_eq!(
        edits,
        vec![BlockEdit {
            pos: lava_source_pos,
            new_state: BlockStateId(14),
        }]
    );

    world
        .set_block_at(lava_source_pos, BlockStateId(0))
        .unwrap();
    let lava_flow_pos = mc_world::BlockPos { x: 4, y: 63, z: 4 };
    world.set_block_at(lava_flow_pos, BlockStateId(11)).unwrap();
    let edits = fluid_tick_edits(
        blocks,
        &facts,
        &world,
        lava_flow_pos,
        BlockStateId(11),
        facts.fluid(11).unwrap(),
    );
    assert_eq!(
        edits,
        vec![BlockEdit {
            pos: lava_flow_pos,
            new_state: BlockStateId(1),
        }]
    );
}

#[test]
fn falling_block_starts_when_support_edit_becomes_replaceable() {
    let facts = fluid_test_facts();
    let registry = Arc::new(fluid_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let sand = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let upper_sand = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    world.set_block_at(support, BlockStateId(1)).unwrap();
    world.set_block_at(sand, BlockStateId(16)).unwrap();
    world.set_block_at(upper_sand, BlockStateId(16)).unwrap();
    world.set_block_at(support, BlockStateId(0)).unwrap();

    let plan = plan_falling_block_starts(
        blocks,
        &facts,
        &world,
        &[AppliedBlockEdit {
            pos: support,
            previous: BlockStateId(1),
            new_state: BlockStateId(0),
        }],
        BlockStateId(0),
    );

    assert_eq!(
        plan.starts,
        vec![
            FallingBlockStart {
                pos: sand,
                state: BlockStateId(16),
            },
            FallingBlockStart {
                pos: upper_sand,
                state: BlockStateId(16),
            }
        ]
    );
}

#[tokio::test]
async fn falling_block_start_planning_does_not_wait_for_world_writer() {
    let blocks = Arc::new(fluid_test_registry());
    let mut state = interaction_state_for_blocks(Arc::clone(&blocks));
    state.block_facts = Arc::new(fluid_test_facts());
    state.entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    insert_fluid_test_chunk(&state).await;
    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let sand = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    state
        .world
        .lock()
        .await
        .set_block_at(sand, mc_world::BlockStateId(16))
        .expect("place falling sand");
    let applied = [AppliedBlockEdit {
        pos: support,
        previous: mc_world::BlockStateId(1),
        new_state: mc_world::BlockStateId(0),
    }];
    let world = Arc::clone(&state.world);
    let world_writer = world.lock().await;
    let mut writer = tokio::io::sink();
    FALLING_BLOCK_START_PLANNING_COMPLETION_COUNT.with(|count| count.set(0));
    let mut pass = Box::pin(start_falling_blocks_after_edits(
        &mut state,
        &mut writer,
        &applied,
    ));

    let pending = std::future::poll_fn(|cx| {
        Poll::Ready(matches!(Future::poll(pass.as_mut(), cx), Poll::Pending))
    })
    .await;
    assert!(
        pending,
        "falling-block removal commit must wait for the writer"
    );
    FALLING_BLOCK_START_PLANNING_COMPLETION_COUNT.with(|count| {
        assert_eq!(
            count.get(),
            1,
            "falling-block column discovery must finish before waiting for the writer"
        );
    });

    drop(world_writer);
    pass.await.expect("falling block starts");
    assert_eq!(
        world.lock().await.get_cached_block(sand),
        Some(mc_world::BlockStateId(0))
    );

    world
        .lock()
        .await
        .set_block_at(sand, mc_world::BlockStateId(16))
        .expect("place replacement falling sand");
    let entities_before = state.sessions.pressure_snapshot().server_entities;
    let mut world_writer = world.lock().await;
    FALLING_BLOCK_START_PLANNING_COMPLETION_COUNT.with(|count| count.set(0));
    let mut stale_pass = Box::pin(start_falling_blocks_after_edits(
        &mut state,
        &mut writer,
        &applied,
    ));
    let pending = std::future::poll_fn(|cx| {
        Poll::Ready(matches!(
            Future::poll(stale_pass.as_mut(), cx),
            Poll::Pending
        ))
    })
    .await;
    assert!(
        pending,
        "stale falling-block commit must wait for the writer"
    );
    FALLING_BLOCK_START_PLANNING_COMPLETION_COUNT.with(|count| assert_eq!(count.get(), 1));
    world_writer
        .set_block_at(sand, mc_world::BlockStateId(1))
        .expect("replace planned falling block");
    drop(world_writer);
    stale_pass.await.expect("stale falling start is rejected");
    assert_eq!(
        world.lock().await.get_cached_block(sand),
        Some(mc_world::BlockStateId(1))
    );
    assert_eq!(
        state.sessions.pressure_snapshot().server_entities,
        entities_before
    );
}

#[tokio::test]
async fn falling_block_landing_on_solid_drops_item_and_despawns_entity() {
    let blocks = Arc::new(fluid_test_registry());
    let mut storage = in_memory_button_world(Arc::clone(&blocks));
    let landing_pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    storage
        .set_block_at(landing_pos, mc_world::BlockStateId(1))
        .expect("place occupied landing block");
    let world_read = storage.read_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:sand").unwrap(),
        protocol_id: 42,
    }]));
    let entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    let config = ServerConfig {
        blocks: Arc::clone(&blocks),
        world: Some(Arc::clone(&world)),
        items,
        entity_types,
        block_facts: Arc::new(fluid_test_facts()),
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(46),
        name: "FallingBlockViewer".to_string(),
    };
    let (tx, mut rx) = mpsc::channel(16);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(4.5, 64.0, 4.5),
    );
    let mut setup_dispatches = sessions.mark_loaded(session_id, (0, 0));

    let falling_spawn =
        sessions.spawn_falling_block(70, Vec3::new(4.5, 65.0, 4.5), mc_world::BlockStateId(16));
    let falling_id = falling_spawn
        .iter()
        .find_map(|dispatch| match &dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .expect("falling block spawn dispatch");
    setup_dispatches.extend(falling_spawn);
    dispatch_and_clear_setup_packets(setup_dispatches, &mut [&mut rx]);

    let (_simulation, owner) = simulation_channel();
    let applied = owner
        .land_falling_blocks(
            &config,
            &sessions,
            Some(&world_read),
            &[LandedFallingBlock {
                id: falling_id,
                pos: landing_pos,
                state: mc_world::BlockStateId(16),
            }],
        )
        .await;

    assert_eq!(applied, 0);
    assert_eq!(
        world.lock().await.get_cached_block(landing_pos),
        Some(mc_world::BlockStateId(1))
    );
    assert!(sessions.server_entity_snapshot(falling_id).is_none());

    let item_spawn = rx
        .try_recv()
        .expect("blocked falling block should spawn item drop");
    assert!(matches!(
        item_spawn,
        OutboundCommand::SpawnEntity(ServerEntitySnapshot {
            type_name,
            item_stack: Some(stack),
            ..
        }) if type_name == "minecraft:item" && stack == EntityItemStack::new(42, 1)
    ));
    assert!(matches!(
        rx.try_recv(),
        Ok(OutboundCommand::DespawnEntity(entity)) if entity.id == falling_id
    ));
}

#[tokio::test]
async fn falling_block_landing_planning_does_not_wait_for_world_writer() {
    let blocks = Arc::new(fluid_test_registry());
    let storage = in_memory_button_world(Arc::clone(&blocks));
    let world_read = storage.read_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        blocks,
        world: Some(Arc::clone(&world)),
        block_facts: Arc::new(fluid_test_facts()),
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let landing_pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let candidate = LandedFallingBlock {
        id: EntityId(99),
        pos: landing_pos,
        state: mc_world::BlockStateId(16),
    };
    let (_simulation, owner) = simulation_channel();
    let world_writer = world.lock().await;
    FALLING_BLOCK_LANDING_PLANNING_COMPLETION_COUNT.with(|count| count.set(0));
    let candidates = [candidate];
    let mut pass =
        Box::pin(owner.land_falling_blocks(&config, &sessions, Some(&world_read), &candidates));

    let pending = std::future::poll_fn(|cx| {
        Poll::Ready(matches!(Future::poll(pass.as_mut(), cx), Poll::Pending))
    })
    .await;
    assert!(
        pending,
        "falling-block landing commit must wait for the writer"
    );
    FALLING_BLOCK_LANDING_PLANNING_COMPLETION_COUNT.with(|count| {
        assert_eq!(
            count.get(),
            1,
            "falling-block landing planning must finish before waiting for the writer"
        );
    });

    drop(world_writer);
    assert_eq!(pass.await, 1);
    assert_eq!(
        world.lock().await.get_cached_block(landing_pos),
        Some(mc_world::BlockStateId(16))
    );
    assert_eq!(
        world_read.get_cached_block(landing_pos),
        Some(mc_world::BlockStateId(16))
    );
}

#[tokio::test]
async fn stale_falling_block_landing_plan_keeps_entity_and_replacement() {
    let blocks = Arc::new(fluid_test_registry());
    let storage = in_memory_button_world(Arc::clone(&blocks));
    let world_read = storage.read_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        blocks,
        world: Some(Arc::clone(&world)),
        block_facts: Arc::new(fluid_test_facts()),
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(47),
        name: "StaleFallingBlock".to_string(),
    };
    let (tx, mut rx) = mpsc::channel(16);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(4.5, 64.0, 4.5),
    );
    let _ = sessions.mark_loaded(session_id, (0, 0));
    let falling_id = sessions
        .spawn_falling_block(70, Vec3::new(4.5, 65.0, 4.5), mc_world::BlockStateId(16))
        .into_iter()
        .find_map(|dispatch| match dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .expect("falling block spawn dispatch");
    while rx.try_recv().is_ok() {}

    let landing_pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let candidates = [LandedFallingBlock {
        id: falling_id,
        pos: landing_pos,
        state: mc_world::BlockStateId(16),
    }];
    let (_simulation, owner) = simulation_channel();
    let mut world_writer = world.lock().await;
    FALLING_BLOCK_LANDING_PLANNING_COMPLETION_COUNT.with(|count| count.set(0));
    let mut pass =
        Box::pin(owner.land_falling_blocks(&config, &sessions, Some(&world_read), &candidates));
    let pending = std::future::poll_fn(|cx| {
        Poll::Ready(matches!(Future::poll(pass.as_mut(), cx), Poll::Pending))
    })
    .await;
    assert!(
        pending,
        "falling-block landing commit must wait for the writer"
    );
    FALLING_BLOCK_LANDING_PLANNING_COMPLETION_COUNT.with(|count| assert_eq!(count.get(), 1));
    world_writer
        .set_block_at(landing_pos, mc_world::BlockStateId(1))
        .expect("replace landing cell after snapshot planning");
    drop(world_writer);

    assert_eq!(pass.await, 0);
    assert_eq!(
        world.lock().await.get_cached_block(landing_pos),
        Some(mc_world::BlockStateId(1))
    );
    assert!(sessions.server_entity_snapshot(falling_id).is_some());
    assert!(rx.try_recv().is_err());
}

#[test]
fn cactus_column_cascades_when_support_breaks() {
    let registry = Arc::new(fluid_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let cactus_1 = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let cactus_2 = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    world.set_block_at(support, BlockStateId(1)).unwrap();
    world.set_block_at(cactus_1, BlockStateId(19)).unwrap();
    world.set_block_at(cactus_2, BlockStateId(19)).unwrap();

    let edits = plan_break_block_edits(
        blocks,
        &world,
        support,
        BlockStateId(1),
        BlockStateId(0),
        BlockStateId(0),
    );

    assert_eq!(
        edits,
        vec![
            BlockEdit {
                pos: support,
                new_state: BlockStateId(0),
            },
            BlockEdit {
                pos: cactus_1,
                new_state: BlockStateId(0),
            },
            BlockEdit {
                pos: cactus_2,
                new_state: BlockStateId(0),
            },
        ]
    );
}

#[test]
fn cactus_column_cascades_when_solid_side_neighbor_is_placed() {
    let registry = Arc::new(fluid_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let placed = mc_world::BlockPos { x: 5, y: 65, z: 4 };
    let cactus_1 = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let cactus_2 = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    world.set_block_at(cactus_1, BlockStateId(19)).unwrap();
    world.set_block_at(cactus_2, BlockStateId(19)).unwrap();
    let mut edits = vec![BlockEdit {
        pos: placed,
        new_state: BlockStateId(1),
    }];
    let snapshot = world.read_view().snapshot_chunks(&[cpos]);

    append_cactus_side_neighbor_cascades(
        blocks,
        &snapshot,
        &mut edits,
        placed,
        BlockStateId(1),
        BlockStateId(0),
    );

    assert_eq!(
        edits,
        vec![
            BlockEdit {
                pos: placed,
                new_state: BlockStateId(1),
            },
            BlockEdit {
                pos: cactus_1,
                new_state: BlockStateId(0),
            },
            BlockEdit {
                pos: cactus_2,
                new_state: BlockStateId(0),
            },
        ]
    );
}

#[test]
fn cactus_column_does_not_cascade_when_cactus_side_neighbor_is_placed() {
    let registry = Arc::new(fluid_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let placed = mc_world::BlockPos { x: 5, y: 65, z: 4 };
    let cactus_1 = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let cactus_2 = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    world.set_block_at(cactus_1, BlockStateId(19)).unwrap();
    world.set_block_at(cactus_2, BlockStateId(19)).unwrap();
    let mut edits = vec![BlockEdit {
        pos: placed,
        new_state: BlockStateId(19),
    }];
    let snapshot = world.read_view().snapshot_chunks(&[cpos]);

    append_cactus_side_neighbor_cascades(
        blocks,
        &snapshot,
        &mut edits,
        placed,
        BlockStateId(19),
        BlockStateId(0),
    );

    assert_eq!(
        edits,
        vec![BlockEdit {
            pos: placed,
            new_state: BlockStateId(19),
        }]
    );
}

#[tokio::test]
async fn cactus_placement_path_cascades_when_solid_side_neighbor_is_placed() {
    let state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    insert_fluid_test_chunk(&state).await;
    let placed = mc_world::BlockPos { x: 5, y: 65, z: 4 };
    let cactus_1 = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let cactus_2 = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    {
        let mut world = state.world.lock().await;
        world.set_block_at(cactus_1, BlockStateId(19)).unwrap();
        world.set_block_at(cactus_2, BlockStateId(19)).unwrap();
    }

    let plan = plan_place_block_edits(
        &state,
        placed,
        BlockStateId(1),
        PlayerPose::new(0.5, 64.0, 0.5),
        Direction::Up,
        0.5,
    )
    .await;

    let plan = plan.expect("dirt placement plan");
    assert_eq!(
        plan.edits,
        vec![
            BlockEdit {
                pos: placed,
                new_state: BlockStateId(1),
            },
            BlockEdit {
                pos: cactus_1,
                new_state: BlockStateId(0),
            },
            BlockEdit {
                pos: cactus_2,
                new_state: BlockStateId(0),
            },
        ]
    );
    for cactus in [cactus_1, cactus_2] {
        let precondition = plan
            .additional_preconditions
            .iter()
            .find(|precondition| precondition.pos == cactus)
            .expect("every cascaded cactus is fenced by its exact source state");
        assert_eq!(precondition.expected_state, BlockStateId(19));
    }
}

#[tokio::test]
async fn cactus_placement_path_does_not_cascade_for_non_solid_side_neighbor() {
    let state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    insert_fluid_test_chunk(&state).await;
    let placed = mc_world::BlockPos { x: 5, y: 65, z: 4 };
    let cactus_1 = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let cactus_2 = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    {
        let mut world = state.world.lock().await;
        world.set_block_at(cactus_1, BlockStateId(19)).unwrap();
        world.set_block_at(cactus_2, BlockStateId(19)).unwrap();
        world
            .set_block_at(mc_world::BlockPos { x: 5, y: 64, z: 4 }, BlockStateId(16))
            .unwrap();
    }

    let plan = plan_place_block_edits(
        &state,
        placed,
        BlockStateId(20),
        PlayerPose::new(0.5, 64.0, 0.5),
        Direction::Up,
        0.5,
    )
    .await;

    let plan = plan.expect("non-solid placement plan");
    assert_eq!(
        plan.edits,
        vec![BlockEdit {
            pos: placed,
            new_state: BlockStateId(20),
        }]
    );
}

#[tokio::test]
async fn vertical_plant_placement_rejects_stone_support() {
    let state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    insert_fluid_test_chunk(&state).await;
    let targets = [
        (mc_world::BlockPos { x: 3, y: 65, z: 4 }, BlockStateId(19)),
        (mc_world::BlockPos { x: 6, y: 65, z: 4 }, BlockStateId(20)),
        (mc_world::BlockPos { x: 9, y: 65, z: 4 }, BlockStateId(21)),
    ];
    {
        let mut world = state.world.lock().await;
        for (target, _) in targets {
            world
                .set_block_at(mc_world::BlockPos { y: 64, ..target }, BlockStateId(1))
                .unwrap();
        }
        world
            .set_block_at(mc_world::BlockPos { x: 10, y: 64, z: 4 }, BlockStateId(2))
            .unwrap();
    }

    for (target, plant) in targets {
        assert_eq!(
            plan_place_block_edits(
                &state,
                target,
                plant,
                PlayerPose::new(0.5, 64.0, 0.5),
                Direction::Up,
                0.5,
            )
            .await,
            None
        );
    }
}

#[tokio::test]
async fn invalid_support_placement_resyncs_without_mutating_or_debiting_inventory() {
    let blocks = Arc::new(fluid_test_registry());
    let cactus = Identifier::parse("minecraft:cactus").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: cactus,
        protocol_id: 42,
    }]));
    let mut state = interaction_state_for_blocks(Arc::clone(&blocks));
    state.items = Arc::clone(&items);
    state.item_to_block = ItemToBlockTable::build(&items, &blocks);
    *state.inventory.held_mut(0).unwrap() = ItemStack::new(42, 1);
    insert_fluid_test_chunk(&state).await;

    let clicked = mc_world::BlockPos { x: 3, y: 64, z: 4 };
    let target = mc_world::BlockPos { y: 65, ..clicked };
    state
        .world
        .lock()
        .await
        .set_block_at(clicked, BlockStateId(1))
        .unwrap();
    let action = test_use_item_on(pack_block_pos(clicked.x, clicked.y, clicked.z));
    let mut writer = Vec::new();

    handle_block_item_placement(
        &mut state,
        &mut writer,
        None,
        GameMode::Survival,
        PlayerPose::new(3.5, 64.0, 4.5),
        clicked,
        &action,
        (clicked.x, clicked.y, clicked.z),
    )
    .await
    .unwrap();

    assert_eq!(state.inventory.held(0), Some(&ItemStack::new(42, 1)));
    assert_eq!(
        state.world.lock().await.get_cached_block(target),
        Some(BlockStateId(0))
    );

    let mut frames = bytes::BytesMut::from(writer.as_slice());
    let mut updates = Vec::new();
    let mut saw_held_resync = false;
    let mut saw_ack = false;
    while let Some(mut frame) =
        mc_protocol::frame::try_decode_frame(&mut frames, Compression::Disabled).unwrap()
    {
        if frame.id == BlockUpdate::ID {
            updates.push(BlockUpdate::decode(&mut frame.body).unwrap());
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let packet = ClientboundContainerSetSlot::decode(&mut frame.body).unwrap();
            assert_eq!(packet.container_id, 0);
            assert_eq!(packet.slot, PlayerInventory::HOTBAR_BASE as i16);
            assert_eq!(packet.item_stack, ItemStack::new(42, 1));
            saw_held_resync = true;
        } else if frame.id == BlockChangedAck::ID {
            assert!(!updates.is_empty(), "ack must follow block resyncs");
            assert!(
                saw_held_resync,
                "ack must follow the unchanged held-stack resync"
            );
            assert_eq!(
                BlockChangedAck::decode(&mut frame.body).unwrap().sequence,
                action.sequence
            );
            saw_ack = true;
        } else {
            panic!("unexpected packet during invalid support placement rejection");
        }
    }

    assert!(
        saw_ack,
        "invalid support placement must acknowledge the action"
    );
    assert!(
        saw_held_resync,
        "invalid support placement must resync the unchanged held stack"
    );
    assert_eq!(updates.len(), 2);
    assert!(updates.iter().any(|update| {
        unpack_block_pos(update.position) == (clicked.x, clicked.y, clicked.z)
            && update.state_id == 1
    }));
    assert!(updates.iter().any(|update| {
        unpack_block_pos(update.position) == (target.x, target.y, target.z) && update.state_id == 0
    }));
}

#[tokio::test]
async fn cactus_placement_path_rejects_adjacent_cactus() {
    let state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    insert_fluid_test_chunk(&state).await;
    let placed = mc_world::BlockPos { x: 5, y: 65, z: 4 };
    {
        let mut world = state.world.lock().await;
        world
            .set_block_at(mc_world::BlockPos { x: 4, y: 65, z: 4 }, BlockStateId(19))
            .unwrap();
    }

    let plan = plan_place_block_edits(
        &state,
        placed,
        BlockStateId(19),
        PlayerPose::new(0.5, 64.0, 0.5),
        Direction::Up,
        0.5,
    )
    .await;

    assert_eq!(plan, None);
}

#[test]
fn cactus_random_tick_grows_on_sand_to_height_three() {
    let registry = Arc::new(fluid_test_registry());
    let facts = fluid_test_facts();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:desert").unwrap(),
            ),
        )
        .unwrap();

    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let cactus_1 = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let cactus_2 = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    let cactus_3 = mc_world::BlockPos { x: 4, y: 67, z: 4 };
    world.set_block_at(support, BlockStateId(16)).unwrap();
    world.set_block_at(cactus_1, BlockStateId(19)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cactus_1,
            BlockStateId(19),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        Some(vec![BlockEdit {
            pos: cactus_2,
            new_state: BlockStateId(19),
        }])
    );
    world.set_block_at(cactus_2, BlockStateId(19)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cactus_2,
            BlockStateId(19),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        Some(vec![BlockEdit {
            pos: cactus_3,
            new_state: BlockStateId(19),
        }])
    );
    world.set_block_at(cactus_3, BlockStateId(19)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cactus_1,
            BlockStateId(19),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );
}

#[test]
fn cactus_random_tick_unsupported_or_obstructed_columns_are_noop() {
    let registry = Arc::new(fluid_test_registry());
    let facts = fluid_test_facts();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:desert").unwrap(),
            ),
        )
        .unwrap();

    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let cactus = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let above = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    let side = mc_world::BlockPos { x: 5, y: 66, z: 4 };
    world.set_block_at(cactus, BlockStateId(19)).unwrap();
    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cactus,
            BlockStateId(19),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );

    world.set_block_at(support, BlockStateId(1)).unwrap();
    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cactus,
            BlockStateId(19),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None,
        "stone is not a vanilla cactus support"
    );
    world.set_block_at(support, BlockStateId(16)).unwrap();
    world.set_block_at(above, BlockStateId(1)).unwrap();
    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cactus,
            BlockStateId(19),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );

    world.set_block_at(above, BlockStateId(0)).unwrap();
    world.set_block_at(side, BlockStateId(1)).unwrap();
    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cactus,
            BlockStateId(19),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );
}

#[test]
fn sugar_cane_random_tick_grows_on_sand_beside_water_to_height_three() {
    let registry = Arc::new(fluid_test_registry());
    let facts = fluid_test_facts();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();

    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let water = mc_world::BlockPos { x: 5, y: 64, z: 4 };
    let cane_1 = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let cane_2 = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    let cane_3 = mc_world::BlockPos { x: 4, y: 67, z: 4 };
    world.set_block_at(support, BlockStateId(16)).unwrap();
    world.set_block_at(water, BlockStateId(2)).unwrap();
    world.set_block_at(cane_1, BlockStateId(21)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cane_1,
            BlockStateId(21),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        Some(vec![BlockEdit {
            pos: cane_2,
            new_state: BlockStateId(21),
        }])
    );
    world.set_block_at(cane_2, BlockStateId(21)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cane_2,
            BlockStateId(21),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        Some(vec![BlockEdit {
            pos: cane_3,
            new_state: BlockStateId(21),
        }])
    );
    world.set_block_at(cane_3, BlockStateId(21)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cane_2,
            BlockStateId(21),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );
}

#[test]
fn sugar_cane_random_tick_unsupported_or_obstructed_columns_are_noop() {
    let registry = Arc::new(fluid_test_registry());
    let facts = fluid_test_facts();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();

    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let water = mc_world::BlockPos { x: 5, y: 64, z: 4 };
    let cane = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let above = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    world.set_block_at(cane, BlockStateId(21)).unwrap();
    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cane,
            BlockStateId(21),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );

    world.set_block_at(support, BlockStateId(1)).unwrap();
    world.set_block_at(water, BlockStateId(2)).unwrap();
    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cane,
            BlockStateId(21),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None,
        "water does not make stone a vanilla sugar-cane support"
    );

    world.set_block_at(support, BlockStateId(16)).unwrap();
    world.set_block_at(above, BlockStateId(1)).unwrap();
    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cane,
            BlockStateId(21),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );
}

#[test]
fn bamboo_column_cascades_when_support_breaks() {
    let registry = Arc::new(fluid_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let bamboo_1 = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let bamboo_2 = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    world.set_block_at(support, BlockStateId(1)).unwrap();
    world.set_block_at(bamboo_1, BlockStateId(20)).unwrap();
    world.set_block_at(bamboo_2, BlockStateId(20)).unwrap();

    let edits = plan_break_block_edits(
        blocks,
        &world,
        support,
        BlockStateId(1),
        BlockStateId(0),
        BlockStateId(0),
    );

    assert_eq!(
        edits,
        vec![
            BlockEdit {
                pos: support,
                new_state: BlockStateId(0),
            },
            BlockEdit {
                pos: bamboo_1,
                new_state: BlockStateId(0),
            },
            BlockEdit {
                pos: bamboo_2,
                new_state: BlockStateId(0),
            },
        ]
    );
}

#[test]
fn bamboo_random_tick_grows_on_sand_until_height_sixteen() {
    let registry = Arc::new(fluid_test_registry());
    let facts = fluid_test_facts();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:bamboo_jungle").unwrap(),
            ),
        )
        .unwrap();

    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let bamboo_1 = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let bamboo_2 = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    let bamboo_3 = mc_world::BlockPos { x: 4, y: 67, z: 4 };
    let bamboo_4 = mc_world::BlockPos { x: 4, y: 68, z: 4 };
    world.set_block_at(support, BlockStateId(16)).unwrap();
    world.set_block_at(bamboo_1, BlockStateId(20)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            bamboo_1,
            BlockStateId(20),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        Some(vec![BlockEdit {
            pos: bamboo_2,
            new_state: BlockStateId(20),
        }])
    );
    world.set_block_at(bamboo_2, BlockStateId(20)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            bamboo_2,
            BlockStateId(20),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        Some(vec![BlockEdit {
            pos: bamboo_3,
            new_state: BlockStateId(20),
        }])
    );
    world.set_block_at(bamboo_3, BlockStateId(20)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            bamboo_3,
            BlockStateId(20),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        Some(vec![BlockEdit {
            pos: bamboo_4,
            new_state: BlockStateId(20),
        }])
    );

    for y in bamboo_4.y..=80 {
        world
            .set_block_at(mc_world::BlockPos { y, ..bamboo_4 }, BlockStateId(20))
            .unwrap();
    }
    let bamboo_16 = mc_world::BlockPos { y: 80, ..bamboo_4 };
    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            bamboo_16,
            BlockStateId(20),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );
}

#[test]
fn bamboo_random_tick_rejects_stone_support() {
    let registry = Arc::new(fluid_test_registry());
    let facts = fluid_test_facts();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let bamboo = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    world.set_block_at(support, BlockStateId(1)).unwrap();
    world.set_block_at(bamboo, BlockStateId(20)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            bamboo,
            BlockStateId(20),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );
}

#[test]
fn lower_vertical_plant_segments_do_not_grow_the_column_top() {
    let registry = Arc::new(fluid_test_registry());
    let facts = fluid_test_facts();
    for (state, support_state) in [
        (BlockStateId(19), BlockStateId(16)),
        (BlockStateId(20), BlockStateId(16)),
        (BlockStateId(21), BlockStateId(16)),
    ] {
        let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
        let cpos = ChunkPos { x: 0, z: 0 };
        world
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
        let bottom = mc_world::BlockPos { x: 4, y: 65, z: 4 };
        let top = mc_world::BlockPos { x: 4, y: 66, z: 4 };
        world.set_block_at(support, support_state).unwrap();
        if state == BlockStateId(21) {
            world
                .set_block_at(mc_world::BlockPos { x: 5, ..support }, BlockStateId(2))
                .unwrap();
        }
        world.set_block_at(bottom, state).unwrap();
        world.set_block_at(top, state).unwrap();

        assert_eq!(
            random_tick_edit(
                registry.as_ref(),
                &facts,
                &world,
                bottom,
                state,
                mc_data::block_facts::RandomTickFamily::Crop,
            ),
            None
        );
    }
}

#[test]
fn bamboo_random_tick_builds_vanilla_age_and_leaf_crown() {
    let registry = Arc::new(bamboo_test_registry());
    let facts = mc_data::block_facts::BlockFactsTable::default();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:bamboo_jungle").unwrap(),
            ),
        )
        .unwrap();
    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let bottom = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    world.set_block_at(support, BlockStateId(1)).unwrap();
    world
        .set_block_at(bottom, bamboo_state(&registry, "0", "none", "0"))
        .unwrap();

    for top_y in 65..=67 {
        let top = mc_world::BlockPos { y: top_y, ..bottom };
        let state = world.get_block(top).unwrap().unwrap();
        let edits = random_tick_edit_seeded(
            registry.as_ref(),
            &facts,
            &world,
            top,
            state,
            mc_data::block_facts::RandomTickFamily::Crop,
            0,
        )
        .expect("successful bamboo growth");
        for edit in edits {
            world.set_block_at(edit.pos, edit.new_state).unwrap();
        }
    }

    assert_eq!(
        (65..=68)
            .map(|y| {
                let state = world
                    .get_block(mc_world::BlockPos { y, ..bottom })
                    .unwrap()
                    .unwrap();
                let state = registry.by_id(state).unwrap();
                (
                    block_state_property(state, "age").unwrap().to_string(),
                    block_state_property(state, "leaves").unwrap().to_string(),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            ("1".into(), "none".into()),
            ("1".into(), "small".into()),
            ("1".into(), "small".into()),
            ("1".into(), "large".into()),
        ]
    );
}

#[test]
fn bamboo_random_tick_uses_one_in_three_growth_chance() {
    let registry = Arc::new(bamboo_test_registry());
    let facts = mc_data::block_facts::BlockFactsTable::default();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let pos = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    world
        .set_block_at(mc_world::BlockPos { y: 64, ..pos }, BlockStateId(1))
        .unwrap();
    let state = bamboo_state(&registry, "0", "none", "0");
    world.set_block_at(pos, state).unwrap();

    assert_eq!(
        random_tick_edit_seeded(
            registry.as_ref(),
            &facts,
            &world,
            pos,
            state,
            mc_data::block_facts::RandomTickFamily::Crop,
            1,
        ),
        None
    );
    assert!(
        random_tick_edit_seeded(
            registry.as_ref(),
            &facts,
            &world,
            pos,
            state,
            mc_data::block_facts::RandomTickFamily::Crop,
            3,
        )
        .is_some()
    );
}

#[test]
fn bamboo_sapling_random_tick_creates_two_exact_segments() {
    let registry = Arc::new(bamboo_test_registry());
    let facts = mc_data::block_facts::BlockFactsTable::default();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:bamboo_jungle").unwrap(),
            ),
        )
        .unwrap();
    let pos = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    world.set_block_at(pos, BlockStateId(2)).unwrap();

    assert_eq!(
        random_tick_edit_seeded(
            registry.as_ref(),
            &facts,
            &world,
            pos,
            BlockStateId(2),
            mc_data::block_facts::RandomTickFamily::Sapling,
            1,
        ),
        None
    );

    assert_eq!(
        random_tick_edit_seeded(
            registry.as_ref(),
            &facts,
            &world,
            pos,
            BlockStateId(2),
            mc_data::block_facts::RandomTickFamily::Sapling,
            0,
        ),
        Some(vec![
            BlockEdit {
                pos,
                new_state: bamboo_state(&registry, "0", "none", "0"),
            },
            BlockEdit {
                pos: mc_world::BlockPos { y: 66, ..pos },
                new_state: bamboo_state(&registry, "0", "small", "0"),
            },
        ])
    );
}

#[test]
fn bamboo_random_tick_marks_the_sixteenth_segment_terminal() {
    let registry = Arc::new(bamboo_test_registry());
    let facts = mc_data::block_facts::BlockFactsTable::default();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:bamboo_jungle").unwrap(),
            ),
        )
        .unwrap();
    let bottom = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let growing = bamboo_state(&registry, "0", "none", "0");
    for y in 65..=79 {
        world
            .set_block_at(mc_world::BlockPos { y, ..bottom }, growing)
            .unwrap();
    }

    let top = mc_world::BlockPos { y: 79, ..bottom };
    let edits = random_tick_edit_seeded(
        registry.as_ref(),
        &facts,
        &world,
        top,
        growing,
        mc_data::block_facts::RandomTickFamily::Crop,
        0,
    )
    .expect("height-fifteen bamboo growth");
    let terminal_pos = mc_world::BlockPos { y: 80, ..bottom };
    assert_eq!(
        edits.last(),
        Some(&BlockEdit {
            pos: terminal_pos,
            new_state: bamboo_state(&registry, "1", "small", "1"),
        })
    );
    for edit in edits {
        world.set_block_at(edit.pos, edit.new_state).unwrap();
    }
    let terminal = world.get_block(terminal_pos).unwrap().unwrap();
    assert_eq!(
        random_tick_edit_seeded(
            registry.as_ref(),
            &facts,
            &world,
            terminal_pos,
            terminal,
            mc_data::block_facts::RandomTickFamily::Crop,
            3,
        ),
        None
    );
}

#[test]
fn crop_random_tick_advances_supported_age_crops_until_mature() {
    let blocks = crop_test_registry();

    for (crop, first_state) in [
        ("minecraft:wheat", 11),
        ("minecraft:carrots", 20),
        ("minecraft:potatoes", 28),
        ("minecraft:beetroots", 36),
        ("minecraft:nether_wart", 44),
    ] {
        let crop = Identifier::parse(crop).unwrap();
        assert_eq!(
            next_crop_growth_state(&blocks, mc_world::BlockStateId(first_state)),
            Some(mc_world::BlockStateId(first_state + 1)),
            "{crop} age 0 should advance"
        );
        assert_eq!(
            next_crop_growth_state(&blocks, mc_world::BlockStateId(first_state + 6)),
            Some(mc_world::BlockStateId(first_state + 7)),
            "{crop} age 6 should advance"
        );
        assert_eq!(
            next_crop_growth_state(&blocks, mc_world::BlockStateId(first_state + 7)),
            None,
            "{crop} max age should not advance"
        );
    }

    assert_eq!(
        next_crop_growth_state(&blocks, mc_world::BlockStateId(1)),
        None
    );
    assert_eq!(
        next_crop_growth_state(&blocks, mc_world::BlockStateId(0)),
        None
    );
}

#[test]
fn farmland_random_tick_does_not_materialize_neighbour_chunks() {
    struct CountingAirGenerator {
        calls: Arc<AtomicUsize>,
    }

    impl mc_world::ChunkGenerator for CountingAirGenerator {
        fn generate(&self, pos: ChunkPos) -> Chunk {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let mut chunk = Chunk::empty(
                pos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            );
            chunk.status = "minecraft:full".into();
            chunk.dirty = true;
            chunk
        }
    }

    let reports = vec![
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:farmland").unwrap(),
            properties: prop_schema(&[("moisture", &["0", "1"])]),
            states: vec![
                state(1, true, &[("moisture", "0")]),
                state(2, false, &[("moisture", "1")]),
            ],
        },
        simple_block(3, "minecraft:water"),
    ];
    let registry = Arc::new(mc_world::BlockRegistry::from_report(&reports).unwrap());
    let facts = mc_data::block_facts::BlockFactsTable::from_blocks_report(&reports);
    let generated_chunks = Arc::new(AtomicUsize::new(0));
    let mut world = mc_world::WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 16)
        .with_generator(Arc::new(CountingAirGenerator {
            calls: Arc::clone(&generated_chunks),
        }));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let edge_farmland = mc_world::BlockPos {
        x: 15,
        y: 64,
        z: 15,
    };
    world.set_block_at(edge_farmland, BlockStateId(2)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            edge_farmland,
            BlockStateId(2),
            mc_data::block_facts::RandomTickFamily::Farmland,
        ),
        Some(vec![BlockEdit {
            pos: edge_farmland,
            new_state: BlockStateId(1),
        }])
    );
    assert_eq!(generated_chunks.load(Ordering::Relaxed), 0);
    assert_eq!(world.cache_len(), 1);
}

#[test]
fn vertical_plant_random_tick_does_not_materialize_neighbour_chunks() {
    struct CountingAirGenerator {
        calls: Arc<AtomicUsize>,
    }

    impl mc_world::ChunkGenerator for CountingAirGenerator {
        fn generate(&self, pos: ChunkPos) -> Chunk {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let mut chunk = Chunk::empty(
                pos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            );
            chunk.status = "minecraft:full".into();
            chunk.dirty = true;
            chunk
        }
    }

    let registry = Arc::new(fluid_test_registry());
    let facts = fluid_test_facts();
    let generated_chunks = Arc::new(AtomicUsize::new(0));
    let mut world = mc_world::WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 16)
        .with_generator(Arc::new(CountingAirGenerator {
            calls: Arc::clone(&generated_chunks),
        }));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let support = mc_world::BlockPos {
        x: 15,
        y: 64,
        z: 15,
    };
    let cactus = mc_world::BlockPos {
        x: 15,
        y: 65,
        z: 15,
    };
    world.set_block_at(support, BlockStateId(1)).unwrap();
    world.set_block_at(cactus, BlockStateId(19)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cactus,
            BlockStateId(19),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );
    assert_eq!(generated_chunks.load(Ordering::Relaxed), 0);
    assert_eq!(world.cache_len(), 1);
}

#[test]
fn stem_crop_growth_advances_melon_and_pumpkin_stems_once() {
    let blocks = crop_test_registry();
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 2 };

    for (stem, first_state) in [("minecraft:pumpkin_stem", 52), ("minecraft:melon_stem", 54)] {
        assert_eq!(
            next_crop_growth_state(&blocks, mc_world::BlockStateId(first_state)),
            Some(mc_world::BlockStateId(first_state + 1)),
            "{stem} age 0 should advance"
        );
        assert_eq!(
            next_crop_growth_state(&blocks, mc_world::BlockStateId(first_state + 1)),
            None,
            "{stem} max fixture age should not advance"
        );
        assert_eq!(
            bonemeal_growth_edit(&blocks, pos, mc_world::BlockStateId(first_state)),
            Some(BlockEdit {
                pos,
                new_state: mc_world::BlockStateId(first_state + 1),
            }),
            "{stem} bonemeal should advance by one age"
        );
    }
}

#[test]
fn mature_stem_growth_places_fruit_and_attaches_stem() {
    let registry = Arc::new(crop_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();

    let melon_stem = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let melon_fruit = mc_world::BlockPos { x: 4, y: 64, z: 3 };
    world.set_block_at(melon_stem, BlockStateId(55)).unwrap();
    assert_eq!(
        random_tick_edit(
            blocks,
            &mc_data::block_facts::BlockFactsTable::default(),
            &world,
            melon_stem,
            BlockStateId(55),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        Some(vec![
            BlockEdit {
                pos: melon_stem,
                new_state: BlockStateId(65),
            },
            BlockEdit {
                pos: melon_fruit,
                new_state: BlockStateId(63),
            },
        ])
    );

    let pumpkin_stem = mc_world::BlockPos { x: 8, y: 64, z: 8 };
    let blocked_north = mc_world::BlockPos { x: 8, y: 64, z: 7 };
    let pumpkin_fruit = mc_world::BlockPos { x: 8, y: 64, z: 9 };
    world.set_block_at(pumpkin_stem, BlockStateId(53)).unwrap();
    world.set_block_at(blocked_north, BlockStateId(1)).unwrap();
    assert_eq!(
        bonemeal_growth_edits(blocks, &world, pumpkin_stem, BlockStateId(53), 0),
        Some(vec![
            BlockEdit {
                pos: pumpkin_stem,
                new_state: BlockStateId(70),
            },
            BlockEdit {
                pos: pumpkin_fruit,
                new_state: BlockStateId(64),
            },
        ])
    );
}

#[test]
fn sweet_berry_bush_growth_advances_until_mature() {
    let blocks = crop_test_registry();
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 2 };

    assert_eq!(
        next_crop_growth_state(&blocks, mc_world::BlockStateId(56)),
        Some(mc_world::BlockStateId(57))
    );
    assert_eq!(
        bonemeal_growth_edit(&blocks, pos, mc_world::BlockStateId(57)),
        Some(BlockEdit {
            pos,
            new_state: mc_world::BlockStateId(58),
        })
    );
    assert_eq!(
        next_crop_growth_state(&blocks, mc_world::BlockStateId(58)),
        Some(mc_world::BlockStateId(59))
    );
    assert_eq!(
        next_crop_growth_state(&blocks, mc_world::BlockStateId(59)),
        None
    );
}

#[test]
fn sweet_berry_harvest_resets_mature_bush_and_drops_berries() {
    let blocks = crop_test_registry();
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:sweet_berries").unwrap(),
        protocol_id: 88,
    }]);
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 2 };

    assert_eq!(
        sweet_berry_harvest(&blocks, &items, pos, mc_world::BlockStateId(58)),
        Some((
            BlockEdit {
                pos,
                new_state: mc_world::BlockStateId(57),
            },
            ItemStack::new(88, 1),
        ))
    );
    assert_eq!(
        sweet_berry_harvest(&blocks, &items, pos, mc_world::BlockStateId(59)),
        Some((
            BlockEdit {
                pos,
                new_state: mc_world::BlockStateId(57),
            },
            ItemStack::new(88, 2),
        ))
    );
    assert_eq!(
        sweet_berry_harvest(&blocks, &items, pos, mc_world::BlockStateId(57)),
        None
    );

    let missing_berries = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:wheat").unwrap(),
        protocol_id: 50,
    }]);
    assert_eq!(
        sweet_berry_harvest(&blocks, &missing_berries, pos, mc_world::BlockStateId(59)),
        None
    );
}

#[tokio::test]
async fn sweet_berry_harvest_planning_does_not_wait_for_world_writer() {
    let mut state = interaction_state_for_blocks(Arc::new(crop_test_registry()));
    state.items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:sweet_berries").unwrap(),
        protocol_id: 88,
    }]));
    insert_fluid_test_chunk(&state).await;
    let position = mc_world::BlockPos { x: 1, y: 64, z: 2 };
    let expected_token = {
        let mut storage = state.world.lock().await;
        storage
            .set_block_at(position, mc_world::BlockStateId(59))
            .expect("place mature berry bush");
        storage
            .block_mutation_token(position)
            .expect("berry bush mutation token")
    };

    let world = Arc::clone(&state.world);
    let world_writer = world.lock().await;
    let (edit, dropped_stack, precondition) = plan_loaded_plant_harvest(&state, position)
        .expect("published mature berry bush should be harvestable");

    assert_eq!(
        edit,
        BlockEdit {
            pos: position,
            new_state: mc_world::BlockStateId(57),
        }
    );
    assert_eq!(dropped_stack, ItemStack::new(88, 2));
    assert_eq!(precondition.expected_state, mc_world::BlockStateId(59));
    assert_eq!(precondition.expected_token, expected_token);
    drop(world_writer);
}

#[test]
fn cocoa_growth_advances_age_without_losing_facing() {
    let blocks = crop_test_registry();
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 2 };

    assert_eq!(
        next_crop_growth_state(&blocks, mc_world::BlockStateId(60)),
        Some(mc_world::BlockStateId(61))
    );
    assert_eq!(
        bonemeal_growth_edit(&blocks, pos, mc_world::BlockStateId(61)),
        Some(BlockEdit {
            pos,
            new_state: mc_world::BlockStateId(62),
        })
    );
    assert_eq!(
        blocks
            .by_id(mc_world::BlockStateId(62))
            .and_then(|state| block_state_property(state, "facing")),
        Some("north")
    );
    assert_eq!(
        next_crop_growth_state(&blocks, mc_world::BlockStateId(62)),
        None
    );
}

#[test]
fn bonemeal_growth_edit_advances_supported_crop_one_age() {
    let blocks = crop_test_registry();
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 2 };

    for (crop, first_state) in [
        ("minecraft:wheat", 11),
        ("minecraft:carrots", 20),
        ("minecraft:potatoes", 28),
        ("minecraft:beetroots", 36),
    ] {
        assert_eq!(
            bonemeal_growth_edit(&blocks, pos, mc_world::BlockStateId(first_state)),
            Some(BlockEdit {
                pos,
                new_state: mc_world::BlockStateId(first_state + 1),
            }),
            "{crop} should advance by one registered age state"
        );
    }
}

#[test]
fn bonemeal_growth_edit_ignores_mature_and_invalid_targets() {
    let blocks = crop_test_registry();
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 2 };

    assert_eq!(
        bonemeal_growth_edit(&blocks, pos, mc_world::BlockStateId(18)),
        None
    );
    assert_eq!(
        bonemeal_growth_edit(&blocks, pos, mc_world::BlockStateId(1)),
        None
    );
    assert_eq!(
        bonemeal_growth_edit(&blocks, pos, mc_world::BlockStateId(44)),
        None,
        "nether wart grows through random ticks but must reject bonemeal"
    );
}

#[tokio::test]
async fn bonemeal_planning_does_not_wait_for_world_writer() {
    let state = interaction_state_for_blocks(Arc::new(crop_test_registry()));
    insert_fluid_test_chunk(&state).await;
    let position = mc_world::BlockPos { x: 1, y: 64, z: 2 };
    let expected_token = {
        let mut storage = state.world.lock().await;
        storage
            .set_block_at(position, mc_world::BlockStateId(11))
            .expect("place young wheat");
        storage
            .block_mutation_token(position)
            .expect("wheat mutation token")
    };

    let world = Arc::clone(&state.world);
    let world_writer = world.lock().await;
    let (edits, preconditions) = plan_loaded_bonemeal_growth(&state, position, 0)
        .expect("published young wheat should accept bonemeal");

    assert_eq!(
        edits,
        vec![BlockEdit {
            pos: position,
            new_state: mc_world::BlockStateId(12),
        }]
    );
    assert_eq!(
        preconditions,
        vec![BlockEditPrecondition {
            pos: position,
            expected_state: mc_world::BlockStateId(11),
            expected_token,
        }]
    );
    drop(world_writer);
}

#[test]
fn sapling_bonemeal_advances_stage_before_growing_a_varied_oak_tree() {
    let registry = sapling_tree_test_registry();
    let mut world = in_memory_tree_world(Arc::clone(&registry));
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(1)).unwrap();

    let stage_edit =
        bonemeal_growth_edits(registry.as_ref(), &world, pos, BlockStateId(1), 0).unwrap();
    assert_eq!(
        stage_edit,
        vec![BlockEdit {
            pos,
            new_state: BlockStateId(27)
        }]
    );

    let mut outcome = BlockEditBatchOutcome::default();
    for edit in &stage_edit {
        apply_block_edit_to_storage(&mut world, None, edit, &mut outcome);
    }
    assert_eq!(world.get_block(pos).unwrap(), Some(BlockStateId(27)));

    let mut inventory = PlayerInventory::empty();
    inventory
        .set_hotbar(
            0,
            ItemStack {
                item_id: 99,
                count: 2,
                damage: None,
                enchantments: Vec::new(),
                custom_name: None,
                item_model: None,
            },
        )
        .unwrap();
    let synced =
        consume_bonemeal_after_growth(&mut inventory, 0, !outcome.applied.is_empty()).unwrap();
    assert_eq!(synced.count, 1);
    assert_eq!(inventory.held(0).unwrap().count, 1);

    let short_tree =
        bonemeal_growth_edits(registry.as_ref(), &world, pos, BlockStateId(27), 0).unwrap();
    let tall_tree =
        bonemeal_growth_edits(registry.as_ref(), &world, pos, BlockStateId(27), 2).unwrap();
    let trunk_height = |edits: &[BlockEdit]| {
        edits
            .iter()
            .filter(|edit| {
                edit.pos.x == pos.x && edit.pos.z == pos.z && edit.new_state == BlockStateId(3)
            })
            .count()
    };
    assert_eq!(trunk_height(&short_tree), 4);
    assert_eq!(trunk_height(&tall_tree), 6);
    assert!(short_tree.iter().any(|edit| {
        edit.pos == mc_world::BlockPos { x: 4, y: 68, z: 4 } && edit.new_state == BlockStateId(4)
    }));
}

#[test]
fn single_sapling_bonemeal_uses_matching_log_and_leaves() {
    let registry = sapling_tree_test_registry();

    for (sapling_state, log_state, leaves_state) in
        [(29, 9, 10), (30, 13, 14), (31, 17, 18), (32, 21, 22)]
    {
        let mut world = in_memory_tree_world(Arc::clone(&registry));
        let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
        world
            .set_block_at(pos, BlockStateId(sapling_state))
            .unwrap();

        let edits = bonemeal_growth_edits(
            registry.as_ref(),
            &world,
            pos,
            BlockStateId(sapling_state),
            0,
        )
        .unwrap();

        assert_eq!(
            edits[0],
            BlockEdit {
                pos,
                new_state: BlockStateId(log_state)
            },
            "sapling state {sapling_state} should use its matching log"
        );
        assert!(
            edits
                .iter()
                .any(|edit| edit.new_state == BlockStateId(leaves_state)),
            "sapling state {sapling_state} should use its matching leaves"
        );
    }
}

#[test]
fn dark_oak_bonemeal_requires_a_complete_two_by_two_square() {
    let registry = sapling_tree_test_registry();
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };

    for (state, neighbors) in [
        (BlockStateId(23), 0),
        (BlockStateId(33), 0),
        (BlockStateId(33), 2),
    ] {
        let mut world = in_memory_tree_world(Arc::clone(&registry));
        world.set_block_at(pos, state).unwrap();
        for offset in [(1, 0), (0, 1)].into_iter().take(neighbors) {
            world
                .set_block_at(
                    mc_world::BlockPos {
                        x: pos.x + offset.0,
                        z: pos.z + offset.1,
                        ..pos
                    },
                    BlockStateId(23),
                )
                .unwrap();
        }

        assert_eq!(
            bonemeal_growth_edits(registry.as_ref(), &world, pos, state, 0),
            None,
            "dark oak must not consume bone meal without all four saplings"
        );
        assert_eq!(world.get_block(pos).unwrap(), Some(state));
    }
}

#[test]
fn dark_oak_two_by_two_uses_one_anchor_and_replaces_all_four_saplings() {
    let registry = sapling_tree_test_registry();
    let northwest = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let saplings = [
        northwest,
        mc_world::BlockPos { x: 5, ..northwest },
        mc_world::BlockPos { z: 5, ..northwest },
        mc_world::BlockPos {
            x: 5,
            z: 5,
            ..northwest
        },
    ];
    let mut world = in_memory_tree_world(Arc::clone(&registry));
    for pos in saplings {
        world.set_block_at(pos, BlockStateId(33)).unwrap();
    }

    let expected = bonemeal_growth_edits(registry.as_ref(), &world, northwest, BlockStateId(33), 0)
        .expect("complete dark oak square grows");
    for clicked in saplings {
        assert_eq!(
            bonemeal_growth_edits(registry.as_ref(), &world, clicked, BlockStateId(33), 0,),
            Some(expected.clone()),
            "each sapling must resolve the same northwest corner"
        );
    }
    for pos in saplings {
        assert!(expected.contains(&BlockEdit {
            pos,
            new_state: BlockStateId(25),
        }));
    }
}

#[test]
fn dark_oak_two_by_two_rejects_unloaded_canopy_without_partial_edits() {
    let registry = sapling_tree_test_registry();
    let northwest = mc_world::BlockPos { x: 14, y: 64, z: 4 };
    let saplings = [
        northwest,
        mc_world::BlockPos { x: 15, ..northwest },
        mc_world::BlockPos { z: 5, ..northwest },
        mc_world::BlockPos {
            x: 15,
            z: 5,
            ..northwest
        },
    ];
    let mut world = in_memory_tree_world(Arc::clone(&registry));
    for pos in saplings {
        world.set_block_at(pos, BlockStateId(33)).unwrap();
    }

    assert_eq!(
        bonemeal_growth_edits(registry.as_ref(), &world, northwest, BlockStateId(33), 0,),
        None
    );
    for pos in saplings {
        assert_eq!(world.get_block(pos).unwrap(), Some(BlockStateId(33)));
    }
}

#[test]
fn spruce_and_jungle_two_by_two_use_one_anchor_and_replace_all_four_saplings() {
    let registry = sapling_tree_test_registry();
    let northwest = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let saplings = [
        northwest,
        mc_world::BlockPos { x: 5, ..northwest },
        mc_world::BlockPos { z: 5, ..northwest },
        mc_world::BlockPos {
            x: 5,
            z: 5,
            ..northwest
        },
    ];

    for (state, log, leaves) in [
        (BlockStateId(30), BlockStateId(13), BlockStateId(14)),
        (BlockStateId(31), BlockStateId(17), BlockStateId(18)),
    ] {
        let mut world = in_memory_tree_world(Arc::clone(&registry));
        for pos in saplings {
            world.set_block_at(pos, state).unwrap();
        }

        let expected = bonemeal_growth_edits(registry.as_ref(), &world, northwest, state, 0)
            .expect("complete spruce and jungle squares grow");
        for clicked in saplings {
            assert_eq!(
                bonemeal_growth_edits(registry.as_ref(), &world, clicked, state, 0),
                Some(expected.clone()),
                "each sapling must resolve the same northwest corner"
            );
        }
        for pos in saplings {
            assert!(expected.contains(&BlockEdit {
                pos,
                new_state: log,
            }));
        }
        assert!(expected.iter().any(|edit| edit.new_state == leaves));
    }
}

#[test]
fn spruce_and_jungle_two_by_two_reject_obstruction_or_unloaded_canopy_atomically() {
    let registry = sapling_tree_test_registry();

    for (state, leaves) in [
        (BlockStateId(30), BlockStateId(14)),
        (BlockStateId(31), BlockStateId(18)),
    ] {
        let northwest = mc_world::BlockPos { x: 4, y: 64, z: 4 };
        let saplings = [
            northwest,
            mc_world::BlockPos { x: 5, ..northwest },
            mc_world::BlockPos { z: 5, ..northwest },
            mc_world::BlockPos {
                x: 5,
                z: 5,
                ..northwest
            },
        ];
        let mut clear_world = in_memory_tree_world(Arc::clone(&registry));
        for pos in saplings {
            clear_world.set_block_at(pos, state).unwrap();
        }
        let clear_edits =
            bonemeal_growth_edits(registry.as_ref(), &clear_world, northwest, state, 0)
                .expect("clear mega-tree space grows");
        let blocked = clear_edits
            .iter()
            .find(|edit| !saplings.contains(&edit.pos) && edit.new_state == leaves)
            .expect("mega-tree template has an obstruction target")
            .pos;
        clear_world.set_block_at(blocked, BlockStateId(5)).unwrap();

        assert_eq!(
            bonemeal_growth_edits(registry.as_ref(), &clear_world, northwest, state, 0),
            None
        );
        for pos in saplings {
            assert_eq!(clear_world.get_block(pos).unwrap(), Some(state));
        }

        let edge = mc_world::BlockPos { x: 14, ..northwest };
        let edge_saplings = [
            edge,
            mc_world::BlockPos { x: 15, ..edge },
            mc_world::BlockPos { z: 5, ..edge },
            mc_world::BlockPos {
                x: 15,
                z: 5,
                ..edge
            },
        ];
        let mut edge_world = in_memory_tree_world(Arc::clone(&registry));
        for pos in edge_saplings {
            edge_world.set_block_at(pos, state).unwrap();
        }
        assert_eq!(
            bonemeal_growth_edits(registry.as_ref(), &edge_world, edge, state, 0),
            None
        );
        for pos in edge_saplings {
            assert_eq!(edge_world.get_block(pos).unwrap(), Some(state));
        }
    }
}

#[tokio::test]
async fn dark_oak_two_by_two_stale_sapling_token_rejects_the_whole_edit_set() {
    let registry = sapling_tree_test_registry();
    let northwest = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let saplings = [
        northwest,
        mc_world::BlockPos { x: 5, ..northwest },
        mc_world::BlockPos { z: 5, ..northwest },
        mc_world::BlockPos {
            x: 5,
            z: 5,
            ..northwest
        },
    ];
    let mut storage = in_memory_tree_world(Arc::clone(&registry));
    for pos in saplings {
        storage.set_block_at(pos, BlockStateId(33)).unwrap();
    }
    let edits = bonemeal_growth_edits(registry.as_ref(), &storage, northwest, BlockStateId(33), 0)
        .expect("complete dark oak square plans one edit set");
    let preconditions = edits
        .iter()
        .map(|edit| BlockEditPrecondition {
            pos: edit.pos,
            expected_state: storage.get_block(edit.pos).unwrap().unwrap(),
            expected_token: storage.block_mutation_token(edit.pos).unwrap(),
        })
        .collect::<Vec<_>>();

    storage.set_block_at(saplings[3], BlockStateId(0)).unwrap();
    storage.set_block_at(saplings[3], BlockStateId(33)).unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = SessionRegistry::new();
    let session = register_ticketed_button_session(&sessions, "DarkOakStaleToken");
    let _ = sessions.mark_loaded(session, (0, 0));
    let (handle, mut owner) = simulation_channel();
    let session_handle = handle.for_session(session);
    let mut growth = Box::pin(session_handle.apply_block_edits(edits, preconditions));
    let waker = std::task::Waker::noop();
    let mut context = Context::from_waker(waker);
    assert!(matches!(
        std::future::Future::poll(growth.as_mut(), &mut context),
        Poll::Pending
    ));
    assert_eq!(
        owner
            .process_tick_with_world(&sessions, Some(&world), None, 1)
            .processed,
        1
    );
    assert!(growth.await.unwrap().is_none());

    let mut storage = world.lock().await;
    for pos in saplings {
        assert_eq!(storage.get_block(pos).unwrap(), Some(BlockStateId(33)));
    }
    assert_eq!(
        storage
            .get_block(mc_world::BlockPos {
                y: northwest.y + 1,
                ..northwest
            })
            .unwrap(),
        Some(BlockStateId(0))
    );
}

#[test]
fn stage_one_oak_sapling_replaces_leaves_and_supported_canopy_vegetation() {
    let registry = sapling_tree_test_registry();
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let canopy_pos = mc_world::BlockPos { x: 5, y: 68, z: 4 };

    for existing in [BlockStateId(4), BlockStateId(34), BlockStateId(35)] {
        let mut world = in_memory_tree_world(Arc::clone(&registry));
        world.set_block_at(pos, BlockStateId(27)).unwrap();
        world.set_block_at(canopy_pos, existing).unwrap();

        let edits = bonemeal_growth_edits(registry.as_ref(), &world, pos, BlockStateId(27), 0)
            .expect("replaceable canopy vegetation permits oak growth");
        assert!(
            edits
                .iter()
                .any(|edit| { edit.pos == canopy_pos && edit.new_state == BlockStateId(4) })
        );
    }
}

#[test]
fn stage_one_oak_sapling_accepts_exact_vanilla_tree_replaceable_membership() {
    assert_eq!(
        VANILLA_26_1_2_TREE_REPLACEABLES.len() + 2,
        55,
        "53 concrete blocks plus the leaves and small_flowers tag members"
    );
    let registry = sapling_tree_test_registry();
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let canopy_pos = mc_world::BlockPos { x: 5, y: 68, z: 4 };

    for name in VANILLA_26_1_2_TREE_REPLACEABLES {
        let state = registry
            .block(&Identifier::parse(name).unwrap())
            .unwrap_or_else(|| panic!("missing tree-replaceable fixture {name}"))
            .default;
        let mut world = in_memory_tree_world(Arc::clone(&registry));
        world.set_block_at(pos, BlockStateId(27)).unwrap();
        world.set_block_at(canopy_pos, state).unwrap();

        let edits = bonemeal_growth_edits(registry.as_ref(), &world, pos, BlockStateId(27), 0)
            .unwrap_or_else(|| panic!("tree planner rejected vanilla replaceable {name}"));
        assert!(edits.iter().any(|edit| edit.pos == canopy_pos));
    }
}

#[test]
fn stage_one_oak_sapling_rejects_unloaded_canopy_atomically() {
    let registry = sapling_tree_test_registry();
    let mut world = in_memory_tree_world(Arc::clone(&registry));
    let pos = mc_world::BlockPos { x: 14, y: 64, z: 4 };
    let loaded_trunk_cell = mc_world::BlockPos { y: 65, ..pos };
    world.set_block_at(pos, BlockStateId(27)).unwrap();

    assert_eq!(
        bonemeal_growth_edits(registry.as_ref(), &world, pos, BlockStateId(27), 0),
        None
    );
    assert_eq!(world.get_block(pos).unwrap(), Some(BlockStateId(27)));
    assert_eq!(
        world.get_block(loaded_trunk_cell).unwrap(),
        Some(BlockStateId(0))
    );
}

#[test]
fn stage_zero_sapling_advances_even_when_tree_space_is_blocked() {
    let registry = sapling_tree_test_registry();
    let mut world = in_memory_tree_world(Arc::clone(&registry));
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(1)).unwrap();
    world
        .set_block_at(mc_world::BlockPos { x: 4, y: 68, z: 4 }, BlockStateId(5))
        .unwrap();

    assert_eq!(
        bonemeal_growth_edits(registry.as_ref(), &world, pos, BlockStateId(1), 0),
        Some(vec![BlockEdit {
            pos,
            new_state: BlockStateId(27),
        }])
    );
    world.set_block_at(pos, BlockStateId(27)).unwrap();
    assert_eq!(
        bonemeal_growth_edits(registry.as_ref(), &world, pos, BlockStateId(27), 0),
        None
    );
    assert_eq!(world.get_block(pos).unwrap(), Some(BlockStateId(27)));

    let mut inventory = PlayerInventory::empty();
    inventory
        .set_hotbar(
            0,
            ItemStack {
                item_id: 99,
                count: 2,
                damage: None,
                enchantments: Vec::new(),
                custom_name: None,
                item_model: None,
            },
        )
        .unwrap();
    assert_eq!(
        consume_bonemeal_after_growth(&mut inventory, 0, false),
        None
    );
    assert_eq!(inventory.held(0).unwrap().count, 2);
}

#[test]
fn sapling_bonemeal_unsupported_and_missing_tree_states_are_noop() {
    let registry = sapling_tree_test_registry();
    let world = in_memory_tree_world(Arc::clone(&registry));
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    assert_eq!(
        bonemeal_growth_edits(registry.as_ref(), &world, pos, BlockStateId(6), 0),
        None
    );

    let reports = vec![
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:oak_sapling"),
        simple_block(2, "minecraft:oak_leaves"),
    ];
    let missing_registry = Arc::new(mc_world::BlockRegistry::from_report(&reports).unwrap());
    let world = in_memory_tree_world(Arc::clone(&missing_registry));
    assert_eq!(
        bonemeal_growth_edits(missing_registry.as_ref(), &world, pos, BlockStateId(1), 0,),
        None
    );
}

#[test]
fn bonemeal_consumes_exactly_one_item_only_after_successful_growth() {
    let mut inventory = PlayerInventory::empty();
    inventory
        .set_hotbar(
            0,
            ItemStack {
                item_id: 99,
                count: 3,
                damage: None,
                enchantments: Vec::new(),
                custom_name: None,
                item_model: None,
            },
        )
        .unwrap();

    assert_eq!(
        consume_bonemeal_after_growth(&mut inventory, 0, false),
        None
    );
    assert_eq!(inventory.held(0).unwrap().count, 3);

    let synced = consume_bonemeal_after_growth(&mut inventory, 0, true).unwrap();
    assert_eq!(synced.count, 2);
    assert_eq!(inventory.held(0).unwrap().count, 2);

    inventory
        .set_hotbar(
            0,
            ItemStack {
                item_id: 99,
                count: 1,
                damage: None,
                enchantments: Vec::new(),
                custom_name: None,
                item_model: None,
            },
        )
        .unwrap();
    let synced = consume_bonemeal_after_growth(&mut inventory, 0, true).unwrap();
    assert!(synced.is_empty());
    assert!(inventory.held(0).unwrap().is_empty());
}

#[test]
fn sapling_random_tick_uses_one_in_seven_chance_and_two_growth_stages() {
    let reports = sapling_tree_test_reports();
    let registry = Arc::new(mc_world::BlockRegistry::from_report(&reports).unwrap());
    let facts = mc_data::block_facts::BlockFactsTable::from_blocks_report(&reports);
    let mut world = in_memory_tree_world(Arc::clone(&registry));
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(1)).unwrap();

    assert_eq!(
        facts.random_tick_family(1),
        Some(mc_data::block_facts::RandomTickFamily::Sapling)
    );
    let edits = random_tick_edit_seeded(
        registry.as_ref(),
        &facts,
        &world,
        pos,
        BlockStateId(1),
        mc_data::block_facts::RandomTickFamily::Sapling,
        0,
    )
    .unwrap();
    assert_eq!(
        edits,
        vec![BlockEdit {
            pos,
            new_state: BlockStateId(27)
        }]
    );
    assert_eq!(
        random_tick_edit_seeded(
            registry.as_ref(),
            &facts,
            &world,
            pos,
            BlockStateId(1),
            mc_data::block_facts::RandomTickFamily::Sapling,
            1,
        ),
        None,
        "six of seven selected random ticks must leave the sapling unchanged"
    );

    let mut outcome = BlockEditBatchOutcome::default();
    for edit in &edits {
        apply_block_edit_to_storage(&mut world, None, edit, &mut outcome);
    }
    assert!(!outcome.applied.is_empty());
    assert_eq!(world.get_block(pos).unwrap(), Some(BlockStateId(27)));

    let tree_edits = random_tick_edit_seeded(
        registry.as_ref(),
        &facts,
        &world,
        pos,
        BlockStateId(27),
        mc_data::block_facts::RandomTickFamily::Sapling,
        0,
    )
    .unwrap();
    assert!(
        tree_edits
            .iter()
            .any(|edit| edit.new_state == BlockStateId(3))
    );
    assert!(
        tree_edits
            .iter()
            .any(|edit| edit.new_state == BlockStateId(4))
    );
}

#[test]
fn sapling_random_tick_obstructed_or_unsupported_targets_are_noop() {
    let reports = sapling_tree_test_reports();
    let registry = Arc::new(mc_world::BlockRegistry::from_report(&reports).unwrap());
    let facts = mc_data::block_facts::BlockFactsTable::from_blocks_report(&reports);
    let mut world = in_memory_tree_world(Arc::clone(&registry));
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(27)).unwrap();
    world
        .set_block_at(mc_world::BlockPos { x: 4, y: 68, z: 4 }, BlockStateId(5))
        .unwrap();

    assert_eq!(
        random_tick_edit_seeded(
            registry.as_ref(),
            &facts,
            &world,
            pos,
            BlockStateId(27),
            mc_data::block_facts::RandomTickFamily::Sapling,
            0,
        ),
        None
    );
    assert_eq!(world.get_block(pos).unwrap(), Some(BlockStateId(27)));
    assert_eq!(
        random_tick_edit_seeded(
            registry.as_ref(),
            &facts,
            &world,
            pos,
            BlockStateId(6),
            mc_data::block_facts::RandomTickFamily::Sapling,
            0,
        ),
        None
    );
}

#[test]
fn farmland_trample_requires_landing_on_block() {
    let old_pose = PlayerPose::new(2.7, 3.0, -1.2);
    let landed = PlayerPose {
        y: 1.0,
        flags: MovePlayerFlags::new(true, false),
        ..old_pose
    };
    let hovering = PlayerPose {
        flags: MovePlayerFlags::new(false, false),
        ..landed
    };

    assert_eq!(
        farmland_trample_pos(old_pose, landed),
        Some(mc_world::BlockPos { x: 2, y: 0, z: -2 })
    );
    assert_eq!(farmland_trample_pos(old_pose, hovering), None);
    assert_eq!(farmland_trample_pos(landed, landed), None);
}

#[tokio::test]
async fn farmland_trample_does_not_overwrite_a_newer_block_state() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:farmland"),
            simple_block(2, "minecraft:dirt"),
            simple_block(3, "minecraft:stone"),
        ])
        .unwrap(),
    );
    let storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let world_read = storage.read_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let mut state = interaction_state_for_items(Arc::new(ItemRegistry::default()));
    state.blocks = blocks;
    state.world = Arc::clone(&world);
    state.world_read = world_read;
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let mut storage = world.lock().await;
    let cpos = ChunkPos { x: 0, z: 0 };
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(pos, BlockStateId(1)).unwrap();

    let old_pose = PlayerPose::new(1.5, 66.0, 1.5);
    let new_pose = PlayerPose {
        y: 65.0,
        flags: MovePlayerFlags::new(true, false),
        ..old_pose
    };
    let mut writer = Vec::new();
    let mut trample = Box::pin(maybe_trample_farmland(
        &mut state,
        &mut writer,
        old_pose,
        new_pose,
    ));
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(trample.as_mut(), cx).is_pending(),
            "trample must wait for the held world writer"
        );
        Poll::Ready(())
    })
    .await;

    storage.set_block_at(pos, BlockStateId(3)).unwrap();
    drop(storage);
    trample.await.unwrap();

    let storage = world.lock().await;
    assert_eq!(storage.get_cached_block(pos), Some(BlockStateId(3)));
}

#[tokio::test]
async fn hoe_tilling_plan_does_not_wait_for_writer_and_guards_the_block_above() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:dirt"),
            simple_block(2, "minecraft:farmland"),
            simple_block(3, "minecraft:stone"),
        ])
        .unwrap(),
    );
    let storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let world_read = storage.read_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let mut state = interaction_state_for_items(Arc::new(ItemRegistry::default()));
    state.blocks = Arc::clone(&blocks);
    state.world = Arc::clone(&world);
    state.world_read = world_read;

    let clicked = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let above = mc_world::BlockPos { y: 65, ..clicked };
    let mut storage = world.lock().await;
    let cpos = ChunkPos { x: 0, z: 0 };
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(clicked, BlockStateId(1)).unwrap();

    let plan = plan_hoe_tilling(&state, clicked, BlockStateId(2)).expect("tillable dirt plan");
    assert_eq!(plan.edits.len(), 1);
    assert_eq!(plan.preconditions.len(), 2);
    assert!(
        plan.preconditions
            .iter()
            .any(|guard| { guard.pos == clicked && guard.expected_state == BlockStateId(1) })
    );
    assert!(
        plan.preconditions
            .iter()
            .any(|guard| { guard.pos == above && guard.expected_state == BlockStateId(0) })
    );

    storage.set_block_at(above, BlockStateId(3)).unwrap();
    assert!(
        apply_block_edit_batch_to_storage_conditionally(
            &mut storage,
            None,
            &plan.edits,
            &plan.preconditions,
        )
        .is_none(),
        "a block placed above after planning must reject tilling"
    );
    assert_eq!(storage.get_cached_block(clicked), Some(BlockStateId(1)));
}

#[test]
fn natural_random_tick_helpers_cover_leaves_grass_and_fire() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:dirt"),
        BlockReport {
            id: Identifier::parse("minecraft:oak_leaves").unwrap(),
            properties: prop_schema(&[
                ("distance", &["6", "7"]),
                ("persistent", &["false", "true"]),
            ]),
            states: vec![
                state(2, true, &[("distance", "7"), ("persistent", "false")]),
                state(3, false, &[("distance", "7"), ("persistent", "true")]),
                state(4, false, &[("distance", "6"), ("persistent", "false")]),
            ],
        },
        BlockReport {
            id: Identifier::parse("minecraft:fire").unwrap(),
            properties: prop_schema(&[("age", &["14", "15"])]),
            states: vec![
                state(5, true, &[("age", "14")]),
                state(6, false, &[("age", "15")]),
            ],
        },
        simple_block(7, "minecraft:grass_block"),
    ])
    .unwrap();

    assert_eq!(
        next_leaf_decay_state(&blocks, mc_world::BlockStateId(2)),
        Some(mc_world::BlockStateId(0))
    );
    assert_eq!(
        next_leaf_decay_state(&blocks, mc_world::BlockStateId(3)),
        None
    );
    assert_eq!(
        next_leaf_decay_state(&blocks, mc_world::BlockStateId(4)),
        None
    );
    assert_eq!(
        next_fire_state(&blocks, mc_world::BlockStateId(5)),
        Some(mc_world::BlockStateId(6))
    );
    assert_eq!(
        next_fire_state(&blocks, mc_world::BlockStateId(6)),
        Some(mc_world::BlockStateId(0))
    );
}

#[test]
fn fire_random_tick_spreads_to_common_fuel() {
    let reports = vec![
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:fire").unwrap(),
            properties: prop_schema(&[("age", &["0", "1"])]),
            states: vec![
                state(1, true, &[("age", "0")]),
                state(2, false, &[("age", "1")]),
            ],
        },
        simple_block(3, "minecraft:oak_log"),
    ];
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&reports).unwrap());
    let facts = mc_data::block_facts::BlockFactsTable::from_blocks_report(&reports);
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let chunk = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            chunk,
            Chunk::empty(
                chunk,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let fire = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let fuel = mc_world::BlockPos { x: 5, y: 64, z: 4 };
    world.set_block_at(fire, BlockStateId(1)).unwrap();
    world.set_block_at(fuel, BlockStateId(3)).unwrap();

    let edits = random_tick_edit_seeded(
        blocks.as_ref(),
        &facts,
        &world,
        fire,
        BlockStateId(1),
        mc_data::block_facts::RandomTickFamily::Fire,
        0,
    )
    .unwrap();

    assert_eq!(
        edits,
        vec![
            BlockEdit {
                pos: fire,
                new_state: BlockStateId(2),
            },
            BlockEdit {
                pos: fuel,
                new_state: BlockStateId(1),
            },
        ]
    );
}

#[test]
fn protected_zone_rejects_only_ambient_fire_target() {
    let source = mc_world::BlockPos { x: -1, y: 64, z: 0 };
    let protected = mc_world::BlockPos { x: 0, y: 64, z: 0 };
    let zone = mc_script::ScriptAxisAlignedZone::try_new_with_protection(
        "claim",
        "minecraft:overworld",
        mc_script::ScriptPosition::try_new(0.0, 0.0, 0.0).unwrap(),
        mc_script::ScriptPosition::try_new(15.0, 319.0, 15.0).unwrap(),
        Some(
            mc_script::ScriptZoneProtection::try_actor_or_operator(
                "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            )
            .unwrap(),
        ),
    )
    .unwrap();
    let protection = crate::script::ZoneProtectionSnapshot::from_zones(vec![zone]);

    assert!(ambient_random_tick_edit_allowed(
        mc_data::block_facts::RandomTickFamily::Fire,
        source,
        source,
        Some(&protection),
    ));
    assert!(!ambient_random_tick_edit_allowed(
        mc_data::block_facts::RandomTickFamily::Fire,
        source,
        protected,
        Some(&protection),
    ));
    assert!(ambient_random_tick_edit_allowed(
        mc_data::block_facts::RandomTickFamily::Crop,
        source,
        protected,
        Some(&protection),
    ));
}

#[test]
fn natural_leaf_decay_uses_vanilla_base_drop_pools() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:oak_leaves"),
        simple_block(2, "minecraft:jungle_leaves"),
        simple_block(3, "minecraft:pale_oak_leaves"),
        simple_block(4, "minecraft:mangrove_leaves"),
    ])
    .unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: Identifier::parse("minecraft:oak_sapling").unwrap(),
            protocol_id: 10,
        },
        ItemReport {
            id: Identifier::parse("minecraft:jungle_sapling").unwrap(),
            protocol_id: 11,
        },
        ItemReport {
            id: Identifier::parse("minecraft:pale_oak_sapling").unwrap(),
            protocol_id: 12,
        },
        ItemReport {
            id: Identifier::parse("minecraft:stick").unwrap(),
            protocol_id: 13,
        },
        ItemReport {
            id: Identifier::parse("minecraft:apple").unwrap(),
            protocol_id: 14,
        },
    ]);

    let all_pools = LeafDecayDropRolls {
        sapling: 0,
        stick: 0,
        apple: 0,
        stick_count: 2,
    };
    assert_eq!(
        natural_leaf_decay_drops(&blocks, &items, BlockStateId(1), all_pools),
        vec![
            ItemStack::new(10, 1),
            ItemStack::new(13, 2),
            ItemStack::new(14, 1),
        ]
    );
    assert_eq!(
        natural_leaf_decay_drops(&blocks, &items, BlockStateId(2), all_pools),
        vec![ItemStack::new(11, 1), ItemStack::new(13, 2)],
        "jungle leaves use the rarer sapling pool and never drop apples"
    );
    assert_eq!(
        natural_leaf_decay_drops(&blocks, &items, BlockStateId(3), all_pools),
        vec![ItemStack::new(12, 1), ItemStack::new(13, 2)],
        "pale oak leaves have no apple pool in the 26.1.2 table"
    );
    assert_eq!(
        natural_leaf_decay_drops(&blocks, &items, BlockStateId(4), all_pools),
        vec![ItemStack::new(13, 2)],
        "mangrove leaves do not drop propagules through decay"
    );

    let boundary_misses = LeafDecayDropRolls {
        sapling: 25,
        stick: 20,
        apple: 5,
        stick_count: 1,
    };
    assert!(
        natural_leaf_decay_drops(&blocks, &items, BlockStateId(2), boundary_misses).is_empty(),
        "vanilla chances are strict 2.5%, 2%, and 0.5% thresholds"
    );
}

#[test]
fn interactive_toggle_helpers_preserve_other_properties() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:oak_trapdoor").unwrap(),
            properties: prop_schema(&[
                ("facing", &["north"]),
                ("open", &["false", "true"]),
                ("waterlogged", &["false"]),
            ]),
            states: vec![
                state(
                    1,
                    true,
                    &[
                        ("facing", "north"),
                        ("open", "false"),
                        ("waterlogged", "false"),
                    ],
                ),
                state(
                    2,
                    false,
                    &[
                        ("facing", "north"),
                        ("open", "true"),
                        ("waterlogged", "false"),
                    ],
                ),
            ],
        },
        BlockReport {
            id: Identifier::parse("minecraft:lever").unwrap(),
            properties: prop_schema(&[("facing", &["north"]), ("powered", &["false", "true"])]),
            states: vec![
                state(3, true, &[("facing", "north"), ("powered", "false")]),
                state(4, false, &[("facing", "north"), ("powered", "true")]),
            ],
        },
    ])
    .unwrap();

    assert_eq!(
        toggled_bool_state(
            &blocks,
            blocks.by_id(mc_world::BlockStateId(1)).unwrap(),
            "open"
        ),
        Some(mc_world::BlockStateId(2))
    );
    assert_eq!(
        toggled_bool_state(
            &blocks,
            blocks.by_id(mc_world::BlockStateId(3)).unwrap(),
            "powered"
        ),
        Some(mc_world::BlockStateId(4))
    );
}

#[test]
fn hand_toggle_respects_door_and_trapdoor_material() {
    let blocks = Arc::new(hand_toggle_test_registry());
    let mut world = in_memory_button_world(Arc::clone(&blocks));
    let oak_lower = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let oak_upper = mc_world::BlockPos { y: 65, ..oak_lower };
    let iron_lower = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let iron_upper = mc_world::BlockPos {
        y: 65,
        ..iron_lower
    };
    let copper_lower = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    let copper_upper = mc_world::BlockPos {
        y: 65,
        ..copper_lower
    };
    let oak_trapdoor = mc_world::BlockPos { x: 4, y: 64, z: 1 };
    let iron_trapdoor = mc_world::BlockPos { x: 5, y: 64, z: 1 };
    let copper_trapdoor = mc_world::BlockPos { x: 6, y: 64, z: 1 };
    let oak_fence_gate = mc_world::BlockPos { x: 7, y: 64, z: 1 };

    for (pos, state_id) in [
        (oak_lower, 1),
        (oak_upper, 2),
        (iron_lower, 5),
        (iron_upper, 6),
        (copper_lower, 9),
        (copper_upper, 10),
        (oak_trapdoor, 13),
        (iron_trapdoor, 15),
        (copper_trapdoor, 17),
        (oak_fence_gate, 19),
    ] {
        world
            .set_block_at(pos, mc_world::BlockStateId(state_id))
            .expect("place toggle test block");
    }

    let oak_plan =
        plan_toggle_block_interaction(&blocks, &world, oak_lower, mc_world::BlockStateId(1), 0)
            .expect("oak door should open by hand");
    assert_eq!(
        oak_plan.edits,
        vec![
            BlockEdit {
                pos: oak_lower,
                new_state: mc_world::BlockStateId(3),
            },
            BlockEdit {
                pos: oak_upper,
                new_state: mc_world::BlockStateId(4),
            },
        ]
    );

    let copper_plan =
        plan_toggle_block_interaction(&blocks, &world, copper_lower, mc_world::BlockStateId(9), 0)
            .expect("copper door should open by hand");
    assert_eq!(
        copper_plan.edits,
        vec![
            BlockEdit {
                pos: copper_lower,
                new_state: mc_world::BlockStateId(11),
            },
            BlockEdit {
                pos: copper_upper,
                new_state: mc_world::BlockStateId(12),
            },
        ]
    );

    assert!(
        plan_toggle_block_interaction(&blocks, &world, iron_lower, mc_world::BlockStateId(5), 0,)
            .is_none(),
        "iron door must not open by hand"
    );

    let oak_trapdoor_plan =
        plan_toggle_block_interaction(&blocks, &world, oak_trapdoor, mc_world::BlockStateId(13), 0)
            .expect("oak trapdoor should open by hand");
    assert_eq!(
        oak_trapdoor_plan.edits,
        vec![BlockEdit {
            pos: oak_trapdoor,
            new_state: mc_world::BlockStateId(14),
        }]
    );

    let copper_trapdoor_plan = plan_toggle_block_interaction(
        &blocks,
        &world,
        copper_trapdoor,
        mc_world::BlockStateId(17),
        0,
    )
    .expect("copper trapdoor should open by hand");
    assert_eq!(
        copper_trapdoor_plan.edits,
        vec![BlockEdit {
            pos: copper_trapdoor,
            new_state: mc_world::BlockStateId(18),
        }]
    );

    assert!(
        plan_toggle_block_interaction(
            &blocks,
            &world,
            iron_trapdoor,
            mc_world::BlockStateId(15),
            0,
        )
        .is_none(),
        "iron trapdoor must not open by hand"
    );

    let fence_gate_plan = plan_toggle_block_interaction(
        &blocks,
        &world,
        oak_fence_gate,
        mc_world::BlockStateId(19),
        0,
    )
    .expect("oak fence gate should open by hand");
    assert_eq!(
        fence_gate_plan.edits,
        vec![BlockEdit {
            pos: oak_fence_gate,
            new_state: mc_world::BlockStateId(20),
        }]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn two_client_door_and_trapdoor_toggles_converge_and_reject_stale_retry() {
    let blocks = Arc::new(hand_toggle_test_registry());
    let mut storage = in_memory_button_world(Arc::clone(&blocks));
    let door_lower = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let door_upper = mc_world::BlockPos {
        y: 65,
        ..door_lower
    };
    let trapdoor = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    for (pos, state) in [
        (door_lower, BlockStateId(1)),
        (door_upper, BlockStateId(2)),
        (trapdoor, BlockStateId(13)),
    ] {
        storage
            .set_block_at(pos, state)
            .expect("seed hand-toggle state");
    }

    let door_plan =
        plan_toggle_block_interaction(&blocks, &storage, door_lower, BlockStateId(1), 0)
            .expect("closed oak door plans one atomic two-half toggle");
    let trapdoor_plan =
        plan_toggle_block_interaction(&blocks, &storage, trapdoor, BlockStateId(13), 0)
            .expect("closed oak trapdoor plans one toggle");
    assert_eq!(door_plan.preconditions.len(), 2);
    assert_eq!(trapdoor_plan.preconditions.len(), 1);

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = SessionRegistry::new();
    let (actor_tx, mut actor_rx) = mpsc::channel(16);
    let (observer_tx, mut observer_rx) = mpsc::channel(16);
    let actor_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(1001),
        name: "DoorActor".to_owned(),
    };
    let observer_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(1002),
        name: "DoorObserver".to_owned(),
    };
    let (actor, _) = sessions.register(
        &actor_profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        actor_tx,
        PlayerPose::new(1.5, 64.0, 3.5),
    );
    let (observer, _) = sessions.register(
        &observer_profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        observer_tx,
        PlayerPose::new(4.5, 64.0, 3.5),
    );
    let mut setup = sessions.mark_loaded(actor, (0, 0));
    setup.extend(sessions.mark_loaded(observer, (0, 0)));
    dispatch_and_clear_setup_packets(setup, &mut [&mut actor_rx, &mut observer_rx]);

    let (handle, mut owner) = simulation_channel();
    let actor_handle = handle.for_session(actor);
    let mut door_request = Box::pin(
        actor_handle.apply_block_edits(door_plan.edits.clone(), door_plan.preconditions.clone()),
    );
    let waker = std::task::Waker::noop();
    let mut context = Context::from_waker(waker);
    assert!(matches!(
        std::future::Future::poll(door_request.as_mut(), &mut context),
        Poll::Pending
    ));
    owner
        .process_commands_with_world(&sessions, Some(&world), None, 1)
        .await;
    assert!(door_request.await.unwrap().is_some());
    let door_deltas = match observer_rx.try_recv().expect("observer door publication") {
        OutboundCommand::BlockDeltas(deltas) => deltas,
        other => panic!("expected observer door BlockDeltas, got {other:?}"),
    };
    let expected_door_deltas = vec![
        BlockDelta {
            x: door_lower.x,
            y: door_lower.y,
            z: door_lower.z,
            state_id: BlockStateId(3),
        },
        BlockDelta {
            x: door_upper.x,
            y: door_upper.y,
            z: door_upper.z,
            state_id: BlockStateId(4),
        },
    ];
    assert_eq!(door_deltas, expected_door_deltas);
    assert_eq!(
        plan_block_delta_packets(&door_deltas),
        vec![BlockDeltaPacket::Section {
            section_x: 0,
            section_y: 4,
            section_z: 0,
            changes: door_deltas.clone(),
        }]
    );
    let mut wire = Vec::new();
    send_block_deltas(&mut wire, Compression::Disabled, &door_deltas, None)
        .await
        .expect("encode observer door deltas");
    let mut frames = bytes::BytesMut::from(wire.as_slice());
    let mut frame = mc_protocol::frame::try_decode_frame(&mut frames, Compression::Disabled)
        .expect("decode observer door frame")
        .expect("observer door frame");
    assert_eq!(frame.id, SectionBlocksUpdate::ID);
    assert_eq!(
        SectionBlocksUpdate::decode(&mut frame.body).expect("decode section door update"),
        SectionBlocksUpdate {
            section_pos: mc_protocol::packets::play::pack_section_pos(0, 4, 0),
            changes: vec![
                SectionBlockChange {
                    relative_pos: mc_protocol::packets::play::pack_section_relative_pos(
                        door_lower.x,
                        door_lower.y,
                        door_lower.z,
                    ),
                    state_id: 3,
                },
                SectionBlockChange {
                    relative_pos: mc_protocol::packets::play::pack_section_relative_pos(
                        door_upper.x,
                        door_upper.y,
                        door_upper.z,
                    ),
                    state_id: 4,
                },
            ],
        }
    );
    assert!(frames.is_empty());

    let mut trapdoor_request = Box::pin(actor_handle.apply_block_edits(
        trapdoor_plan.edits.clone(),
        trapdoor_plan.preconditions.clone(),
    ));
    assert!(matches!(
        std::future::Future::poll(trapdoor_request.as_mut(), &mut context),
        Poll::Pending
    ));
    owner
        .process_commands_with_world(&sessions, Some(&world), None, 1)
        .await;
    assert!(trapdoor_request.await.unwrap().is_some());
    let trapdoor_deltas = match observer_rx
        .try_recv()
        .expect("observer trapdoor publication")
    {
        OutboundCommand::BlockDeltas(deltas) => deltas,
        other => panic!("expected observer trapdoor BlockDeltas, got {other:?}"),
    };
    assert_eq!(
        trapdoor_deltas,
        vec![BlockDelta {
            x: trapdoor.x,
            y: trapdoor.y,
            z: trapdoor.z,
            state_id: BlockStateId(14),
        }]
    );
    let mut wire = Vec::new();
    send_block_deltas(&mut wire, Compression::Disabled, &trapdoor_deltas, None)
        .await
        .expect("encode actor trapdoor delta");
    let mut frames = bytes::BytesMut::from(wire.as_slice());
    let mut frame = mc_protocol::frame::try_decode_frame(&mut frames, Compression::Disabled)
        .expect("decode actor trapdoor frame")
        .expect("actor trapdoor frame");
    assert_eq!(frame.id, BlockUpdate::ID);
    assert_eq!(
        BlockUpdate::decode(&mut frame.body).expect("decode actor trapdoor update"),
        BlockUpdate {
            position: pack_block_pos(trapdoor.x, trapdoor.y, trapdoor.z),
            state_id: 14,
        }
    );
    assert!(frames.is_empty());

    assert!(actor_rx.try_recv().is_err());
    assert!(observer_rx.try_recv().is_err());

    let mut stale_retry =
        Box::pin(actor_handle.apply_block_edits(door_plan.edits, door_plan.preconditions));
    assert!(matches!(
        std::future::Future::poll(stale_retry.as_mut(), &mut context),
        Poll::Pending
    ));
    owner
        .process_commands_with_world(&sessions, Some(&world), None, 1)
        .await;
    assert!(stale_retry.await.unwrap().is_none());
    assert!(observer_rx.try_recv().is_err());
    assert!(actor_rx.try_recv().is_err());
    owner
        .process_commands_with_world(&sessions, Some(&world), None, 2)
        .await;
    assert!(observer_rx.try_recv().is_err());
    assert!(actor_rx.try_recv().is_err());

    let storage = world.lock().await;
    assert_eq!(storage.get_cached_block(door_lower), Some(BlockStateId(3)));
    assert_eq!(storage.get_cached_block(door_upper), Some(BlockStateId(4)));
    assert_eq!(storage.get_cached_block(trapdoor), Some(BlockStateId(14)));
    for (pos, expected_half) in [(door_lower, "lower"), (door_upper, "upper")] {
        let state = blocks
            .by_id(
                storage
                    .get_cached_block(pos)
                    .expect("door half remains loaded"),
            )
            .expect("door half state remains registered");
        assert_eq!(block_state_property(state, "facing"), Some("north"));
        assert_eq!(block_state_property(state, "half"), Some(expected_half));
        assert_eq!(block_state_property(state, "open"), Some("true"));
    }
    let state = blocks
        .by_id(
            storage
                .get_cached_block(trapdoor)
                .expect("trapdoor remains loaded"),
        )
        .expect("trapdoor state remains registered");
    assert_eq!(block_state_property(state, "facing"), Some("north"));
    assert_eq!(block_state_property(state, "half"), Some("bottom"));
    assert_eq!(block_state_property(state, "open"), Some("true"));
}

#[test]
fn lever_toggle_powers_adjacent_iron_door() {
    let blocks = Arc::new(button_and_door_test_registry());
    let mut world = in_memory_button_world(Arc::clone(&blocks));
    let lever = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let door_lower = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let door_upper = mc_world::BlockPos {
        y: 65,
        ..door_lower
    };
    for (pos, state_id) in [(lever, 7), (door_lower, 3), (door_upper, 4)] {
        world
            .set_block_at(pos, mc_world::BlockStateId(state_id))
            .expect("place lever propagation test block");
    }

    let plan = plan_toggle_block_interaction(&blocks, &world, lever, mc_world::BlockStateId(7), 0)
        .expect("lever should power adjacent iron door");

    assert_eq!(
        plan.edits,
        vec![
            BlockEdit {
                pos: lever,
                new_state: mc_world::BlockStateId(8),
            },
            BlockEdit {
                pos: door_lower,
                new_state: mc_world::BlockStateId(5),
            },
            BlockEdit {
                pos: door_upper,
                new_state: mc_world::BlockStateId(6),
            },
        ]
    );
    assert!(plan.scheduled_block_ticks.is_empty());
}

#[test]
fn lever_extends_one_block_piston_and_retracts_the_head() {
    let blocks = Arc::new(piston_test_registry());
    let mut world = in_memory_button_world(Arc::clone(&blocks));
    let lever = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let piston = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let arm = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    let destination = mc_world::BlockPos { x: 4, y: 64, z: 1 };
    for (pos, state_id) in [(lever, 3), (piston, 5), (arm, 8)] {
        world
            .set_block_at(pos, mc_world::BlockStateId(state_id))
            .expect("place piston test block");
    }

    let extend =
        plan_toggle_block_interaction(&blocks, &world, lever, mc_world::BlockStateId(3), 0)
            .expect("lever should extend adjacent piston");
    assert_eq!(
        extend.edits,
        vec![
            BlockEdit {
                pos: lever,
                new_state: mc_world::BlockStateId(4),
            },
            BlockEdit {
                pos: piston,
                new_state: mc_world::BlockStateId(6),
            },
            BlockEdit {
                pos: destination,
                new_state: mc_world::BlockStateId(8),
            },
            BlockEdit {
                pos: arm,
                new_state: mc_world::BlockStateId(7),
            },
        ]
    );
    apply_block_edit_batch_to_storage_conditionally(
        &mut world,
        None,
        &extend.edits,
        &extend.preconditions,
    )
    .expect("extension plan remains current");

    let other_lever = mc_world::BlockPos { x: 2, y: 65, z: 1 };
    world
        .set_block_at(other_lever, mc_world::BlockStateId(3))
        .expect("place alternate piston control");
    let stale_retract =
        plan_toggle_block_interaction(&blocks, &world, lever, mc_world::BlockStateId(4), 1)
            .expect("lever should retract adjacent piston");
    assert_eq!(
        stale_retract.edits,
        vec![
            BlockEdit {
                pos: lever,
                new_state: mc_world::BlockStateId(3),
            },
            BlockEdit {
                pos: piston,
                new_state: mc_world::BlockStateId(5),
            },
            BlockEdit {
                pos: arm,
                new_state: mc_world::BlockStateId(0),
            },
        ]
    );
    world
        .set_block_at(other_lever, mc_world::BlockStateId(4))
        .expect("power alternate piston control");
    assert!(
        apply_block_edit_batch_to_storage_conditionally(
            &mut world,
            None,
            &stale_retract.edits,
            &stale_retract.preconditions,
        )
        .is_none(),
        "alternate power change must stale the retraction"
    );
    assert_eq!(
        world.get_cached_block(piston),
        Some(mc_world::BlockStateId(6))
    );
    world
        .set_block_at(other_lever, mc_world::BlockStateId(3))
        .expect("release alternate piston control");
    let retract =
        plan_toggle_block_interaction(&blocks, &world, lever, mc_world::BlockStateId(4), 2)
            .expect("released alternate control permits retraction");
    apply_block_edit_batch_to_storage_conditionally(
        &mut world,
        None,
        &retract.edits,
        &retract.preconditions,
    )
    .expect("retraction plan remains current");
    assert_eq!(
        world.get_cached_block(destination),
        Some(mc_world::BlockStateId(8))
    );
}

#[test]
fn empty_piston_extends_with_an_occupied_block_two_spaces_ahead() {
    let blocks = Arc::new(piston_test_registry());
    let mut world = in_memory_button_world(Arc::clone(&blocks));
    let lever = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let piston = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let arm = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    let destination = mc_world::BlockPos { x: 4, y: 64, z: 1 };
    for (pos, state_id) in [(lever, 3), (piston, 5), (destination, 8)] {
        world
            .set_block_at(pos, mc_world::BlockStateId(state_id))
            .expect("place empty piston test block");
    }

    let plan = plan_toggle_block_interaction(&blocks, &world, lever, mc_world::BlockStateId(3), 0)
        .expect("empty piston should extend");

    assert_eq!(
        plan.edits,
        vec![
            BlockEdit {
                pos: lever,
                new_state: mc_world::BlockStateId(4),
            },
            BlockEdit {
                pos: piston,
                new_state: mc_world::BlockStateId(6),
            },
            BlockEdit {
                pos: arm,
                new_state: mc_world::BlockStateId(7),
            },
        ]
    );
}

#[test]
fn protected_piston_destination_rejects_the_atomic_piston_group() {
    let blocks = Arc::new(piston_test_registry());
    let mut world = in_memory_button_world(Arc::clone(&blocks));
    let lever = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let piston = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let arm = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    let destination = mc_world::BlockPos { x: 4, y: 64, z: 1 };
    for (pos, state_id) in [(lever, 3), (piston, 5), (arm, 8)] {
        world
            .set_block_at(pos, mc_world::BlockStateId(state_id))
            .expect("place protected piston test block");
    }
    let zone = mc_script::ScriptAxisAlignedZone::try_new_with_protection(
        "piston-destination",
        "minecraft:overworld",
        mc_script::ScriptPosition::try_new(4.0, 64.0, 1.0).unwrap(),
        mc_script::ScriptPosition::try_new(4.0, 64.0, 1.0).unwrap(),
        Some(
            mc_script::ScriptZoneProtection::try_actor_or_operator(
                "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            )
            .unwrap(),
        ),
    )
    .unwrap();
    let protection = crate::script::ZoneProtectionSnapshot::from_zones(vec![zone]);

    let plan = plan_toggle_block_interaction_with_protection(
        &blocks,
        &world,
        lever,
        mc_world::BlockStateId(3),
        0,
        Some(&protection),
    )
    .expect("direct lever edit remains valid");

    assert_eq!(
        plan.edits,
        vec![BlockEdit {
            pos: lever,
            new_state: mc_world::BlockStateId(4),
        }]
    );
    assert_eq!(
        world.get_cached_block(piston),
        Some(mc_world::BlockStateId(5))
    );
    assert_eq!(world.get_cached_block(arm), Some(mc_world::BlockStateId(8)));
    assert_eq!(
        world.get_cached_block(destination),
        Some(mc_world::BlockStateId(0))
    );
}

#[test]
#[ignore = "explicit local 26.1.2 blocks sidecar parity gate"]
fn real_door_states_plan_hand_toggle_when_sidecar_is_present() {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let blocks_json = manifest.join("../../data/vanilla/reports/blocks.json");
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&report).expect("registry builds"));
    let mut world = in_memory_button_world(Arc::clone(&blocks));
    let lower = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let upper = mc_world::BlockPos { y: 65, ..lower };
    let oak_lower = real_door_state(&blocks, "minecraft:oak_door", "lower", false);
    let oak_upper = real_door_state(&blocks, "minecraft:oak_door", "upper", false);
    let oak_open = real_door_state(&blocks, "minecraft:oak_door", "lower", true);
    let oak_upper_open = real_door_state(&blocks, "minecraft:oak_door", "upper", true);
    world
        .set_block_at(lower, oak_lower)
        .expect("set lower")
        .expect("chunk exists");
    world
        .set_block_at(upper, oak_upper)
        .expect("set upper")
        .expect("chunk exists");

    let plan = plan_toggle_block_interaction(&blocks, &world, lower, oak_lower, 0)
        .expect("real oak door should hand-toggle");

    assert_eq!(
        plan.edits,
        vec![
            BlockEdit {
                pos: lower,
                new_state: oak_open,
            },
            BlockEdit {
                pos: upper,
                new_state: oak_upper_open,
            },
        ]
    );
}

#[test]
fn button_press_schedules_release_tick_without_global_scan() {
    let blocks = Arc::new(button_test_registry());
    let mut world = in_memory_button_world(Arc::clone(&blocks));
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    world
        .set_block_at(pos, mc_world::BlockStateId(1))
        .expect("place unpowered button");

    let plan = plan_toggle_block_interaction(&blocks, &world, pos, mc_world::BlockStateId(1), 100)
        .expect("button press should be interactive");

    assert_eq!(
        plan.edits,
        vec![BlockEdit {
            pos,
            new_state: mc_world::BlockStateId(2)
        }]
    );
    assert_eq!(plan.preconditions.len(), 1);
    assert_eq!(plan.preconditions[0].pos, pos);
    assert_eq!(
        plan.preconditions[0].expected_state,
        mc_world::BlockStateId(1)
    );
    assert_eq!(plan.scheduled_block_ticks.len(), 1);
    assert_eq!(plan.scheduled_block_ticks[0].pos, pos);
    assert_eq!(plan.scheduled_block_ticks[0].trigger_tick, 120);
    let ticks = world
        .scheduled_block_ticks(ChunkPos { x: 0, z: 0 })
        .expect("read scheduled block ticks")
        .expect("loaded chunk should expose ticks");
    assert!(ticks.is_empty(), "planning must not mutate world storage");
}

#[test]
fn button_press_does_not_materialize_unloaded_adjacent_chunks() {
    struct CountingAirGenerator {
        calls: Arc<AtomicUsize>,
    }

    impl mc_world::ChunkGenerator for CountingAirGenerator {
        fn generate(&self, pos: ChunkPos) -> Chunk {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let mut chunk = Chunk::empty(
                pos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            );
            chunk.status = "minecraft:full".into();
            chunk.dirty = true;
            chunk
        }
    }

    let blocks = Arc::new(button_test_registry());
    let generated_chunks = Arc::new(AtomicUsize::new(0));
    let mut world = in_memory_button_world(Arc::clone(&blocks)).with_generator(Arc::new(
        CountingAirGenerator {
            calls: Arc::clone(&generated_chunks),
        },
    ));
    let pos = mc_world::BlockPos { x: 15, y: 64, z: 1 };
    world
        .set_block_at(pos, mc_world::BlockStateId(1))
        .expect("place edge button");

    let plan = plan_toggle_block_interaction(&blocks, &world, pos, mc_world::BlockStateId(1), 100)
        .expect("edge button press should be interactive");

    assert_eq!(
        plan.edits,
        vec![BlockEdit {
            pos,
            new_state: mc_world::BlockStateId(2)
        }]
    );
    assert_eq!(generated_chunks.load(Ordering::Relaxed), 0);
    assert_eq!(world.cache_len(), 1);
}

#[test]
fn powered_button_press_is_consumed_without_duplicate_release_tick() {
    let blocks = Arc::new(button_test_registry());
    let mut world = in_memory_button_world(Arc::clone(&blocks));
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    world
        .set_block_at(pos, mc_world::BlockStateId(2))
        .expect("place powered button");
    world
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            pos,
            Identifier::parse("minecraft:stone_button").unwrap(),
            120,
            0,
        ))
        .expect("schedule existing button release");

    let plan = plan_toggle_block_interaction(&blocks, &world, pos, mc_world::BlockStateId(2), 105)
        .expect("already powered button should still consume the interaction");

    assert!(plan.edits.is_empty());
    let ticks = world
        .scheduled_block_ticks(ChunkPos { x: 0, z: 0 })
        .expect("read scheduled block ticks")
        .expect("loaded chunk should expose ticks");
    assert_eq!(ticks.len(), 1);
    assert_eq!(ticks[0].trigger_tick, 120);
}

#[tokio::test]
async fn scheduled_button_tick_releases_powered_button() {
    let blocks = Arc::new(button_test_registry());
    let world = Arc::new(tokio::sync::Mutex::new(in_memory_button_world(Arc::clone(
        &blocks,
    ))));
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = world.lock().await;
        storage
            .set_block_at(pos, mc_world::BlockStateId(2))
            .expect("place powered button");
        storage
            .schedule_block_tick(mc_world::ScheduledBlockTick::new(
                pos,
                Identifier::parse("minecraft:stone_button").unwrap(),
                120,
                0,
            ))
            .expect("schedule button release");
    }
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "ButtonRelease");
    let storage = world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let (_simulation, owner) = simulation_channel();
    let resources = ChunkPipelineResources::with_limits(1, 1);
    let world_writer = world.lock().await;
    let block_tick = owner.run_scheduled_block_ticks_with_budget(
        &config,
        &sessions,
        SimulationWorldAccess {
            read: Some(&world_read),
            mutation: Some(&world_mutation),
            cpu: Some(&resources),
            light: config.block_light.as_ref(),
        },
        120,
        1,
    );
    let report = tokio::time::timeout(Duration::from_secs(1), block_tick)
        .await
        .expect("resident scheduled-block commit must not wait for the world writer");
    drop(world_writer);

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    let storage = world.lock().await;
    assert_eq!(
        storage.get_cached_block(pos),
        Some(mc_world::BlockStateId(1))
    );
}

#[tokio::test]
async fn scheduled_buttons_in_distinct_regions_do_not_wait_for_world_writer() {
    let blocks = Arc::new(button_test_registry());
    let mut storage = in_memory_button_world(Arc::clone(&blocks));
    let east_chunk = ChunkPos { x: 8, z: 0 };
    storage
        .insert_generated_chunk(
            east_chunk,
            Chunk::empty(
                east_chunk,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let positions = [
        mc_world::BlockPos { x: 1, y: 64, z: 1 },
        mc_world::BlockPos {
            x: 8 * 16 + 1,
            y: 64,
            z: 1,
        },
    ];
    for position in positions {
        storage
            .set_block_at(position, mc_world::BlockStateId(2))
            .expect("place powered button");
        storage
            .schedule_block_tick(mc_world::ScheduledBlockTick::new(
                position,
                Identifier::parse("minecraft:stone_button").unwrap(),
                120,
                0,
            ))
            .expect("schedule button release");
    }
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let session = register_ticketed_button_session(&sessions, "RegionalButtonRelease");
    let _ = sessions.mark_loaded(session, (0, 0));
    let _ = sessions.mark_loaded(session, (8, 0));
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);
    let storage = world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let (_simulation, mut owner) = simulation_channel();
    let resources = ChunkPipelineResources::with_limits(1, 2);
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    owner.install_regional_block_edit_probe(entered_tx, release_rx);
    let world_writer = world.lock().await;
    let mut block_tick = Box::pin(owner.run_scheduled_block_ticks_with_budget(
        &config,
        &sessions,
        SimulationWorldAccess {
            read: Some(&world_read),
            mutation: Some(&world_mutation),
            cpu: Some(&resources),
            light: config.block_light.as_ref(),
        },
        120,
        2,
    ));
    let entered_task = tokio::task::spawn_blocking(move || {
        [entered_rx.recv().unwrap(), entered_rx.recv().unwrap()]
    });
    let entered = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::select! {
            entered = entered_task => entered.unwrap(),
            _ = &mut block_tick => {
                panic!("scheduled regional fanout completed before worker probe")
            }
        }
    })
    .await
    .expect("both scheduled regional workers enter before either release");
    assert_ne!(entered[0], entered[1]);
    release_tx.send(()).unwrap();
    release_tx.send(()).unwrap();
    let report = tokio::time::timeout(Duration::from_secs(1), block_tick)
        .await
        .expect("distinct resident regions complete without the world writer");

    assert_eq!(report.drained, 2);
    assert_eq!(report.applied, 2);
    let (reopened, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert_eq!(pending.len(), 1, "one regional wave uses one WAL decision");
    let restored = reopened.decode_pending(&pending).unwrap();
    assert_eq!(restored.len(), 2);
    for position in positions {
        let chunk_pos = ChunkPos {
            x: position.x.div_euclid(16),
            z: position.z.div_euclid(16),
        };
        let chunk = restored
            .iter()
            .find(|chunk| chunk.pos == chunk_pos)
            .expect("journaled regional button chunk");
        assert_eq!(
            chunk.get_block(
                position.x.rem_euclid(16) as u8,
                position.y,
                position.z.rem_euclid(16) as u8,
            ),
            Some(mc_world::BlockStateId(1))
        );
        assert!(chunk.scheduled_block_ticks().is_empty());
    }
    drop(world_writer);
    let storage = world.lock().await;
    for position in positions {
        assert_eq!(
            storage.get_cached_block(position),
            Some(mc_world::BlockStateId(1))
        );
    }
}

#[tokio::test]
async fn scheduled_button_regions_replan_when_region_order_repeats() {
    let blocks = Arc::new(button_and_door_test_registry());
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    for chunk in [
        ChunkPos { x: 0, z: 0 },
        ChunkPos { x: 8, z: 0 },
        ChunkPos { x: 0, z: 1 },
    ] {
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
    }
    let first_button = mc_world::BlockPos { x: 1, y: 64, z: 15 };
    let middle_button = mc_world::BlockPos {
        x: 8 * 16 + 1,
        y: 64,
        z: 1,
    };
    let last_button = mc_world::BlockPos { x: 1, y: 64, z: 17 };
    let lower_door = mc_world::BlockPos { x: 1, y: 64, z: 16 };
    let upper_door = mc_world::BlockPos {
        y: 65,
        ..lower_door
    };
    let positions = [first_button, middle_button, last_button];
    for position in positions {
        storage.set_block_at(position, BlockStateId(2)).unwrap();
        storage
            .schedule_block_tick(ScheduledBlockTick::new(
                position,
                Identifier::parse("minecraft:stone_button").unwrap(),
                120,
                0,
            ))
            .unwrap();
    }
    storage.set_block_at(lower_door, BlockStateId(5)).unwrap();
    storage.set_block_at(upper_door, BlockStateId(6)).unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(122),
        name: "RepeatedScheduledRegion".to_string(),
    };
    let loaded = HashSet::from([(0, 0), (8, 0), (0, 1)]);
    let (tx, _rx) = mpsc::channel(16);
    let (session, _) = sessions.register(
        &profile,
        (0, 0),
        16,
        loaded.clone(),
        tx,
        PlayerPose::new(1.5, 64.0, 1.5),
    );
    for chunk in loaded {
        let _ = sessions.mark_loaded(session, chunk);
    }
    let storage = world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let (_simulation, owner) = simulation_channel();
    let resources = ChunkPipelineResources::with_limits(1, 2);
    let world_writer = world.lock().await;
    let report = owner
        .run_scheduled_block_ticks_with_budget(
            &config,
            &sessions,
            SimulationWorldAccess {
                read: Some(&world_read),
                mutation: Some(&world_mutation),
                cpu: Some(&resources),
                light: config.block_light.as_ref(),
            },
            120,
            3,
        )
        .await;
    drop(world_writer);

    assert_eq!(report.drained, 3);
    assert_eq!(report.applied, 5);
    let mut storage = world.lock().await;
    for position in positions {
        assert_eq!(storage.get_cached_block(position), Some(BlockStateId(1)));
        assert!(
            storage
                .scheduled_block_ticks(ChunkPos {
                    x: position.x.div_euclid(16),
                    z: position.z.div_euclid(16),
                })
                .unwrap()
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(storage.get_cached_block(lower_door), Some(BlockStateId(3)));
    assert_eq!(storage.get_cached_block(upper_door), Some(BlockStateId(4)));
}

#[tokio::test]
async fn scheduled_button_crossing_region_boundary_commits_without_world_storage() {
    let blocks = Arc::new(button_and_door_test_registry());
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    for chunk in [ChunkPos { x: 7, z: 0 }, ChunkPos { x: 8, z: 0 }] {
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
    }
    let button = mc_world::BlockPos {
        x: 8 * 16 - 1,
        y: 64,
        z: 1,
    };
    let lower_door = mc_world::BlockPos {
        x: 8 * 16,
        y: 64,
        z: 1,
    };
    let upper_door = mc_world::BlockPos {
        y: 65,
        ..lower_door
    };
    storage.set_block_at(button, BlockStateId(2)).unwrap();
    storage.set_block_at(lower_door, BlockStateId(5)).unwrap();
    storage.set_block_at(upper_door, BlockStateId(6)).unwrap();
    storage
        .schedule_block_tick(ScheduledBlockTick::new(
            button,
            Identifier::parse("minecraft:stone_button").unwrap(),
            120,
            0,
        ))
        .unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(120),
        name: "BoundaryButtonRelease".to_string(),
    };
    let (tx, _rx) = mpsc::channel(16);
    let (session, _) = sessions.register(
        &profile,
        (7, 0),
        1,
        HashSet::from([(7, 0), (8, 0)]),
        tx,
        PlayerPose::new(127.5, 64.0, 1.5),
    );
    let _ = sessions.mark_loaded(session, (7, 0));
    let _ = sessions.mark_loaded(session, (8, 0));
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);
    let storage = world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let (_simulation, owner) = simulation_channel();
    let resources = ChunkPipelineResources::with_limits(1, 2);
    let world_writer = world.lock().await;
    let report = tokio::time::timeout(
        Duration::from_secs(5),
        owner.run_scheduled_block_ticks_with_budget(
            &config,
            &sessions,
            SimulationWorldAccess {
                read: Some(&world_read),
                mutation: Some(&world_mutation),
                cpu: Some(&resources),
                light: config.block_light.as_ref(),
            },
            120,
            1,
        ),
    )
    .await
    .expect("cross-region scheduled block transaction must not wait for WorldStorage");
    drop(world_writer);

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 3);
    let (reopened, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert_eq!(pending.len(), 1, "boundary commit uses one WAL decision");
    let restored = reopened.decode_pending(&pending).unwrap();
    assert_eq!(restored.len(), 2);
    let west = restored
        .iter()
        .find(|chunk| chunk.pos == ChunkPos { x: 7, z: 0 })
        .unwrap();
    let east = restored
        .iter()
        .find(|chunk| chunk.pos == ChunkPos { x: 8, z: 0 })
        .unwrap();
    assert_eq!(west.get_block(15, 64, 1), Some(BlockStateId(1)));
    assert!(west.scheduled_block_ticks().is_empty());
    assert_eq!(east.get_block(0, 64, 1), Some(BlockStateId(3)));
    assert_eq!(east.get_block(0, 65, 1), Some(BlockStateId(4)));
    let storage = world.lock().await;
    assert_eq!(storage.get_cached_block(button), Some(BlockStateId(1)));
    assert_eq!(storage.get_cached_block(lower_door), Some(BlockStateId(3)));
    assert_eq!(storage.get_cached_block(upper_door), Some(BlockStateId(4)));
}

#[tokio::test]
async fn aborted_cross_region_scheduled_task_finishes_reserved_transaction() {
    let blocks = Arc::new(button_and_door_test_registry());
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let west_chunk = ChunkPos { x: 7, z: 0 };
    let east_chunk = ChunkPos { x: 8, z: 0 };
    for chunk in [west_chunk, east_chunk] {
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
    }
    let west = mc_world::BlockPos {
        x: 127,
        y: 64,
        z: 1,
    };
    let east = mc_world::BlockPos {
        x: 128,
        y: 64,
        z: 1,
    };
    storage.set_block_at(west, BlockStateId(1)).unwrap();
    storage.set_block_at(east, BlockStateId(1)).unwrap();
    let due = ScheduledBlockTick::new(west, Identifier::parse("minecraft:stone").unwrap(), 20, 0);
    storage.schedule_block_tick(due.clone()).unwrap();
    let west_token = storage.block_mutation_token(west).unwrap();
    let east_token = storage.block_mutation_token(east).unwrap();
    let mutation = storage.mutation_view();
    let read = storage.read_view();

    let sessions = Arc::new(SessionRegistry::new());
    let (requests, receiver) = std::sync::mpsc::sync_channel(4);
    let (append_started_tx, append_started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let (appended_tx, appended_rx) = tokio::sync::oneshot::channel();
    let worker = std::thread::spawn(move || {
        let super::world_journal::WriterRequest::Replace { reply, .. } = receiver.recv().unwrap()
        else {
            panic!("expected reservation write");
        };
        reply.send(Ok(())).unwrap();
        let super::world_journal::WriterRequest::Append { reply, .. } = receiver.recv().unwrap()
        else {
            panic!("expected decision append");
        };
        append_started_tx.send(()).unwrap();
        release_rx.blocking_recv().unwrap();
        reply.send(Ok(())).unwrap();
        appended_tx.send(()).unwrap();
        let super::world_journal::WriterRequest::Shutdown { reply } = receiver.recv().unwrap()
        else {
            panic!("expected journal shutdown");
        };
        reply.send(()).unwrap();
    });
    let journal = super::world_journal::WorldChunkJournal::from_parts_for_test(
        std::path::PathBuf::from("abort-cross-region-journal"),
        Arc::clone(&blocks),
        Arc::new(mc_data::items::solaris_required_items()),
        requests,
        worker,
    );
    sessions.install_world_chunk_journal(journal.clone());

    let task_sessions = Arc::clone(&sessions);
    let task_mutation = mutation.clone();
    let task = tokio::spawn(async move {
        let edits = [
            mc_world::ResidentBlockEdit {
                pos: west,
                new_state: BlockStateId(0),
                preserve_light: true,
            },
            mc_world::ResidentBlockEdit {
                pos: east,
                new_state: BlockStateId(0),
                preserve_light: true,
            },
        ];
        let preconditions = [
            mc_world::ResidentBlockPrecondition {
                pos: west,
                expected_state: BlockStateId(1),
                expected_token: west_token,
            },
            mc_world::ResidentBlockPrecondition {
                pos: east,
                expected_state: BlockStateId(1),
                expected_token: east_token,
            },
        ];
        commit_cross_region_scheduled_block_tick(
            &task_sessions,
            &task_mutation,
            20,
            ResidentBlockCommit {
                edits: &edits,
                preconditions: &preconditions,
                consumed_block_ticks: std::slice::from_ref(&due),
                consumed_fluid_ticks: &[],
                scheduled_fluid_ticks: &[],
                light_table: None,
                leaf_trigger_tick: None,
            },
        )
        .await
    });
    append_started_rx.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    release_tx.send(()).unwrap();
    appended_rx.await.unwrap();

    assert_eq!(mutation.schedule_fluid_ticks(&[]), 0);
    assert_eq!(read.get_cached_block(west), Some(BlockStateId(0)));
    assert_eq!(read.get_cached_block(east), Some(BlockStateId(0)));
    assert_eq!(journal.watermark(), Some(1));
    assert_eq!(storage.plan_dirty_flush().unwrap().chunk_count(), 2);
}

#[tokio::test]
async fn known_cross_region_append_failure_closes_reserved_decision_empty() {
    let blocks = Arc::new(button_and_door_test_registry());
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let west_chunk = ChunkPos { x: 7, z: 0 };
    let east_chunk = ChunkPos { x: 8, z: 0 };
    for chunk in [west_chunk, east_chunk] {
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
    }
    let west = mc_world::BlockPos {
        x: 127,
        y: 64,
        z: 1,
    };
    let east = mc_world::BlockPos {
        x: 128,
        y: 64,
        z: 1,
    };
    storage.set_block_at(west, BlockStateId(1)).unwrap();
    storage.set_block_at(east, BlockStateId(1)).unwrap();
    let due = ScheduledBlockTick::new(west, Identifier::parse("minecraft:stone").unwrap(), 20, 0);
    storage.schedule_block_tick(due.clone()).unwrap();
    let west_token = storage.block_mutation_token(west).unwrap();
    let east_token = storage.block_mutation_token(east).unwrap();
    let mutation = storage.mutation_view();
    let read = storage.read_view();

    let temp = tempfile::tempdir().unwrap();
    let journal_blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
    let items = Arc::new(mc_data::items::solaris_required_items());
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&journal_blocks),
        Arc::clone(&items),
    )
    .unwrap();
    assert!(pending.is_empty());
    let sessions = Arc::new(SessionRegistry::new());
    let failure = sessions.subscribe_world_chunk_journal_failure();
    sessions.install_world_chunk_journal(journal.clone());

    let edits = [
        mc_world::ResidentBlockEdit {
            pos: west,
            new_state: BlockStateId(0),
            preserve_light: true,
        },
        mc_world::ResidentBlockEdit {
            pos: east,
            new_state: BlockStateId(0),
            preserve_light: true,
        },
    ];
    let preconditions = [
        mc_world::ResidentBlockPrecondition {
            pos: west,
            expected_state: BlockStateId(1),
            expected_token: west_token,
        },
        mc_world::ResidentBlockPrecondition {
            pos: east,
            expected_state: BlockStateId(1),
            expected_token: east_token,
        },
    ];
    let outcome = commit_cross_region_scheduled_block_tick(
        &sessions,
        &mutation,
        20,
        ResidentBlockCommit {
            edits: &edits,
            preconditions: &preconditions,
            consumed_block_ticks: std::slice::from_ref(&due),
            consumed_fluid_ticks: &[],
            scheduled_fluid_ticks: &[],
            light_table: None,
            leaf_trigger_tick: None,
        },
    )
    .await
    .expect("known append failure closes its reservation")
    .expect("known append failure is a rejected resident transaction");

    assert!(outcome.applied.is_empty());
    assert_eq!(read.get_cached_block(west), Some(BlockStateId(1)));
    assert_eq!(read.get_cached_block(east), Some(BlockStateId(1)));
    assert_eq!(
        storage.scheduled_block_ticks(west_chunk).unwrap().unwrap(),
        std::slice::from_ref(&due)
    );
    assert_eq!(journal.watermark(), Some(1));
    assert!(!*failure.borrow());
    drop(sessions);
    drop(journal);

    let (reopened, pending) =
        super::world_journal::WorldChunkJournal::open(temp.path(), journal_blocks, items).unwrap();
    assert_eq!(pending.len(), 1);
    assert!(reopened.decode_pending(&pending).unwrap().is_empty());
    let next_decision_id = reopened.reserve_decision_ids(1).unwrap()[0];
    assert_eq!(next_decision_id, pending[0].id() + 1);
    reopened
        .record_reserved_snapshot_groups(21, vec![(next_decision_id, Vec::new())])
        .unwrap();
    assert_eq!(reopened.watermark(), Some(next_decision_id));
}

#[tokio::test]
async fn scheduled_button_regions_commit_without_the_global_world_writer() {
    let blocks = Arc::new(button_and_door_test_registry());
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    for chunk in [
        ChunkPos { x: -1, z: 0 },
        ChunkPos { x: 7, z: 0 },
        ChunkPos { x: 8, z: 0 },
        ChunkPos { x: 16, z: 0 },
    ] {
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
    }
    let west_button = mc_world::BlockPos {
        x: -16 + 1,
        y: 64,
        z: 1,
    };
    let boundary_button = mc_world::BlockPos {
        x: 8 * 16 - 1,
        y: 64,
        z: 1,
    };
    let lower_door = mc_world::BlockPos {
        x: 8 * 16,
        y: 64,
        z: 1,
    };
    let upper_door = mc_world::BlockPos {
        y: 65,
        ..lower_door
    };
    let east_button = mc_world::BlockPos {
        x: 16 * 16 + 1,
        y: 64,
        z: 1,
    };
    for position in [west_button, boundary_button, east_button] {
        storage.set_block_at(position, BlockStateId(2)).unwrap();
        storage
            .schedule_block_tick(ScheduledBlockTick::new(
                position,
                Identifier::parse("minecraft:stone_button").unwrap(),
                120,
                0,
            ))
            .unwrap();
    }
    storage.set_block_at(lower_door, BlockStateId(5)).unwrap();
    storage.set_block_at(upper_door, BlockStateId(6)).unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(121),
        name: "RegionalBarrierOrder".to_string(),
    };
    let (tx, _rx) = mpsc::channel(16);
    let loaded = HashSet::from([(-1, 0), (7, 0), (8, 0), (16, 0)]);
    let (session, _) = sessions.register(
        &profile,
        (-1, 0),
        16,
        loaded.clone(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    for chunk in loaded {
        let _ = sessions.mark_loaded(session, chunk);
    }
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);
    let storage = world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let (_simulation, owner) = simulation_channel();
    let resources = ChunkPipelineResources::with_limits(1, 2);
    let world_writer = world.lock().await;
    let report = tokio::time::timeout(
        Duration::from_secs(5),
        owner.run_scheduled_block_ticks_with_budget(
            &config,
            &sessions,
            SimulationWorldAccess {
                read: Some(&world_read),
                mutation: Some(&world_mutation),
                cpu: Some(&resources),
                light: config.block_light.as_ref(),
            },
            120,
            3,
        ),
    )
    .await
    .expect("mixed single-region and cross-region wave completes without the world writer");
    drop(world_writer);
    assert!(
        !*sessions.subscribe_world_chunk_journal_failure().borrow(),
        "mixed regional wave must not fail-stop its world journal"
    );
    assert_eq!(report.drained, 3);
    assert_eq!(report.applied, 5);
    let storage = world.lock().await;
    assert_eq!(storage.get_cached_block(west_button), Some(BlockStateId(1)));
    assert_eq!(
        storage.get_cached_block(boundary_button),
        Some(BlockStateId(1))
    );
    assert_eq!(storage.get_cached_block(lower_door), Some(BlockStateId(3)));
    assert_eq!(storage.get_cached_block(upper_door), Some(BlockStateId(4)));
    assert_eq!(storage.get_cached_block(east_button), Some(BlockStateId(1)));
}

#[tokio::test]
async fn resident_scheduled_button_tick_updates_without_world_writer_or_journal_wait() {
    let blocks = Arc::new(button_test_registry());
    let world = Arc::new(tokio::sync::Mutex::new(in_memory_button_world(Arc::clone(
        &blocks,
    ))));
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = world.lock().await;
        storage
            .set_block_at(pos, mc_world::BlockStateId(2))
            .expect("place powered button");
        storage
            .schedule_block_tick(mc_world::ScheduledBlockTick::new(
                pos,
                Identifier::parse("minecraft:stone_button").unwrap(),
                120,
                0,
            ))
            .expect("schedule button release");
    }
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "DurableButtonRelease");
    let storage = world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let (_simulation, owner) = simulation_channel();
    let world_writer = world.lock().await;
    let report = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        owner.run_scheduled_block_ticks_with_budget(
            &config,
            &sessions,
            SimulationWorldAccess {
                read: Some(&world_read),
                mutation: Some(&world_mutation),
                ..SimulationWorldAccess::default()
            },
            120,
            1,
        ),
    )
    .await
    .expect("resident button tick completion event");
    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);

    drop(world_writer);
    let storage = world.lock().await;
    assert_eq!(
        storage.get_cached_block(pos),
        Some(mc_world::BlockStateId(1))
    );
    assert!(
        storage
            .cached_chunk(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .scheduled_block_ticks()
            .is_empty()
    );
}

#[tokio::test]
async fn stale_resident_journal_commit_does_not_block_the_next_decision() {
    let blocks = Arc::new(button_test_registry());
    let world = Arc::new(tokio::sync::Mutex::new(in_memory_button_world(Arc::clone(
        &blocks,
    ))));
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = world.lock().await;
        storage
            .set_block_at(pos, mc_world::BlockStateId(2))
            .expect("place powered button");
    }
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);

    let storage = world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    let token = storage
        .block_mutation_token(pos)
        .expect("button mutation token");
    drop(storage);
    let edit = mc_world::ResidentBlockEdit {
        pos,
        new_state: mc_world::BlockStateId(1),
        preserve_light: true,
    };
    let stale = mc_world::ResidentBlockPrecondition {
        pos,
        expected_state: mc_world::BlockStateId(1),
        expected_token: token,
    };
    let current = mc_world::ResidentBlockPrecondition {
        pos,
        expected_state: mc_world::BlockStateId(2),
        expected_token: token,
    };

    let first = commit_resident_block_edits(
        &sessions,
        &world_read,
        &world_mutation,
        120,
        ResidentBlockCommit {
            edits: std::slice::from_ref(&edit),
            preconditions: std::slice::from_ref(&stale),
            consumed_block_ticks: &[],
            consumed_fluid_ticks: &[],
            scheduled_fluid_ticks: &[],
            light_table: None,
            leaf_trigger_tick: None,
        },
    )
    .await
    .expect("stale resident decision is a normal rejection")
    .expect("same-region stale decision has an outcome");
    assert!(first.applied.is_empty());

    let second = commit_resident_block_edits(
        &sessions,
        &world_read,
        &world_mutation,
        121,
        ResidentBlockCommit {
            edits: std::slice::from_ref(&edit),
            preconditions: std::slice::from_ref(&current),
            consumed_block_ticks: &[],
            consumed_fluid_ticks: &[],
            scheduled_fluid_ticks: &[],
            light_table: None,
            leaf_trigger_tick: None,
        },
    )
    .await
    .expect("decision after stale reservation must remain writable")
    .expect("same-region current decision has an outcome");
    assert_eq!(second.applied.len(), 1);

    let (reopened, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    let restored = reopened.decode_pending(&pending).unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(
        restored[0].get_block(1, pos.y, 1),
        Some(mc_world::BlockStateId(1))
    );
}

#[tokio::test]
async fn removed_log_pushes_leaf_distance_updates_through_scheduled_ticks() {
    let blocks = leaf_distance_test_registry();
    let mut storage = in_memory_button_world(Arc::clone(&blocks));
    let log = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let first_leaf = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let second_leaf = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    storage
        .set_block_at(log, mc_world::BlockStateId(1))
        .expect("place supporting log");
    storage
        .set_block_at(first_leaf, mc_world::BlockStateId(2))
        .expect("place first leaf");
    storage
        .set_block_at(second_leaf, mc_world::BlockStateId(2))
        .expect("place second leaf");
    let log_token = storage
        .block_mutation_token(log)
        .expect("supporting log has a mutation token");
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = SessionRegistry::new();
    let session = register_ticketed_button_session(&sessions, "LeafDistanceUpdates");
    let _ = sessions.mark_loaded(session, (0, 0));
    let (handle, mut owner) = simulation_channel();
    let session_handle = handle.for_session(session);
    let mut removal = Box::pin(session_handle.apply_block_edits(
        vec![BlockEdit {
            pos: log,
            new_state: mc_world::BlockStateId(0),
        }],
        vec![BlockEditPrecondition {
            pos: log,
            expected_state: mc_world::BlockStateId(1),
            expected_token: log_token,
        }],
    ));
    let waker = std::task::Waker::noop();
    let mut context = Context::from_waker(waker);
    assert!(matches!(
        std::future::Future::poll(removal.as_mut(), &mut context),
        Poll::Pending
    ));
    assert_eq!(
        owner
            .process_tick_with_world(&sessions, Some(&world), None, 1)
            .processed,
        1
    );
    assert_eq!(
        removal
            .await
            .expect("simulation owner applies log removal")
            .expect("matching log precondition")
            .applied
            .len(),
        1
    );
    {
        let mut storage = world.lock().await;
        let first_tick = storage
            .scheduled_block_ticks(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .expect("loaded chunk exposes leaf tick");
        assert_eq!(first_tick.len(), 1);
        assert_eq!(first_tick[0].pos, first_leaf);
        assert_eq!(first_tick[0].trigger_tick, 1);
    }

    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };

    let first = run_scheduled_block_ticks(&config, &sessions, 1).await;
    assert_eq!(first.drained, 1);
    assert_eq!(first.applied, 1);
    {
        let mut storage = world.lock().await;
        assert_eq!(
            storage.get_cached_block(first_leaf),
            Some(mc_world::BlockStateId(3)),
            "the first leaf should move from distance 1 to 2"
        );
        let second_tick = storage
            .scheduled_block_ticks(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .expect("loaded chunk exposes propagated leaf tick");
        assert!(
            second_tick
                .iter()
                .any(|tick| tick.pos == second_leaf && tick.trigger_tick == 2),
            "the changed first leaf must notify the second leaf"
        );
    }

    let second = run_scheduled_block_ticks(&config, &sessions, 2).await;
    assert_eq!(second.applied, 1);
    assert_eq!(
        world.lock().await.get_cached_block(second_leaf),
        Some(mc_world::BlockStateId(4)),
        "the second leaf should move from distance 1 to 3"
    );
}

async fn run_scheduled_block_ticks_for_range(
    config: &ServerConfig,
    sessions: &SessionRegistry,
    start: u64,
    end: u64,
) -> ScheduledBlockTickReport {
    let mut report = ScheduledBlockTickReport::default();
    for tick in start..=end {
        report = run_scheduled_block_ticks(config, sessions, tick).await;
    }
    report
}

#[tokio::test]
async fn stable_leaf_tick_is_checkpoint_only_without_world_journal_decision() {
    let blocks = leaf_distance_test_registry();
    let cpos = ChunkPos { x: 0, z: 0 };
    let leaf = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let log = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(leaf, BlockStateId(2)).unwrap();
    storage.set_block_at(log, BlockStateId(1)).unwrap();
    storage
        .schedule_block_tick(ScheduledBlockTick::new(
            leaf,
            Identifier::parse("minecraft:oak_leaves").unwrap(),
            20,
            0,
        ))
        .unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "StableLeafNoop");
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 0);
    let mut storage = world.lock().await;
    let ticks = storage
        .scheduled_block_ticks(cpos)
        .unwrap()
        .expect("loaded chunk exposes scheduled ticks");
    assert!(
        ticks.is_empty(),
        "the no-op tick is consumed in resident state"
    );
    drop(storage);
    let (_reopened, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(
        pending.is_empty(),
        "replaying a stable no-op leaf tick after a crash is harmless"
    );
}

#[tokio::test]
async fn scheduled_hopper_tick_pulls_one_item_into_hopper_before_ejecting_without_generating_neighbors()
 {
    struct CountingAirGenerator {
        calls: Arc<AtomicUsize>,
    }

    impl mc_world::ChunkGenerator for CountingAirGenerator {
        fn generate(&self, pos: ChunkPos) -> Chunk {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let mut chunk = Chunk::empty(
                pos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            );
            chunk.status = "minecraft:full".into();
            chunk.dirty = true;
            chunk
        }
    }

    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
        ])
        .unwrap(),
    );
    let generated_chunks = Arc::new(AtomicUsize::new(0));
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks)).with_generator(
        Arc::new(CountingAirGenerator {
            calls: Arc::clone(&generated_chunks),
        }),
    );
    storage
        .insert_generated_chunk(
            ChunkPos { x: 0, z: 0 },
            Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
        )
        .unwrap();
    let source_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let target_pos = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    storage.set_block_at(source_pos, BlockStateId(1)).unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(target_pos, BlockStateId(1)).unwrap();
    let mut source = mc_world::ChestBlockEntity::default();
    source.slots[0] = mc_world::FurnaceSlot {
        count: 2,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };
    storage.set_chest_block_entity(source_pos, source).unwrap();
    storage
        .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
        .unwrap();
    storage
        .set_chest_block_entity(target_pos, mc_world::ChestBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        items: Arc::new(ItemRegistry::from_report(&[ItemReport {
            id: Identifier::parse("minecraft:apple").unwrap(),
            protocol_id: 42,
        }])),
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let source_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(42),
        name: "HopperSourceViewer".to_string(),
    };
    let target_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(43),
        name: "HopperTargetViewer".to_string(),
    };
    let (source_tx, mut source_rx) = mpsc::channel(16);
    let (source_session_id, _) = sessions.register(
        &source_profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        source_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let (target_tx, mut target_rx) = mpsc::channel(16);
    let (target_session_id, _) = sessions.register(
        &target_profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        target_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let mut setup_dispatches = sessions.mark_loaded(source_session_id, (0, 0));
    setup_dispatches.extend(sessions.mark_loaded(target_session_id, (0, 0)));
    dispatch_and_clear_setup_packets(setup_dispatches, &mut [&mut source_rx, &mut target_rx]);
    assert_eq!(
        sessions.register_chest_viewer(source_session_id, source_pos),
        1
    );
    assert_eq!(
        sessions.register_chest_viewer(target_session_id, target_pos),
        1
    );
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);

    let storage = world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let (_simulation, owner) = simulation_channel();
    let world_writer = world.lock().await;
    let report = tokio::time::timeout(
        Duration::from_secs(5),
        owner.run_scheduled_block_ticks_with_budget(
            &config,
            &sessions,
            SimulationWorldAccess {
                read: Some(&world_read),
                mutation: Some(&world_mutation),
                ..SimulationWorldAccess::default()
            },
            20,
            1,
        ),
    )
    .await
    .expect("resident hopper transfer journal completion event");
    drop(world_writer);

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    {
        let mut storage = world.lock().await;
        assert_eq!(generated_chunks.load(Ordering::Relaxed), 0);
        assert_eq!(storage.cache_len(), 1);
        let source = storage
            .chest_block_entity(source_pos)
            .unwrap()
            .expect("source chest");
        let hopper = storage
            .hopper_block_entity(hopper_pos)
            .unwrap()
            .expect("hopper");
        let target = storage
            .chest_block_entity(target_pos)
            .unwrap()
            .expect("target chest");
        assert_eq!(source.slots[0].count, 1);
        assert_eq!(
            hopper.slots[0],
            mc_world::FurnaceSlot {
                count: 1,
                item_id: 42,
                damage: None,
                enchantments: Vec::new(),
            }
        );
        assert!(
            hopper.slots[1..]
                .iter()
                .all(mc_world::FurnaceSlot::is_empty)
        );
        assert!(target.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        let scheduled = storage
            .scheduled_block_ticks(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .expect("chunk scheduled ticks");
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].pos, hopper_pos);
        assert_eq!(scheduled[0].trigger_tick, 21);
        assert_eq!(
            scheduled[0].block,
            Identifier::parse("minecraft:hopper").unwrap()
        );
    }
    match source_rx
        .try_recv()
        .expect("source viewer receives chest slots")
    {
        OutboundCommand::ChestSlots {
            position,
            state_id,
            slots,
        } => {
            assert_eq!(position, source_pos);
            assert_eq!(state_id, 2);
            assert_eq!(slots[0], ItemStack::new(42, 1));
        }
        other => panic!("unexpected outbound command: {other:?}"),
    }
    assert!(target_rx.try_recv().is_err());

    let (reopened, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    let restored = reopened.decode_pending(&pending).unwrap();
    assert_eq!(restored.len(), 1);
    let source = restored[0]
        .chests
        .get(&source_pos)
        .expect("journaled source chest");
    let hopper = restored[0]
        .hoppers
        .get(&hopper_pos)
        .expect("journaled hopper");
    assert_eq!(source.slots[0].count, 1);
    assert_eq!(hopper.slots[0].item_id, 42);
    assert_eq!(hopper.slots[0].count, 1);
}

#[tokio::test]
async fn scheduled_hopper_ejection_schedules_comparator_tick_for_target_chest() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
            BlockReport {
                id: Identifier::parse("minecraft:comparator").unwrap(),
                properties: prop_schema(&[
                    ("facing", &["west"]),
                    ("mode", &["compare"]),
                    ("powered", &["false", "true"]),
                ]),
                states: vec![
                    state(
                        3,
                        true,
                        &[
                            ("facing", "west"),
                            ("mode", "compare"),
                            ("powered", "false"),
                        ],
                    ),
                    state(
                        4,
                        false,
                        &[("facing", "west"), ("mode", "compare"), ("powered", "true")],
                    ),
                ],
            },
        ])
        .unwrap(),
    );
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            ChunkPos { x: 0, z: 0 },
            Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
        )
        .unwrap();
    let hopper_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let target_pos = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let comparator_pos = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(target_pos, BlockStateId(1)).unwrap();
    storage
        .set_block_at(comparator_pos, BlockStateId(3))
        .unwrap();
    let mut hopper = mc_world::HopperBlockEntity::default();
    hopper.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };
    storage.set_hopper_block_entity(hopper_pos, hopper).unwrap();
    storage
        .set_chest_block_entity(target_pos, mc_world::ChestBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "HopperComparator");
    RESIDENT_HOPPER_TRANSFER_COMMIT_COUNT.with(|count| count.set(0));

    let first = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(first.drained, 1);
    assert_eq!(first.applied, 1);
    RESIDENT_HOPPER_TRANSFER_COMMIT_COUNT.with(|count| {
        assert_eq!(
            count.get(),
            1,
            "same-region hopper transfer uses resident CAS"
        )
    });
    {
        let mut storage = world.lock().await;
        let target = storage
            .chest_block_entity(target_pos)
            .unwrap()
            .expect("target chest");
        assert_eq!(
            target.slots[0],
            mc_world::FurnaceSlot {
                count: 1,
                item_id: 42,
                damage: None,
                enchantments: Vec::new(),
            }
        );
        assert_eq!(
            storage.get_cached_block(comparator_pos),
            Some(BlockStateId(3))
        );
        let scheduled = storage
            .scheduled_block_ticks(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .expect("chunk scheduled ticks");
        assert!(
            scheduled.iter().any(|tick| {
                tick.pos == comparator_pos
                    && tick.block == Identifier::parse("minecraft:comparator").unwrap()
                    && tick.trigger_tick == 22
            }),
            "hopper target mutation should schedule a delayed comparator refresh"
        );
    }

    let final_report = run_scheduled_block_ticks_for_range(&config, &sessions, 21, 22).await;

    assert_eq!(final_report.applied, 1);
    let storage = world.lock().await;
    assert_eq!(
        storage.get_cached_block(comparator_pos),
        Some(BlockStateId(4))
    );
}

#[tokio::test]
async fn scheduled_hopper_transfer_across_region_boundary_uses_atomic_resident_commit() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
        ])
        .unwrap(),
    );
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    for chunk_x in [7, 8] {
        let position = ChunkPos { x: chunk_x, z: 0 };
        storage
            .insert_generated_chunk(
                position,
                Chunk::empty(position, BlockStateId(0), biome.clone()),
            )
            .unwrap();
    }
    let hopper_pos = mc_world::BlockPos {
        x: 127,
        y: 64,
        z: 1,
    };
    let target_pos = mc_world::BlockPos {
        x: 128,
        y: 64,
        z: 1,
    };
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(target_pos, BlockStateId(1)).unwrap();
    let mut hopper = mc_world::HopperBlockEntity::default();
    hopper.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };
    storage.set_hopper_block_entity(hopper_pos, hopper).unwrap();
    storage
        .set_chest_block_entity(target_pos, mc_world::ChestBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(44),
        name: "CrossRegionHopper".to_string(),
    };
    let (tx, _rx) = mpsc::channel(16);
    let loaded = HashSet::from([(7, 0), (8, 0)]);
    let (session_id, _) = sessions.register(
        &profile,
        (7, 0),
        0,
        loaded.clone(),
        tx,
        PlayerPose::new(127.5, 64.0, 1.5),
    );
    for chunk in loaded {
        let _ = sessions.mark_loaded(session_id, chunk);
    }
    RESIDENT_HOPPER_TRANSFER_COMMIT_COUNT.with(|count| count.set(0));

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    RESIDENT_HOPPER_TRANSFER_COMMIT_COUNT.with(|count| {
        assert_eq!(
            count.get(),
            1,
            "cross-region hopper transfer must use one atomic resident commit"
        )
    });
    let mut storage = world.lock().await;
    let hopper = storage
        .hopper_block_entity(hopper_pos)
        .unwrap()
        .expect("hopper");
    let target = storage
        .chest_block_entity(target_pos)
        .unwrap()
        .expect("target chest");
    assert!(hopper.slots.iter().all(mc_world::FurnaceSlot::is_empty));
    assert_eq!(target.slots[0].count, 1);
    assert_eq!(target.slots[0].item_id, 42);
}

#[test]
fn comparator_container_signal_uses_vanilla_discrete_fullness_formula() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
        ])
        .unwrap(),
    );
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            ChunkPos { x: 0, z: 0 },
            Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
        )
        .unwrap();
    let chest_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    storage.set_block_at(chest_pos, BlockStateId(1)).unwrap();
    storage
        .set_chest_block_entity(chest_pos, mc_world::ChestBlockEntity::default())
        .unwrap();
    assert_eq!(
        container_redstone_signal_at(&blocks, &mut storage, chest_pos),
        0
    );

    let mut chest = mc_world::ChestBlockEntity::default();
    chest.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };
    storage.set_chest_block_entity(chest_pos, chest).unwrap();
    assert_eq!(
        container_redstone_signal_at(&blocks, &mut storage, chest_pos),
        1
    );

    let mut chest = mc_world::ChestBlockEntity::default();
    for slot in &mut chest.slots {
        *slot = mc_world::FurnaceSlot {
            count: HOPPER_TRANSFER_MAX_STACK,
            item_id: 42,
            damage: None,
            enchantments: Vec::new(),
        };
    }
    storage.set_chest_block_entity(chest_pos, chest).unwrap();
    assert_eq!(
        container_redstone_signal_at(&blocks, &mut storage, chest_pos),
        15
    );
}

#[tokio::test]
async fn placing_hopper_schedules_initial_transfer_tick() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:dirt"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["down"])]),
                states: vec![state(2, true, &[("facing", "down")])],
            },
        ])
        .unwrap(),
    );
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:hopper").unwrap(),
        protocol_id: 42,
    }]));
    let storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let world_read = storage.read_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let item_to_block = ItemToBlockTable::build(&items, &blocks);
    let mut state = InteractionState {
        world: Arc::clone(&world),
        world_read,
        blocks,
        block_light: None,
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        water: None,
        sessions: Arc::new(SessionRegistry::new()),
        simulation: simulation_channel().0,
        session_id: 1,
        workspace: LightWorkspace::new(),
        light_cache: LightCache::new(),
        compression: Compression::Disabled,
        selected_hotbar_slot: 0,
        inventory: PlayerInventory::empty(),
        carried_item: ItemStack::EMPTY,
        player_persistence: Arc::new(Mutex::new(PlayerPersistedState::new_default(
            PlayerPose::new(0.5, 64.0, 0.5),
        ))),
        inventory_state_id: 1,
        inventory_quickcraft: QuickCraftState::default(),
        items,
        item_facts: Arc::new(ItemFactsTable::default()),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        item_to_block,
        tags: Arc::new(TagsData::default()),
        recipes: Vec::new(),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        script_zones: None,
        next_container_id: FURNACE_CONTAINER_ID_MIN,
        active_container: None,
        pending_break: None,
        delayed_break: None,
        pending_use: None,
        pending_sign_edit: None,
        shield_use: None,
        last_entity_attack_tick: None,
    };
    *state.inventory.held_mut(0).unwrap() = ItemStack::new(42, 1);
    let session_id = register_interaction_player(&mut state, "HopperPlacementBuilder");
    let (simulation, mut simulation_owner) = simulation_channel();
    state.simulation = simulation.for_session(session_id);
    let owner_sessions = Arc::clone(&state.sessions);
    let owner_world = Arc::clone(&world);
    let (owner_stop_tx, mut owner_stop_rx) = tokio::sync::oneshot::channel();
    let owner_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = &mut owner_stop_rx => {
                    simulation_owner.shutdown();
                    break;
                }
                ready = simulation_owner.wait_for_command() => {
                    if !ready {
                        break;
                    }
                    simulation_owner.process_tick_with_world(
                        &owner_sessions,
                        Some(&owner_world),
                        None,
                        SIMULATION_COMMAND_BATCH_LIMIT,
                    );
                }
            }
        }
    });
    let cpos = ChunkPos { x: 0, z: 0 };
    let clicked_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let target_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    {
        let mut storage = world.lock().await;
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage.set_block_at(clicked_pos, BlockStateId(1)).unwrap();
    }
    let action = test_use_item_on(pack_block_pos(clicked_pos.x, clicked_pos.y, clicked_pos.z));
    let mut writer = tokio::io::sink();

    handle_block_item_placement(
        &mut state,
        &mut writer,
        None,
        GameMode::Survival,
        PlayerPose::new(1.5, 64.0, 1.5),
        clicked_pos,
        &action,
        (clicked_pos.x, clicked_pos.y, clicked_pos.z),
    )
    .await
    .unwrap();
    let _ = owner_stop_tx.send(());
    owner_task.await.unwrap();

    let mut storage = world.lock().await;
    assert_eq!(storage.get_cached_block(target_pos), Some(BlockStateId(2)));
    let scheduled = storage
        .scheduled_block_ticks(cpos)
        .unwrap()
        .expect("chunk scheduled ticks");
    assert_eq!(scheduled.len(), 1);
    assert_eq!(scheduled[0].pos, target_pos);
    assert_eq!(scheduled[0].trigger_tick, HOPPER_TICK_DELAY_TICKS);
    assert_eq!(
        scheduled[0].block,
        Identifier::parse("minecraft:hopper").unwrap()
    );
}

#[tokio::test]
async fn scheduled_block_pass_backfills_loaded_hopper_missing_initial_tick() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
        ])
        .unwrap(),
    );
    let cpos = ChunkPos { x: 0, z: 0 };
    let source_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let target_pos = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(source_pos, BlockStateId(1)).unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(target_pos, BlockStateId(1)).unwrap();
    let mut source = mc_world::ChestBlockEntity::default();
    source.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };
    storage.set_chest_block_entity(source_pos, source).unwrap();
    storage
        .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
        .unwrap();
    storage
        .set_chest_block_entity(target_pos, mc_world::ChestBlockEntity::default())
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "BackfillHopper");

    let first = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(first.drained, 0);
    assert_eq!(first.applied, 0);
    {
        let mut storage = world.lock().await;
        let source = storage
            .chest_block_entity(source_pos)
            .unwrap()
            .expect("source chest");
        let target = storage
            .chest_block_entity(target_pos)
            .unwrap()
            .expect("target chest");
        assert_eq!(source.slots[0].count, 1);
        assert!(target.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        let scheduled = storage
            .scheduled_block_ticks(cpos)
            .unwrap()
            .expect("chunk scheduled ticks");
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].pos, hopper_pos);
        assert_eq!(scheduled[0].trigger_tick, 21);
        assert_eq!(
            scheduled[0].block,
            Identifier::parse("minecraft:hopper").unwrap()
        );
    }

    let second = run_scheduled_block_ticks(&config, &sessions, 21).await;

    assert_eq!(second.drained, 1);
    assert_eq!(second.applied, 1);
    let mut storage = world.lock().await;
    let source = storage
        .chest_block_entity(source_pos)
        .unwrap()
        .expect("source chest");
    let target = storage
        .chest_block_entity(target_pos)
        .unwrap()
        .expect("target chest");
    assert!(source.slots.iter().all(mc_world::FurnaceSlot::is_empty));
    let hopper = storage
        .hopper_block_entity(hopper_pos)
        .unwrap()
        .expect("hopper");
    assert_eq!(
        hopper.slots[0],
        mc_world::FurnaceSlot {
            count: 1,
            item_id: 42,
            damage: None,
            enchantments: Vec::new(),
        }
    );
    assert!(target.slots.iter().all(mc_world::FurnaceSlot::is_empty));
}

#[tokio::test]
async fn scheduled_block_pass_does_not_duplicate_existing_hopper_tick() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["down"])]),
                states: vec![state(1, true, &[("facing", "down")])],
            },
        ])
        .unwrap(),
    );
    let cpos = ChunkPos { x: 0, z: 0 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(1)).unwrap();
    storage
        .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            40,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "ExistingHopperTick");

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 0);
    assert_eq!(report.applied, 0);
    let mut storage = world.lock().await;
    let scheduled = storage
        .scheduled_block_ticks(cpos)
        .unwrap()
        .expect("chunk scheduled ticks");
    assert_eq!(scheduled.len(), 1);
    assert_eq!(scheduled[0].pos, hopper_pos);
    assert_eq!(scheduled[0].trigger_tick, 40);
    assert_eq!(
        scheduled[0].block,
        Identifier::parse("minecraft:hopper").unwrap()
    );
}

#[tokio::test]
async fn scheduled_hopper_cooldown_tick_uses_resident_commit() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["down"])]),
                states: vec![state(1, true, &[("facing", "down")])],
            },
        ])
        .unwrap(),
    );
    let cpos = ChunkPos { x: 0, z: 0 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(1)).unwrap();
    storage
        .set_hopper_block_entity(
            hopper_pos,
            mc_world::HopperBlockEntity {
                transfer_cooldown: 8,
                ..mc_world::HopperBlockEntity::default()
            },
        )
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "ResidentHopperCooldown");
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);

    let storage = world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let (_simulation, owner) = simulation_channel();
    let world_writer = world.lock().await;
    let report = tokio::time::timeout(
        Duration::from_secs(5),
        owner.run_scheduled_block_ticks_with_budget(
            &config,
            &sessions,
            SimulationWorldAccess {
                read: Some(&world_read),
                mutation: Some(&world_mutation),
                ..SimulationWorldAccess::default()
            },
            20,
            1,
        ),
    )
    .await
    .expect("resident hopper journal completion event");
    drop(world_writer);

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 0);
    let mut storage = world.lock().await;
    assert_eq!(
        storage
            .hopper_block_entity(hopper_pos)
            .unwrap()
            .unwrap()
            .transfer_cooldown,
        7
    );
    let scheduled = storage
        .scheduled_block_ticks(cpos)
        .unwrap()
        .expect("hopper cooldown schedules its next tick");
    assert_eq!(scheduled.len(), 1);
    assert_eq!(scheduled[0].pos, hopper_pos);
    assert_eq!(scheduled[0].trigger_tick, 21);

    let (reopened, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    let restored = reopened.decode_pending(&pending).unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(
        restored[0]
            .hoppers
            .get(&hopper_pos)
            .expect("journaled hopper")
            .transfer_cooldown,
        7
    );
    assert_eq!(restored[0].scheduled_block_ticks().len(), 1);
    assert_eq!(restored[0].scheduled_block_ticks()[0].trigger_tick, 1);
}

#[tokio::test]
async fn scheduled_hopper_cooldowns_share_one_wal_decision() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["down"])]),
                states: vec![state(1, true, &[("facing", "down")])],
            },
        ])
        .unwrap(),
    );
    let cpos = ChunkPos { x: 0, z: 0 };
    let positions = [
        mc_world::BlockPos { x: 1, y: 64, z: 1 },
        mc_world::BlockPos { x: 2, y: 64, z: 1 },
    ];
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    for position in positions {
        storage.set_block_at(position, BlockStateId(1)).unwrap();
        storage
            .set_hopper_block_entity(
                position,
                mc_world::HopperBlockEntity {
                    transfer_cooldown: 8,
                    ..mc_world::HopperBlockEntity::default()
                },
            )
            .unwrap();
        storage
            .schedule_block_tick(mc_world::ScheduledBlockTick::new(
                position,
                Identifier::parse("minecraft:hopper").unwrap(),
                20,
                0,
            ))
            .unwrap();
    }
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "GroupedHopperCooldowns");
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 2);
    let (reopened, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert_eq!(pending.len(), 1, "one hopper pass uses one WAL decision");
    let restored = reopened.decode_pending(&pending).unwrap();
    assert_eq!(restored.len(), 1);
    assert!(positions.iter().all(|position| {
        restored[0]
            .hoppers
            .get(position)
            .is_some_and(|hopper| hopper.transfer_cooldown == 7)
    }));
}

#[test]
fn scheduled_hopper_container_dispatch_does_not_hold_world_writer() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
        ])
        .unwrap(),
    );
    let cpos = ChunkPos { x: 0, z: 0 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let target_pos = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(target_pos, BlockStateId(1)).unwrap();
    let mut hopper = mc_world::HopperBlockEntity::default();
    hopper.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };
    storage.set_hopper_block_entity(hopper_pos, hopper).unwrap();
    storage
        .set_chest_block_entity(target_pos, mc_world::ChestBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = Arc::new(SessionRegistry::new());
    register_loaded_button_session(&sessions, "HopperWorldLock");
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    sessions.install_server_container_dispatch_probe(reached_tx, resume_rx);

    let tick_sessions = Arc::clone(&sessions);
    let tick_thread = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(run_scheduled_block_ticks(&config, &tick_sessions, 20))
    });
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("scheduled hopper reaches container dispatch");
    let writer_available = world.try_lock().is_ok();
    resume_tx.send(()).expect("release container dispatch");
    let report = tick_thread.join().expect("scheduled hopper tick joins");

    assert!(
        writer_available,
        "container dispatch must run after releasing the world writer"
    );
    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
}

#[tokio::test]
async fn scheduled_hopper_tick_feeds_valid_input_into_furnace_below() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let iron_ore = Identifier::parse("minecraft:iron_ore").unwrap();
    let iron_ingot = Identifier::parse("minecraft:iron_ingot").unwrap();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["down"])]),
                states: vec![state(2, true, &[("facing", "down")])],
            },
            simple_block(3, "minecraft:furnace"),
        ])
        .unwrap(),
    );
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: iron_ore.clone(),
            protocol_id: 42,
        },
        ItemReport {
            id: iron_ingot.clone(),
            protocol_id: 43,
        },
    ]));
    let recipes = Arc::new(vec![Recipe {
        id: Identifier::parse("minecraft:test_iron_ore").unwrap(),
        kind: RecipeKind::Smelting(SmeltingRecipe {
            ingredient: Ingredient {
                alternatives: vec![IngredientAlternative::Item(iron_ore)],
            },
            cooking_time: 200,
            experience_milli: 0,
        }),
        result: RecipeResult {
            item: iron_ingot,
            count: 1,
        },
    }]);
    let cpos = ChunkPos { x: 0, z: 0 };
    let source_pos = mc_world::BlockPos { x: 1, y: 66, z: 1 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let furnace_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(source_pos, BlockStateId(1)).unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(furnace_pos, BlockStateId(3)).unwrap();
    let mut source = mc_world::ChestBlockEntity::default();
    source.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };
    storage.set_chest_block_entity(source_pos, source).unwrap();
    storage
        .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
        .unwrap();
    storage
        .set_furnace_block_entity(furnace_pos, mc_world::FurnaceBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        items,
        tags: Arc::new(TagsData::default()),
        recipes,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let source_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(44),
        name: "HopperFurnaceSourceViewer".to_string(),
    };
    let furnace_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(45),
        name: "HopperFurnaceViewer".to_string(),
    };
    let (source_tx, mut source_rx) = mpsc::channel(16);
    let (source_session_id, _) = sessions.register(
        &source_profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        source_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let (furnace_tx, mut furnace_rx) = mpsc::channel(16);
    let (furnace_session_id, _) = sessions.register(
        &furnace_profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        furnace_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let mut setup_dispatches = sessions.mark_loaded(source_session_id, (0, 0));
    setup_dispatches.extend(sessions.mark_loaded(furnace_session_id, (0, 0)));
    dispatch_and_clear_setup_packets(setup_dispatches, &mut [&mut source_rx, &mut furnace_rx]);
    assert_eq!(
        sessions.register_chest_viewer(source_session_id, source_pos),
        1
    );
    assert_eq!(
        sessions.register_furnace_viewer(furnace_session_id, furnace_pos),
        1
    );

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    let final_report = run_scheduled_block_ticks_for_range(&config, &sessions, 21, 28).await;
    assert_eq!(final_report.drained, 1);
    assert_eq!(final_report.applied, 1);
    {
        let mut storage = world.lock().await;
        let source = storage
            .chest_block_entity(source_pos)
            .unwrap()
            .expect("source chest");
        let hopper = storage
            .hopper_block_entity(hopper_pos)
            .unwrap()
            .expect("hopper");
        let furnace = storage
            .furnace_block_entity(furnace_pos)
            .unwrap()
            .expect("furnace");
        assert!(source.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        assert!(hopper.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        assert_eq!(
            furnace.slots[0],
            mc_world::FurnaceSlot {
                count: 1,
                item_id: 42,
                damage: None,
                enchantments: Vec::new(),
            }
        );
        assert!(furnace.slots[1].is_empty());
        assert!(furnace.slots[2].is_empty());
        let scheduled = storage
            .scheduled_block_ticks(cpos)
            .unwrap()
            .expect("chunk scheduled ticks");
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].pos, hopper_pos);
        assert_eq!(scheduled[0].trigger_tick, 29);
    }
    match source_rx
        .try_recv()
        .expect("source viewer receives chest slots")
    {
        OutboundCommand::ChestSlots {
            position,
            state_id,
            slots,
        } => {
            assert_eq!(position, source_pos);
            assert_eq!(state_id, 2);
            assert_eq!(slots[0], ItemStack::EMPTY);
        }
        other => panic!("unexpected outbound command: {other:?}"),
    }
    match furnace_rx
        .try_recv()
        .expect("furnace viewer receives furnace slots")
    {
        OutboundCommand::FurnaceSlots {
            position,
            state_id,
            slots,
        } => {
            assert_eq!(position, furnace_pos);
            assert_eq!(state_id, 2);
            assert_eq!(slots[0], ItemStack::new(42, 1));
            assert_eq!(slots[1], ItemStack::EMPTY);
            assert_eq!(slots[2], ItemStack::EMPTY);
        }
        other => panic!("unexpected outbound command: {other:?}"),
    }
}

#[tokio::test]
async fn scheduled_hopper_tick_feeds_side_fuel_into_furnace() {
    let oak_stairs = Identifier::parse("minecraft:oak_stairs").unwrap();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
            simple_block(3, "minecraft:furnace"),
        ])
        .unwrap(),
    );
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: oak_stairs,
        protocol_id: 44,
    }]));
    let recipes: Arc<Vec<mc_data::recipes::Recipe>> = Arc::new(Vec::new());
    let cpos = ChunkPos { x: 0, z: 0 };
    let source_pos = mc_world::BlockPos { x: 1, y: 66, z: 1 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let furnace_pos = mc_world::BlockPos { x: 2, y: 65, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(source_pos, BlockStateId(1)).unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(furnace_pos, BlockStateId(3)).unwrap();
    let mut source = mc_world::ChestBlockEntity::default();
    source.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 44,
        damage: None,
        enchantments: Vec::new(),
    };
    storage.set_chest_block_entity(source_pos, source).unwrap();
    storage
        .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
        .unwrap();
    storage
        .set_furnace_block_entity(furnace_pos, mc_world::FurnaceBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let tags = mc_data::tags::solaris_required_item_tags(&items);
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        items,
        tags: Arc::new(tags),
        recipes,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let source_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(46),
        name: "HopperFuelSourceViewer".to_string(),
    };
    let furnace_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(47),
        name: "HopperFuelViewer".to_string(),
    };
    let (source_tx, mut source_rx) = mpsc::channel(16);
    let (source_session_id, _) = sessions.register(
        &source_profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        source_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let (furnace_tx, mut furnace_rx) = mpsc::channel(16);
    let (furnace_session_id, _) = sessions.register(
        &furnace_profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        furnace_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let mut setup_dispatches = sessions.mark_loaded(source_session_id, (0, 0));
    setup_dispatches.extend(sessions.mark_loaded(furnace_session_id, (0, 0)));
    dispatch_and_clear_setup_packets(setup_dispatches, &mut [&mut source_rx, &mut furnace_rx]);
    assert_eq!(
        sessions.register_chest_viewer(source_session_id, source_pos),
        1
    );
    assert_eq!(
        sessions.register_furnace_viewer(furnace_session_id, furnace_pos),
        1
    );

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    let final_report = run_scheduled_block_ticks_for_range(&config, &sessions, 21, 28).await;
    assert_eq!(final_report.drained, 1);
    assert_eq!(final_report.applied, 1);
    {
        let mut storage = world.lock().await;
        let source = storage
            .chest_block_entity(source_pos)
            .unwrap()
            .expect("source chest");
        let hopper = storage
            .hopper_block_entity(hopper_pos)
            .unwrap()
            .expect("hopper");
        let furnace = storage
            .furnace_block_entity(furnace_pos)
            .unwrap()
            .expect("furnace");
        assert!(source.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        assert!(hopper.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        assert!(furnace.slots[0].is_empty());
        assert_eq!(
            furnace.slots[1],
            mc_world::FurnaceSlot {
                count: 1,
                item_id: 44,
                damage: None,
                enchantments: Vec::new(),
            }
        );
        assert!(furnace.slots[2].is_empty());
        let scheduled = storage
            .scheduled_block_ticks(cpos)
            .unwrap()
            .expect("chunk scheduled ticks");
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].pos, hopper_pos);
        assert_eq!(scheduled[0].trigger_tick, 29);
    }
    match source_rx
        .try_recv()
        .expect("source viewer receives chest slots")
    {
        OutboundCommand::ChestSlots {
            position,
            state_id,
            slots,
        } => {
            assert_eq!(position, source_pos);
            assert_eq!(state_id, 2);
            assert_eq!(slots[0], ItemStack::EMPTY);
        }
        other => panic!("unexpected outbound command: {other:?}"),
    }
    match furnace_rx
        .try_recv()
        .expect("furnace viewer receives furnace slots")
    {
        OutboundCommand::FurnaceSlots {
            position,
            state_id,
            slots,
        } => {
            assert_eq!(position, furnace_pos);
            assert_eq!(state_id, 2);
            assert_eq!(slots[0], ItemStack::EMPTY);
            assert_eq!(slots[1], ItemStack::new(44, 1));
            assert_eq!(slots[2], ItemStack::EMPTY);
        }
        other => panic!("unexpected outbound command: {other:?}"),
    }
}

#[tokio::test]
async fn scheduled_hopper_tick_extracts_furnace_output_into_chest() {
    let iron_ingot = Identifier::parse("minecraft:iron_ingot").unwrap();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
            simple_block(3, "minecraft:furnace"),
        ])
        .unwrap(),
    );
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: iron_ingot,
        protocol_id: 43,
    }]));
    let recipes: Arc<Vec<mc_data::recipes::Recipe>> = Arc::new(Vec::new());
    let cpos = ChunkPos { x: 0, z: 0 };
    let furnace_pos = mc_world::BlockPos { x: 1, y: 66, z: 1 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let target_pos = mc_world::BlockPos { x: 2, y: 65, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(furnace_pos, BlockStateId(3)).unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(target_pos, BlockStateId(1)).unwrap();
    let mut furnace = mc_world::FurnaceBlockEntity::default();
    furnace.slots[2] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 43,
        damage: None,
        enchantments: Vec::new(),
    };
    storage
        .set_furnace_block_entity(furnace_pos, furnace)
        .unwrap();
    storage
        .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
        .unwrap();
    storage
        .set_chest_block_entity(target_pos, mc_world::ChestBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        items,
        tags: Arc::new(TagsData::default()),
        recipes,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let furnace_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(48),
        name: "HopperOutputFurnaceViewer".to_string(),
    };
    let target_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(49),
        name: "HopperOutputTargetViewer".to_string(),
    };
    let (furnace_tx, mut furnace_rx) = mpsc::channel(16);
    let (furnace_session_id, _) = sessions.register(
        &furnace_profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        furnace_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let (target_tx, mut target_rx) = mpsc::channel(16);
    let (target_session_id, _) = sessions.register(
        &target_profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        target_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let mut setup_dispatches = sessions.mark_loaded(furnace_session_id, (0, 0));
    setup_dispatches.extend(sessions.mark_loaded(target_session_id, (0, 0)));
    dispatch_and_clear_setup_packets(setup_dispatches, &mut [&mut furnace_rx, &mut target_rx]);
    assert_eq!(
        sessions.register_furnace_viewer(furnace_session_id, furnace_pos),
        1
    );
    assert_eq!(
        sessions.register_chest_viewer(target_session_id, target_pos),
        1
    );

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    let final_report = run_scheduled_block_ticks_for_range(&config, &sessions, 21, 28).await;
    assert_eq!(final_report.drained, 1);
    assert_eq!(final_report.applied, 1);
    {
        let mut storage = world.lock().await;
        let furnace = storage
            .furnace_block_entity(furnace_pos)
            .unwrap()
            .expect("furnace");
        let hopper = storage
            .hopper_block_entity(hopper_pos)
            .unwrap()
            .expect("hopper");
        let target = storage
            .chest_block_entity(target_pos)
            .unwrap()
            .expect("target chest");
        assert!(furnace.slots[0].is_empty());
        assert!(furnace.slots[1].is_empty());
        assert!(furnace.slots[2].is_empty());
        assert!(hopper.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        assert_eq!(
            target.slots[0],
            mc_world::FurnaceSlot {
                count: 1,
                item_id: 43,
                damage: None,
                enchantments: Vec::new(),
            }
        );
        let scheduled = storage
            .scheduled_block_ticks(cpos)
            .unwrap()
            .expect("chunk scheduled ticks");
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].pos, hopper_pos);
        assert_eq!(scheduled[0].trigger_tick, 29);
    }
    match furnace_rx
        .try_recv()
        .expect("furnace viewer receives furnace slots")
    {
        OutboundCommand::FurnaceSlots {
            position,
            state_id,
            slots,
        } => {
            assert_eq!(position, furnace_pos);
            assert_eq!(state_id, 2);
            assert_eq!(slots[0], ItemStack::EMPTY);
            assert_eq!(slots[1], ItemStack::EMPTY);
            assert_eq!(slots[2], ItemStack::EMPTY);
        }
        other => panic!("unexpected outbound command: {other:?}"),
    }
    match target_rx
        .try_recv()
        .expect("target viewer receives chest slots")
    {
        OutboundCommand::ChestSlots {
            position,
            state_id,
            slots,
        } => {
            assert_eq!(position, target_pos);
            assert_eq!(state_id, 2);
            assert_eq!(slots[0], ItemStack::new(43, 1));
        }
        other => panic!("unexpected outbound command: {other:?}"),
    }
}

#[tokio::test]
async fn scheduled_hopper_tick_feeds_campfire_cooking_slot() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let porkchop = Identifier::parse("minecraft:porkchop").unwrap();
    let cooked_porkchop = Identifier::parse("minecraft:cooked_porkchop").unwrap();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["down"])]),
                states: vec![state(2, true, &[("facing", "down")])],
            },
            BlockReport {
                id: Identifier::parse("minecraft:campfire").unwrap(),
                properties: prop_schema(&[("lit", &["true"])]),
                states: vec![state(3, true, &[("lit", "true")])],
            },
        ])
        .unwrap(),
    );
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: porkchop.clone(),
            protocol_id: 42,
        },
        ItemReport {
            id: cooked_porkchop.clone(),
            protocol_id: 43,
        },
    ]));
    let recipes = Arc::new(vec![Recipe {
        id: Identifier::parse("minecraft:test_campfire").unwrap(),
        kind: RecipeKind::CampfireCooking(SmeltingRecipe {
            ingredient: Ingredient {
                alternatives: vec![IngredientAlternative::Item(porkchop.clone())],
            },
            cooking_time: 1,
            experience_milli: 0,
        }),
        result: RecipeResult {
            item: cooked_porkchop,
            count: 1,
        },
    }]);
    let cpos = ChunkPos { x: 0, z: 0 };
    let source_pos = mc_world::BlockPos { x: 1, y: 66, z: 1 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let campfire_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(source_pos, BlockStateId(1)).unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(campfire_pos, BlockStateId(3)).unwrap();
    let mut source = mc_world::ChestBlockEntity::default();
    source.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };
    storage.set_chest_block_entity(source_pos, source).unwrap();
    storage
        .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        items,
        tags: Arc::new(TagsData::default()),
        recipes,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(51),
        name: "HopperCampfireViewer".to_string(),
    };
    let (tx, mut rx) = mpsc::channel(16);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let _ = sessions.mark_loaded(session_id, (0, 0));
    assert_eq!(sessions.register_chest_viewer(session_id, source_pos), 1);

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    let final_report = run_scheduled_block_ticks_for_range(&config, &sessions, 21, 28).await;
    assert_eq!(final_report.drained, 1);
    assert_eq!(final_report.applied, 1);
    {
        let mut storage = world.lock().await;
        let source = storage
            .chest_block_entity(source_pos)
            .unwrap()
            .expect("source chest");
        let hopper = storage
            .hopper_block_entity(hopper_pos)
            .unwrap()
            .expect("hopper");
        assert!(source.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        assert!(hopper.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        let scheduled = storage
            .scheduled_block_ticks(cpos)
            .unwrap()
            .expect("chunk scheduled ticks");
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].pos, hopper_pos);
        assert_eq!(scheduled[0].trigger_tick, 29);
    }

    let mut saw_chest = false;
    let mut saw_campfire = false;
    for _ in 0..2 {
        match rx.try_recv().expect("hopper campfire update") {
            OutboundCommand::ChestSlots {
                position,
                state_id,
                slots,
            } => {
                assert_eq!(position, source_pos);
                assert_eq!(state_id, 2);
                assert_eq!(slots[0], ItemStack::EMPTY);
                saw_chest = true;
            }
            OutboundCommand::BlockEntityData {
                position,
                block_entity_type,
                nbt,
            } => {
                assert_eq!(position, campfire_pos);
                assert_eq!(block_entity_type, CAMPFIRE_BLOCK_ENTITY_TYPE_ID);
                assert_eq!(
                    nbt,
                    Tag::Compound(vec![(
                        "Items".into(),
                        Tag::List(ListTag {
                            element_type: mc_nbt::tag_type::COMPOUND,
                            elements: vec![Tag::Compound(vec![
                                ("Slot".into(), Tag::Int(0)),
                                ("id".into(), Tag::String(porkchop.as_str().to_string())),
                                ("count".into(), Tag::Int(1)),
                            ])],
                        }),
                    )])
                );
                saw_campfire = true;
            }
            other => panic!("unexpected outbound command: {other:?}"),
        }
    }
    assert!(saw_chest);
    assert!(saw_campfire);

    let (_simulation, owner) = simulation_channel();
    let cook_report = owner
        .run_campfire_cooking_ticks(&config, &sessions, None, None)
        .await;

    assert_eq!(cook_report.persisted, 1);
    assert_eq!(cook_report.completed, 1);
    assert_eq!(cook_report.dropped, 1);
    assert!(sessions.campfire_cooking_state(campfire_pos).is_empty());
}

#[test]
fn hopper_campfire_persistence_failure_does_not_publish_cooking_state() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let porkchop = Identifier::parse("minecraft:porkchop").unwrap();
    let cooked_porkchop = Identifier::parse("minecraft:cooked_porkchop").unwrap();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[simple_block(0, "minecraft:air")])
            .expect("air registry"),
    );
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: porkchop.clone(),
            protocol_id: 42,
        },
        ItemReport {
            id: cooked_porkchop.clone(),
            protocol_id: 43,
        },
    ]);
    let recipes = vec![Recipe {
        id: Identifier::parse("minecraft:test_campfire").unwrap(),
        kind: RecipeKind::CampfireCooking(SmeltingRecipe {
            ingredient: Ingredient {
                alternatives: vec![IngredientAlternative::Item(porkchop)],
            },
            cooking_time: 20,
            experience_milli: 0,
        }),
        result: RecipeResult {
            item: cooked_porkchop,
            count: 1,
        },
    }];
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let cpos = ChunkPos { x: 0, z: 0 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let sessions = SessionRegistry::new();
    let tags = TagsData::default();
    let context = HopperTransferContext {
        blocks: blocks.as_ref(),
        items: &items,
        tags: &tags,
        recipes: &recipes,
        sessions: &sessions,
    };
    let moving = FurnaceSlot {
        count: 1,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };

    assert!(
        insert_hopper_stack_into_campfire(&context, &mut storage, position, &moving).is_none(),
        "failed persistence must not tell the hopper to debit its source slot"
    );
    assert!(sessions.campfire_cooking_state(position).is_empty());
}

#[tokio::test]
async fn scheduled_hopper_tick_pulls_from_second_half_of_double_chest() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
        ])
        .unwrap(),
    );
    let cpos = ChunkPos { x: 0, z: 0 };
    let source_left_pos = mc_world::BlockPos { x: 1, y: 66, z: 1 };
    let source_right_pos = mc_world::BlockPos { x: 2, y: 66, z: 1 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let target_pos = mc_world::BlockPos { x: 2, y: 65, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage
        .set_block_at(source_left_pos, BlockStateId(1))
        .unwrap();
    storage
        .set_block_at(source_right_pos, BlockStateId(1))
        .unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(target_pos, BlockStateId(1)).unwrap();
    storage
        .set_chest_block_entity(source_left_pos, mc_world::ChestBlockEntity::default())
        .unwrap();
    let mut source_right = mc_world::ChestBlockEntity::default();
    source_right.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };
    storage
        .set_chest_block_entity(source_right_pos, source_right)
        .unwrap();
    storage
        .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
        .unwrap();
    storage
        .set_chest_block_entity(target_pos, mc_world::ChestBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "DoubleChestSourceHopper");

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    let mut storage = world.lock().await;
    let source_left = storage
        .chest_block_entity(source_left_pos)
        .unwrap()
        .expect("source left chest");
    let source_right = storage
        .chest_block_entity(source_right_pos)
        .unwrap()
        .expect("source right chest");
    let target = storage
        .chest_block_entity(target_pos)
        .unwrap()
        .expect("target chest");
    let hopper = storage
        .hopper_block_entity(hopper_pos)
        .unwrap()
        .expect("hopper");
    assert!(
        source_left
            .slots
            .iter()
            .all(mc_world::FurnaceSlot::is_empty)
    );
    assert!(
        source_right
            .slots
            .iter()
            .all(mc_world::FurnaceSlot::is_empty)
    );
    assert_eq!(
        hopper.slots[0],
        mc_world::FurnaceSlot {
            count: 1,
            item_id: 42,
            damage: None,
            enchantments: Vec::new(),
        }
    );
    assert!(target.slots.iter().all(mc_world::FurnaceSlot::is_empty));
}

#[tokio::test]
async fn scheduled_hopper_tick_inserts_into_second_half_of_double_chest() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
            BlockReport {
                id: Identifier::parse("minecraft:comparator").unwrap(),
                properties: prop_schema(&[
                    ("facing", &["west"]),
                    ("mode", &["compare"]),
                    ("powered", &["false", "true"]),
                ]),
                states: vec![
                    state(
                        3,
                        true,
                        &[
                            ("facing", "west"),
                            ("mode", "compare"),
                            ("powered", "false"),
                        ],
                    ),
                    state(
                        4,
                        false,
                        &[("facing", "west"), ("mode", "compare"), ("powered", "true")],
                    ),
                ],
            },
        ])
        .unwrap(),
    );
    let cpos = ChunkPos { x: 0, z: 0 };
    let source_pos = mc_world::BlockPos { x: 1, y: 66, z: 1 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let target_left_pos = mc_world::BlockPos { x: 2, y: 65, z: 1 };
    let target_right_pos = mc_world::BlockPos { x: 3, y: 65, z: 1 };
    let comparator_pos = mc_world::BlockPos { x: 4, y: 65, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(source_pos, BlockStateId(1)).unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage
        .set_block_at(target_left_pos, BlockStateId(1))
        .unwrap();
    storage
        .set_block_at(target_right_pos, BlockStateId(1))
        .unwrap();
    storage
        .set_block_at(comparator_pos, BlockStateId(3))
        .unwrap();
    let mut source = mc_world::ChestBlockEntity::default();
    source.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };
    storage.set_chest_block_entity(source_pos, source).unwrap();
    storage
        .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
        .unwrap();
    let mut target_left = mc_world::ChestBlockEntity::default();
    for slot in &mut target_left.slots {
        *slot = mc_world::FurnaceSlot {
            count: 64,
            item_id: 42,
            damage: None,
            enchantments: Vec::new(),
        };
    }
    storage
        .set_chest_block_entity(target_left_pos, target_left)
        .unwrap();
    storage
        .set_chest_block_entity(target_right_pos, mc_world::ChestBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(52),
        name: "DoubleChestTargetViewer".to_string(),
    };
    let (tx, mut rx) = mpsc::channel(16);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let _ = sessions.mark_loaded(session_id, (0, 0));
    assert_eq!(
        sessions.register_chest_viewer(session_id, target_left_pos),
        1
    );

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    let final_report = run_scheduled_block_ticks_for_range(&config, &sessions, 21, 28).await;
    assert_eq!(final_report.drained, 1);
    assert_eq!(final_report.applied, 1);
    let comparator_report = run_scheduled_block_ticks_for_range(&config, &sessions, 29, 30).await;
    assert_eq!(comparator_report.applied, 1);
    {
        let mut storage = world.lock().await;
        let source = storage
            .chest_block_entity(source_pos)
            .unwrap()
            .expect("source chest");
        let target_left = storage
            .chest_block_entity(target_left_pos)
            .unwrap()
            .expect("target left chest");
        let target_right = storage
            .chest_block_entity(target_right_pos)
            .unwrap()
            .expect("target right chest");
        assert!(source.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        assert!(target_left.slots.iter().all(|slot| {
            *slot
                == mc_world::FurnaceSlot {
                    count: 64,
                    item_id: 42,
                    damage: None,
                    enchantments: Vec::new(),
                }
        }));
        assert_eq!(
            target_right.slots[0],
            mc_world::FurnaceSlot {
                count: 1,
                item_id: 42,
                damage: None,
                enchantments: Vec::new(),
            }
        );
        assert_eq!(
            storage.get_cached_block(comparator_pos),
            Some(BlockStateId(4))
        );
    }

    match rx.try_recv().expect("double chest target receives slots") {
        OutboundCommand::ChestSlots {
            position,
            state_id,
            slots,
        } => {
            assert_eq!(position, target_left_pos);
            assert_eq!(state_id, 2);
            assert_eq!(slots.len(), 54);
            assert_eq!(slots[0], ItemStack::new(42, 64));
            assert_eq!(slots[27], ItemStack::new(42, 1));
        }
        other => panic!("unexpected outbound command: {other:?}"),
    }
}

#[tokio::test]
async fn scheduled_hopper_tick_does_not_extract_empty_furnace_output() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
            simple_block(3, "minecraft:furnace"),
        ])
        .unwrap(),
    );
    let cpos = ChunkPos { x: 0, z: 0 };
    let furnace_pos = mc_world::BlockPos { x: 1, y: 66, z: 1 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let target_pos = mc_world::BlockPos { x: 2, y: 65, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(furnace_pos, BlockStateId(3)).unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(target_pos, BlockStateId(1)).unwrap();
    storage
        .set_furnace_block_entity(furnace_pos, mc_world::FurnaceBlockEntity::default())
        .unwrap();
    storage
        .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
        .unwrap();
    storage
        .set_chest_block_entity(target_pos, mc_world::ChestBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        items: Arc::new(ItemRegistry::from_report(&[])),
        tags: Arc::new(TagsData::default()),
        recipes: Arc::new(Vec::new()),
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(50),
        name: "EmptyFurnaceOutputLoadedViewer".to_string(),
    };
    let (tx, _rx) = mpsc::channel(16);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let _ = sessions.mark_loaded(session_id, (0, 0));

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 0);
    {
        let mut storage = world.lock().await;
        let furnace = storage
            .furnace_block_entity(furnace_pos)
            .unwrap()
            .expect("furnace");
        let hopper = storage
            .hopper_block_entity(hopper_pos)
            .unwrap()
            .expect("hopper");
        let target = storage
            .chest_block_entity(target_pos)
            .unwrap()
            .expect("target chest");
        assert!(furnace.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        assert!(hopper.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        assert!(target.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        let scheduled = storage
            .scheduled_block_ticks(cpos)
            .unwrap()
            .expect("chunk scheduled ticks");
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].pos, hopper_pos);
        assert_eq!(scheduled[0].trigger_tick, 21);
    }
}

#[test]
fn scheduled_hopper_transfer_preserves_enchantments_when_merging_matching_stacks() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
        ])
        .unwrap(),
    );
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:iron_ingot").unwrap(),
        protocol_id: 43,
    }]);
    let cpos = ChunkPos { x: 0, z: 0 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let target_pos = mc_world::BlockPos { x: 2, y: 65, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(target_pos, BlockStateId(1)).unwrap();
    let efficiency = mc_data::ItemEnchantment {
        id: Identifier::parse("minecraft:efficiency").unwrap(),
        level: 1,
    };
    let mut hopper = mc_world::HopperBlockEntity {
        transfer_cooldown: 0,
        ..Default::default()
    };
    hopper.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 43,
        damage: None,
        enchantments: vec![efficiency.clone()],
    };
    let mut target = mc_world::ChestBlockEntity::default();
    target.slots[0] = mc_world::FurnaceSlot {
        count: 63,
        item_id: 43,
        damage: None,
        enchantments: vec![efficiency.clone()],
    };
    storage.set_hopper_block_entity(hopper_pos, hopper).unwrap();
    storage.set_chest_block_entity(target_pos, target).unwrap();

    let tags = TagsData::default();
    let recipes = Vec::new();
    let sessions = SessionRegistry::new();
    let context = HopperTransferContext {
        blocks: blocks.as_ref(),
        items: &items,
        tags: &tags,
        recipes: recipes.as_slice(),
        sessions: &sessions,
    };
    let result = scheduled_hopper_transfer(&context, &mut storage, hopper_pos, BlockStateId(2))
        .expect("transfer should apply");

    assert!(result.moved);
    assert_eq!(result.updates.len(), 1);
    let hopper = storage
        .hopper_block_entity(hopper_pos)
        .unwrap()
        .expect("hopper");
    let target = storage
        .chest_block_entity(target_pos)
        .unwrap()
        .expect("target chest");
    assert!(hopper.slots.iter().all(mc_world::FurnaceSlot::is_empty));
    assert_eq!(hopper.transfer_cooldown, HOPPER_TRANSFER_DELAY_TICKS as i32);
    assert_eq!(
        target.slots[0],
        mc_world::FurnaceSlot {
            count: 64,
            item_id: 43,
            damage: None,
            enchantments: vec![efficiency],
        }
    );
}

#[test]
fn scheduled_hopper_transfer_preserves_hopper_slot_when_target_has_no_room() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
        ])
        .unwrap(),
    );
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:iron_ingot").unwrap(),
        protocol_id: 43,
    }]);
    let cpos = ChunkPos { x: 0, z: 0 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let target_pos = mc_world::BlockPos { x: 2, y: 65, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(target_pos, BlockStateId(1)).unwrap();
    let mut hopper = mc_world::HopperBlockEntity {
        transfer_cooldown: 0,
        ..Default::default()
    };
    hopper.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 43,
        damage: None,
        enchantments: Vec::new(),
    };
    let mut target = mc_world::ChestBlockEntity::default();
    for slot in &mut target.slots {
        *slot = mc_world::FurnaceSlot {
            count: 64,
            item_id: 43,
            damage: None,
            enchantments: Vec::new(),
        };
    }
    storage.set_hopper_block_entity(hopper_pos, hopper).unwrap();
    storage.set_chest_block_entity(target_pos, target).unwrap();

    let tags = TagsData::default();
    let recipes = Vec::new();
    let sessions = SessionRegistry::new();
    let context = HopperTransferContext {
        blocks: blocks.as_ref(),
        items: &items,
        tags: &tags,
        recipes: recipes.as_slice(),
        sessions: &sessions,
    };
    let result = scheduled_hopper_transfer(&context, &mut storage, hopper_pos, BlockStateId(2))
        .expect("hopper tick runs");

    assert!(!result.moved);
    assert!(result.updates.is_empty());
    let hopper = storage
        .hopper_block_entity(hopper_pos)
        .unwrap()
        .expect("hopper");
    let target = storage
        .chest_block_entity(target_pos)
        .unwrap()
        .expect("target chest");
    assert_eq!(
        hopper.slots[0],
        mc_world::FurnaceSlot {
            count: 1,
            item_id: 43,
            damage: None,
            enchantments: Vec::new(),
        }
    );
    assert_eq!(hopper.transfer_cooldown, 0);
    assert!(target.slots.iter().all(|slot| {
        *slot
            == mc_world::FurnaceSlot {
                count: 64,
                item_id: 43,
                damage: None,
                enchantments: Vec::new(),
            }
    }));
}

#[tokio::test]
async fn scheduled_button_tick_ignores_ticketed_chunk_until_loaded() {
    let blocks = Arc::new(button_test_registry());
    let world = Arc::new(tokio::sync::Mutex::new(in_memory_button_world(Arc::clone(
        &blocks,
    ))));
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = world.lock().await;
        storage
            .set_block_at(pos, mc_world::BlockStateId(2))
            .expect("place powered button");
        storage
            .schedule_block_tick(mc_world::ScheduledBlockTick::new(
                pos,
                Identifier::parse("minecraft:stone_button").unwrap(),
                120,
                0,
            ))
            .expect("schedule button release");
    }
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let session_id = register_ticketed_button_session(&sessions, "TicketedButton");

    let before_loaded = run_scheduled_block_ticks(&config, &sessions, 120).await;
    assert_eq!(before_loaded.drained, 0);
    assert_eq!(before_loaded.applied, 0);
    {
        let mut storage = world.lock().await;
        assert_eq!(
            storage.get_cached_block(pos),
            Some(mc_world::BlockStateId(2))
        );
        let ticks = storage
            .scheduled_block_ticks(ChunkPos { x: 0, z: 0 })
            .expect("read scheduled block ticks")
            .expect("cached chunk should expose ticks");
        assert_eq!(ticks.len(), 1);
    }

    let _ = sessions.mark_loaded(session_id, (0, 0));
    SCHEDULED_BLOCK_PLANNING_WITHOUT_WRITER_COUNT.with(|count| count.set(0));
    let after_loaded = run_scheduled_block_ticks(&config, &sessions, 120).await;

    assert_eq!(after_loaded.drained, 1);
    assert_eq!(after_loaded.applied, 1);
    SCHEDULED_BLOCK_PLANNING_WITHOUT_WRITER_COUNT.with(|count| {
        assert_eq!(
            count.get(),
            1,
            "ordinary scheduled-block planning must not hold the world writer"
        );
    });
    let storage = world.lock().await;
    assert_eq!(
        storage.get_cached_block(pos),
        Some(mc_world::BlockStateId(1))
    );
}

#[tokio::test]
async fn stale_scheduled_button_plan_keeps_due_tick_after_aba() {
    let blocks = Arc::new(button_test_registry());
    let mut storage = in_memory_button_world(Arc::clone(&blocks));
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    storage
        .set_block_at(pos, mc_world::BlockStateId(2))
        .expect("place powered button");
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            pos,
            Identifier::parse("minecraft:stone_button").unwrap(),
            120,
            0,
        ))
        .expect("schedule button release");
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let chunk = ChunkPos { x: 0, z: 0 };
    let storage = world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let loaded = world_read.snapshot_chunks(&[chunk]);
    let due = due_scheduled_block_ticks(&loaded, &[chunk], 120, 1);
    assert_eq!(due.len(), 1);
    let planning_chunks = scheduled_block_planning_chunks(&due);
    let snapshot = world_read.snapshot_chunks(&planning_chunks);
    let plan = plan_scheduled_block_tick_edits(&config, &snapshot, &due, None)
        .expect("button-only batch uses snapshot planning");
    assert_eq!(
        plan.edits,
        vec![BlockEdit {
            pos,
            new_state: mc_world::BlockStateId(1),
        }]
    );

    let mut storage = world.lock().await;
    assert_eq!(
        storage
            .set_block_at(pos, mc_world::BlockStateId(1))
            .unwrap(),
        Some(mc_world::BlockStateId(2))
    );
    assert_eq!(
        storage
            .set_block_at(pos, mc_world::BlockStateId(2))
            .unwrap(),
        Some(mc_world::BlockStateId(1))
    );
    drop(storage);
    let (edits, preconditions) =
        resident_block_edit_inputs(&plan.edits, &plan.preconditions, None).unwrap();
    assert_eq!(
        world_mutation.apply_scheduled_block_tick_plan_conditionally(
            &mc_world::ResidentScheduledBlockTickPlan {
                consumed_ticks: &due,
                edits: &edits,
                preconditions: &preconditions,
                light_table: None,
                leaf_trigger_tick: Some(121),
            },
        ),
        mc_world::ResidentBlockEditBatchResult::Stale
    );
    let mut storage = world.lock().await;
    assert_eq!(
        storage.get_cached_block(pos),
        Some(mc_world::BlockStateId(2))
    );
    let restored = storage.scheduled_block_ticks(chunk).unwrap().unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].pos, pos);
    assert_eq!(restored[0].block.as_str(), "minecraft:stone_button");
}

#[test]
fn button_release_keeps_adjacent_door_powered_by_other_control() {
    let blocks = Arc::new(button_and_door_test_registry());
    let mut world = in_memory_button_world(Arc::clone(&blocks));
    let release_button_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let lower_door_pos = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let upper_door_pos = mc_world::BlockPos { x: 2, y: 65, z: 1 };
    let other_button_pos = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    world
        .set_block_at(release_button_pos, mc_world::BlockStateId(2))
        .expect("place releasing powered button");
    world
        .set_block_at(lower_door_pos, mc_world::BlockStateId(5))
        .expect("place powered lower iron door");
    world
        .set_block_at(upper_door_pos, mc_world::BlockStateId(6))
        .expect("place powered upper iron door");
    world
        .set_block_at(other_button_pos, mc_world::BlockStateId(2))
        .expect("place other powered button");

    let edits = scheduled_block_tick_edits(
        &blocks,
        &mut world,
        release_button_pos,
        mc_world::BlockStateId(2),
    )
    .expect("powered button release should edit the releasing button");

    assert_eq!(
        edits,
        vec![BlockEdit {
            pos: release_button_pos,
            new_state: mc_world::BlockStateId(1),
        }]
    );
}

#[tokio::test]
async fn button_press_powers_adjacent_iron_door_until_scheduled_release() {
    let blocks = Arc::new(button_and_door_test_registry());
    let world = Arc::new(tokio::sync::Mutex::new(in_memory_button_world(Arc::clone(
        &blocks,
    ))));
    let button_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let lower_door_pos = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let upper_door_pos = mc_world::BlockPos { x: 2, y: 65, z: 1 };
    {
        let mut storage = world.lock().await;
        storage
            .set_block_at(button_pos, mc_world::BlockStateId(1))
            .expect("place unpowered button");
        storage
            .set_block_at(lower_door_pos, mc_world::BlockStateId(3))
            .expect("place unpowered lower iron door");
        storage
            .set_block_at(upper_door_pos, mc_world::BlockStateId(4))
            .expect("place unpowered upper iron door");
        let plan = plan_toggle_block_interaction(
            &blocks,
            &*storage,
            button_pos,
            mc_world::BlockStateId(1),
            100,
        )
        .expect("button should press and power adjacent door");
        assert_eq!(
            plan.edits,
            vec![
                BlockEdit {
                    pos: button_pos,
                    new_state: mc_world::BlockStateId(2)
                },
                BlockEdit {
                    pos: lower_door_pos,
                    new_state: mc_world::BlockStateId(5)
                },
                BlockEdit {
                    pos: upper_door_pos,
                    new_state: mc_world::BlockStateId(6)
                },
            ]
        );
        let outcome = apply_block_edit_batch_with_scheduled_ticks_to_storage_conditionally(
            &mut storage,
            None,
            &plan.edits,
            &plan.preconditions,
            &plan.scheduled_block_ticks,
        )
        .expect("button plan should match its captured world version");
        assert_eq!(outcome.applied.len(), 3);
    }
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "ButtonDoor");

    let report = run_scheduled_block_ticks(&config, &sessions, 120).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 3);
    let storage = world.lock().await;
    assert_eq!(
        storage.get_cached_block(button_pos),
        Some(mc_world::BlockStateId(1))
    );
    assert_eq!(
        storage.get_cached_block(lower_door_pos),
        Some(mc_world::BlockStateId(3))
    );
    assert_eq!(
        storage.get_cached_block(upper_door_pos),
        Some(mc_world::BlockStateId(4))
    );
}

#[tokio::test]
async fn scheduled_button_release_keeps_piston_extended_when_head_is_protected() {
    let blocks = Arc::new(piston_test_registry());
    let world = Arc::new(tokio::sync::Mutex::new(in_memory_button_world(Arc::clone(
        &blocks,
    ))));
    let button = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let piston = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let arm = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    let destination = mc_world::BlockPos { x: 4, y: 64, z: 1 };
    {
        let mut storage = world.lock().await;
        for (pos, state_id) in [(button, 1), (piston, 5), (arm, 8)] {
            storage
                .set_block_at(pos, mc_world::BlockStateId(state_id))
                .expect("place scheduled piston test block");
        }
        let plan = plan_toggle_block_interaction(
            &blocks,
            &*storage,
            button,
            mc_world::BlockStateId(1),
            100,
        )
        .expect("button should extend adjacent piston");
        apply_block_edit_batch_with_scheduled_ticks_to_storage_conditionally(
            &mut storage,
            None,
            &plan.edits,
            &plan.preconditions,
            &plan.scheduled_block_ticks,
        )
        .expect("button extension plan remains current");
    }
    let zone = mc_script::ScriptAxisAlignedZone::try_new_with_protection(
        "piston-head",
        "minecraft:overworld",
        mc_script::ScriptPosition::try_new(3.0, 64.0, 1.0).unwrap(),
        mc_script::ScriptPosition::try_new(3.0, 64.0, 1.0).unwrap(),
        Some(
            mc_script::ScriptZoneProtection::try_actor_or_operator(
                "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            )
            .unwrap(),
        ),
    )
    .unwrap();
    let protection = Arc::new(crate::script::ZoneProtectionSnapshot::from_zones(vec![
        zone,
    ]));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "ProtectedPiston");

    let report =
        run_scheduled_block_ticks_with_protection(&config, &sessions, protection, 120).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    let storage = world.lock().await;
    assert_eq!(
        storage.get_cached_block(button),
        Some(mc_world::BlockStateId(1))
    );
    assert_eq!(
        storage.get_cached_block(piston),
        Some(mc_world::BlockStateId(6))
    );
    assert_eq!(
        storage.get_cached_block(arm),
        Some(mc_world::BlockStateId(7))
    );
    assert_eq!(
        storage.get_cached_block(destination),
        Some(mc_world::BlockStateId(8))
    );
}

fn button_test_registry() -> mc_world::BlockRegistry {
    mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:stone_button").unwrap(),
            properties: prop_schema(&[
                ("face", &["wall"]),
                ("facing", &["north"]),
                ("powered", &["false", "true"]),
            ]),
            states: vec![
                state(
                    1,
                    true,
                    &[("face", "wall"), ("facing", "north"), ("powered", "false")],
                ),
                state(
                    2,
                    false,
                    &[("face", "wall"), ("facing", "north"), ("powered", "true")],
                ),
            ],
        },
    ])
    .unwrap()
}

fn hand_toggle_test_registry() -> mc_world::BlockRegistry {
    mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        simple_door_block(1, "minecraft:oak_door"),
        simple_door_block(5, "minecraft:iron_door"),
        simple_door_block(9, "minecraft:copper_door"),
        simple_trapdoor_block(13, "minecraft:oak_trapdoor"),
        simple_trapdoor_block(15, "minecraft:iron_trapdoor"),
        simple_trapdoor_block(17, "minecraft:copper_trapdoor"),
        simple_fence_gate_block(19, "minecraft:oak_fence_gate"),
    ])
    .unwrap()
}

fn simple_fence_gate_block(first_id: u32, name: &str) -> BlockReport {
    BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: prop_schema(&[
            ("facing", &["north"]),
            ("in_wall", &["false"]),
            ("open", &["false", "true"]),
            ("powered", &["false"]),
        ]),
        states: vec![
            state(
                first_id,
                true,
                &[
                    ("facing", "north"),
                    ("in_wall", "false"),
                    ("open", "false"),
                    ("powered", "false"),
                ],
            ),
            state(
                first_id + 1,
                false,
                &[
                    ("facing", "north"),
                    ("in_wall", "false"),
                    ("open", "true"),
                    ("powered", "false"),
                ],
            ),
        ],
    }
}

fn simple_door_block(first_id: u32, name: &str) -> BlockReport {
    BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: prop_schema(&[
            ("facing", &["north"]),
            ("half", &["lower", "upper"]),
            ("open", &["false", "true"]),
            ("powered", &["false"]),
        ]),
        states: vec![
            state(
                first_id,
                true,
                &[
                    ("facing", "north"),
                    ("half", "lower"),
                    ("open", "false"),
                    ("powered", "false"),
                ],
            ),
            state(
                first_id + 1,
                false,
                &[
                    ("facing", "north"),
                    ("half", "upper"),
                    ("open", "false"),
                    ("powered", "false"),
                ],
            ),
            state(
                first_id + 2,
                false,
                &[
                    ("facing", "north"),
                    ("half", "lower"),
                    ("open", "true"),
                    ("powered", "false"),
                ],
            ),
            state(
                first_id + 3,
                false,
                &[
                    ("facing", "north"),
                    ("half", "upper"),
                    ("open", "true"),
                    ("powered", "false"),
                ],
            ),
        ],
    }
}

fn simple_trapdoor_block(first_id: u32, name: &str) -> BlockReport {
    BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: prop_schema(&[
            ("facing", &["north"]),
            ("half", &["bottom"]),
            ("open", &["false", "true"]),
            ("powered", &["false"]),
            ("waterlogged", &["false"]),
        ]),
        states: vec![
            state(
                first_id,
                true,
                &[
                    ("facing", "north"),
                    ("half", "bottom"),
                    ("open", "false"),
                    ("powered", "false"),
                    ("waterlogged", "false"),
                ],
            ),
            state(
                first_id + 1,
                false,
                &[
                    ("facing", "north"),
                    ("half", "bottom"),
                    ("open", "true"),
                    ("powered", "false"),
                    ("waterlogged", "false"),
                ],
            ),
        ],
    }
}

fn real_door_state(
    blocks: &mc_world::BlockRegistry,
    id: &str,
    half: &str,
    open: bool,
) -> mc_world::BlockStateId {
    let id = Identifier::parse(id).expect("static door identifier");
    blocks
        .by_name_and_props(
            &id,
            &[
                ("facing".to_string(), "north".to_string()),
                ("half".to_string(), half.to_string()),
                ("hinge".to_string(), "left".to_string()),
                (
                    "open".to_string(),
                    if open { "true" } else { "false" }.to_string(),
                ),
                ("powered".to_string(), "false".to_string()),
            ],
        )
        .expect("real door state")
}

fn in_memory_button_world(registry: Arc<mc_world::BlockRegistry>) -> mc_world::WorldStorage {
    let mut world = mc_world::WorldStorage::in_memory(registry);
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    world
}

fn register_loaded_button_session(sessions: &SessionRegistry, name: &str) {
    let id = register_ticketed_button_session(sessions, name);
    let _ = sessions.mark_loaded(id, (0, 0));
}

fn dispatch_and_clear_setup_packets(
    dispatches: Vec<VisibilityDispatch>,
    outbound: &mut [&mut mpsc::Receiver<OutboundCommand>],
) {
    dispatch_visibility_commands(dispatches);
    for receiver in outbound {
        while receiver.try_recv().is_ok() {}
    }
}

fn register_ticketed_button_session(
    sessions: &SessionRegistry,
    name: &str,
) -> super::session::SessionId {
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(name.bytes().map(u128::from).sum()),
        name: name.to_string(),
    };
    let (tx, _rx) = mpsc::channel(16);
    let (id, _) = sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    id
}

fn piston_test_registry() -> mc_world::BlockRegistry {
    mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:stone_button").unwrap(),
            properties: prop_schema(&[
                ("face", &["wall"]),
                ("facing", &["east"]),
                ("powered", &["false", "true"]),
            ]),
            states: vec![
                state(
                    1,
                    true,
                    &[("face", "wall"), ("facing", "east"), ("powered", "false")],
                ),
                state(
                    2,
                    false,
                    &[("face", "wall"), ("facing", "east"), ("powered", "true")],
                ),
            ],
        },
        BlockReport {
            id: Identifier::parse("minecraft:lever").unwrap(),
            properties: prop_schema(&[("powered", &["false", "true"])]),
            states: vec![
                state(3, true, &[("powered", "false")]),
                state(4, false, &[("powered", "true")]),
            ],
        },
        BlockReport {
            id: Identifier::parse("minecraft:piston").unwrap(),
            properties: prop_schema(&[("extended", &["false", "true"]), ("facing", &["east"])]),
            states: vec![
                state(5, true, &[("extended", "false"), ("facing", "east")]),
                state(6, false, &[("extended", "true"), ("facing", "east")]),
            ],
        },
        BlockReport {
            id: Identifier::parse("minecraft:piston_head").unwrap(),
            properties: prop_schema(&[
                ("facing", &["east"]),
                ("short", &["false"]),
                ("type", &["normal"]),
            ]),
            states: vec![state(
                7,
                true,
                &[("facing", "east"), ("short", "false"), ("type", "normal")],
            )],
        },
        simple_block(8, "minecraft:stone"),
    ])
    .unwrap()
}

fn button_and_door_test_registry() -> mc_world::BlockRegistry {
    mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:stone_button").unwrap(),
            properties: prop_schema(&[
                ("face", &["wall"]),
                ("facing", &["east"]),
                ("powered", &["false", "true"]),
            ]),
            states: vec![
                state(
                    1,
                    true,
                    &[("face", "wall"), ("facing", "east"), ("powered", "false")],
                ),
                state(
                    2,
                    false,
                    &[("face", "wall"), ("facing", "east"), ("powered", "true")],
                ),
            ],
        },
        BlockReport {
            id: Identifier::parse("minecraft:iron_door").unwrap(),
            properties: prop_schema(&[
                ("facing", &["north"]),
                ("half", &["lower", "upper"]),
                ("open", &["false", "true"]),
                ("powered", &["false", "true"]),
            ]),
            states: vec![
                state(
                    3,
                    true,
                    &[
                        ("facing", "north"),
                        ("half", "lower"),
                        ("open", "false"),
                        ("powered", "false"),
                    ],
                ),
                state(
                    4,
                    false,
                    &[
                        ("facing", "north"),
                        ("half", "upper"),
                        ("open", "false"),
                        ("powered", "false"),
                    ],
                ),
                state(
                    5,
                    false,
                    &[
                        ("facing", "north"),
                        ("half", "lower"),
                        ("open", "true"),
                        ("powered", "true"),
                    ],
                ),
                state(
                    6,
                    false,
                    &[
                        ("facing", "north"),
                        ("half", "upper"),
                        ("open", "true"),
                        ("powered", "true"),
                    ],
                ),
            ],
        },
        BlockReport {
            id: Identifier::parse("minecraft:lever").unwrap(),
            properties: prop_schema(&[("powered", &["false", "true"])]),
            states: vec![
                state(7, true, &[("powered", "false")]),
                state(8, false, &[("powered", "true")]),
            ],
        },
    ])
    .unwrap()
}

#[test]
fn door_half_state_builds_two_block_placement_states() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:oak_door").unwrap(),
            properties: prop_schema(&[
                ("facing", &["north", "south"]),
                ("half", &["lower", "upper"]),
                ("open", &["false"]),
                ("powered", &["false"]),
            ]),
            states: vec![
                state(
                    1,
                    true,
                    &[
                        ("facing", "north"),
                        ("half", "lower"),
                        ("open", "false"),
                        ("powered", "false"),
                    ],
                ),
                state(
                    2,
                    false,
                    &[
                        ("facing", "north"),
                        ("half", "upper"),
                        ("open", "false"),
                        ("powered", "false"),
                    ],
                ),
                state(
                    3,
                    false,
                    &[
                        ("facing", "south"),
                        ("half", "lower"),
                        ("open", "false"),
                        ("powered", "false"),
                    ],
                ),
                state(
                    4,
                    false,
                    &[
                        ("facing", "south"),
                        ("half", "upper"),
                        ("open", "false"),
                        ("powered", "false"),
                    ],
                ),
            ],
        },
    ])
    .unwrap();
    let default = blocks.by_id(mc_world::BlockStateId(1)).unwrap();

    assert_eq!(
        door_half_state(&blocks, default, "lower", "south"),
        Some(mc_world::BlockStateId(3))
    );
    assert_eq!(
        door_half_state(&blocks, default, "upper", "south"),
        Some(mc_world::BlockStateId(4))
    );
    assert_eq!(horizontal_facing_from_yaw(180.0), "north");
}

fn oriented_placement_test_registry() -> Arc<mc_world::BlockRegistry> {
    Arc::new(mc_world::BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap())
}

fn oriented_placement_state(
    blocks: &mc_world::BlockRegistry,
    block: &str,
    properties: &[(&str, &str)],
) -> mc_world::BlockStateId {
    blocks
        .by_name_and_props(
            &Identifier::parse(block).unwrap(),
            &properties
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|| panic!("missing canonical state {block} {properties:?}"))
}

fn torch_placement_test_registry() -> Arc<mc_world::BlockRegistry> {
    Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:stone"),
            BlockReport {
                id: Identifier::parse("minecraft:oak_fence").unwrap(),
                properties: prop_schema(&[
                    ("east", &["false"]),
                    ("north", &["false"]),
                    ("south", &["false"]),
                    ("west", &["false"]),
                    ("waterlogged", &["false"]),
                ]),
                states: vec![state(
                    2,
                    true,
                    &[
                        ("east", "false"),
                        ("north", "false"),
                        ("south", "false"),
                        ("west", "false"),
                        ("waterlogged", "false"),
                    ],
                )],
            },
            simple_block(3, "minecraft:torch"),
            BlockReport {
                id: Identifier::parse("minecraft:wall_torch").unwrap(),
                properties: prop_schema(&[("facing", &["north", "south", "west", "east"])]),
                states: vec![
                    state(4, true, &[("facing", "north")]),
                    state(5, false, &[("facing", "south")]),
                    state(6, false, &[("facing", "west")]),
                    state(7, false, &[("facing", "east")]),
                ],
            },
        ])
        .unwrap(),
    )
}

fn torch_support_pos(pos: mc_world::BlockPos, direction: Direction) -> mc_world::BlockPos {
    match direction {
        Direction::North => mc_world::BlockPos {
            z: pos.z + 1,
            ..pos
        },
        Direction::South => mc_world::BlockPos {
            z: pos.z - 1,
            ..pos
        },
        Direction::West => mc_world::BlockPos {
            x: pos.x + 1,
            ..pos
        },
        Direction::East => mc_world::BlockPos {
            x: pos.x - 1,
            ..pos
        },
        Direction::Up => mc_world::BlockPos {
            y: pos.y - 1,
            ..pos
        },
        Direction::Down => mc_world::BlockPos {
            y: pos.y + 1,
            ..pos
        },
    }
}

fn plan_torch_placement(
    blocks: Arc<mc_world::BlockRegistry>,
    support_state: mc_world::BlockStateId,
    direction: Direction,
) -> Option<super::block_placement::PlannedBlockPlacement> {
    let mut world = in_memory_button_world(Arc::clone(&blocks));
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    world
        .set_block_at(torch_support_pos(pos, direction), support_state)
        .expect("set torch support")
        .expect("replace torch support");
    let snapshot = world
        .read_view()
        .snapshot_chunks(&[ChunkPos { x: 0, z: 0 }]);

    plan_block_placement(
        &blocks,
        mc_world::BlockStateId(3),
        Some(&snapshot),
        pos,
        PlayerPose::new(0.5, 64.0, 0.5),
        direction,
        0.5,
        mc_world::BlockStateId(0),
    )
}

fn plan_oriented_test_placement(
    blocks: Arc<mc_world::BlockRegistry>,
    placed_state: mc_world::BlockStateId,
    yaw: f32,
    direction: mc_protocol::packets::play::Direction,
    target_relative_hit_y: f32,
) -> mc_world::BlockStateId {
    let world = in_memory_button_world(Arc::clone(&blocks));
    let snapshot = world
        .read_view()
        .snapshot_chunks(&[ChunkPos { x: 0, z: 0 }]);
    let mut pose = PlayerPose::new(0.5, 64.0, 0.5);
    pose.yaw = yaw;
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };

    plan_block_placement(
        &blocks,
        placed_state,
        Some(&snapshot),
        pos,
        pose,
        direction,
        target_relative_hit_y,
        mc_world::BlockStateId(0),
    )
    .expect("ordinary oriented block placement plans")
    .edits[0]
        .new_state
}

#[test]
fn stair_placement_uses_yaw_and_cursor_height_for_all_facings_and_halves() {
    let blocks = oriented_placement_test_registry();
    let held = blocks
        .block(&Identifier::parse("minecraft:oak_stairs").unwrap())
        .unwrap()
        .default;

    for (yaw, facing) in [
        (0.0, "south"),
        (90.0, "west"),
        (180.0, "north"),
        (270.0, "east"),
    ] {
        for (cursor_y, half) in [(0.25, "bottom"), (0.75, "top")] {
            assert_eq!(
                plan_oriented_test_placement(
                    Arc::clone(&blocks),
                    held,
                    yaw,
                    mc_protocol::packets::play::Direction::East,
                    cursor_y,
                ),
                oriented_placement_state(
                    &blocks,
                    "minecraft:oak_stairs",
                    &[
                        ("facing", facing),
                        ("half", half),
                        ("shape", "straight"),
                        ("waterlogged", "false"),
                    ],
                ),
            );
        }
    }
}

#[test]
fn slab_placement_uses_clicked_face_and_cursor_height() {
    let blocks = oriented_placement_test_registry();
    let held = blocks
        .block(&Identifier::parse("minecraft:oak_slab").unwrap())
        .unwrap()
        .default;

    for (direction, cursor_y, expected_type) in [
        (mc_protocol::packets::play::Direction::Up, 0.75, "bottom"),
        (mc_protocol::packets::play::Direction::Down, 0.25, "top"),
        (mc_protocol::packets::play::Direction::East, 0.25, "bottom"),
        (mc_protocol::packets::play::Direction::East, 0.5, "bottom"),
        (mc_protocol::packets::play::Direction::East, 0.75, "top"),
    ] {
        assert_eq!(
            plan_oriented_test_placement(Arc::clone(&blocks), held, 0.0, direction, cursor_y,),
            oriented_placement_state(
                &blocks,
                "minecraft:oak_slab",
                &[("type", expected_type), ("waterlogged", "false")],
            ),
        );
    }
}

#[test]
fn torch_placement_uses_the_clicked_horizontal_face_for_wall_facing() {
    let blocks = torch_placement_test_registry();

    for (direction, expected_state) in [
        (Direction::North, 4),
        (Direction::South, 5),
        (Direction::West, 6),
        (Direction::East, 7),
    ] {
        let plan = plan_torch_placement(Arc::clone(&blocks), mc_world::BlockStateId(1), direction)
            .expect("full sturdy support permits wall torch placement");
        assert_eq!(plan.edits.len(), 1);
        assert_eq!(
            plan.edits[0].new_state,
            mc_world::BlockStateId(expected_state)
        );
    }
}

#[test]
fn torch_placement_on_top_uses_the_standing_state() {
    let plan = plan_torch_placement(
        torch_placement_test_registry(),
        mc_world::BlockStateId(1),
        Direction::Up,
    )
    .expect("full sturdy top support permits standing torch placement");

    assert_eq!(plan.edits.len(), 1);
    assert_eq!(plan.edits[0].new_state, mc_world::BlockStateId(3));
}

#[test]
fn torch_placement_rejects_non_full_support_faces() {
    let blocks = torch_placement_test_registry();

    assert!(plan_torch_placement(blocks, mc_world::BlockStateId(2), Direction::East).is_none());
}

#[test]
fn torch_placement_rejects_downward_faces() {
    assert!(
        plan_torch_placement(
            torch_placement_test_registry(),
            mc_world::BlockStateId(1),
            Direction::Down,
        )
        .is_none()
    );
}

#[test]
fn placement_cursor_height_is_relative_to_the_placed_target() {
    assert_eq!(cursor_y_relative_to_target(64, 64, 0.5), 0.5);
    assert_eq!(cursor_y_relative_to_target(64, 65, 1.0), 0.0);
    assert_eq!(cursor_y_relative_to_target(64, 63, 0.0), 1.0);
}

#[test]
fn noncanonical_stair_family_fails_closed() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            BlockReport {
                id: Identifier::parse("minecraft:incomplete_stairs").unwrap(),
                properties: prop_schema(&[
                    ("facing", &["north", "south"]),
                    ("half", &["bottom", "top"]),
                ]),
                states: vec![state(1, true, &[("facing", "north"), ("half", "bottom")])],
            },
        ])
        .unwrap(),
    );
    let world = in_memory_button_world(Arc::clone(&blocks));
    let snapshot = world
        .read_view()
        .snapshot_chunks(&[ChunkPos { x: 0, z: 0 }]);

    assert!(
        plan_block_placement(
            &blocks,
            mc_world::BlockStateId(1),
            Some(&snapshot),
            mc_world::BlockPos { x: 1, y: 64, z: 1 },
            PlayerPose::new(0.5, 64.0, 0.5),
            mc_protocol::packets::play::Direction::East,
            0.75,
            mc_world::BlockStateId(0),
        )
        .is_none()
    );
}

#[test]
fn sign_placement_sets_wall_facing_and_floor_rotation() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:oak_sign").unwrap(),
            properties: prop_schema(&[("rotation", &["0", "4"])]),
            states: vec![
                state(1, true, &[("rotation", "0")]),
                state(2, false, &[("rotation", "4")]),
            ],
        },
        BlockReport {
            id: Identifier::parse("minecraft:oak_wall_sign").unwrap(),
            properties: prop_schema(&[("facing", &["north", "east"])]),
            states: vec![
                state(3, true, &[("facing", "north")]),
                state(4, false, &[("facing", "east")]),
            ],
        },
    ])
    .unwrap();
    let mut pose = PlayerPose::new(0.5, 64.0, 0.5);
    pose.yaw = 90.0;

    assert_eq!(
        sign_placement_state(
            &blocks,
            blocks.by_id(mc_world::BlockStateId(1)).unwrap(),
            pose,
            Direction::Up,
        ),
        Some(mc_world::BlockStateId(2))
    );
    assert_eq!(
        sign_placement_state(
            &blocks,
            blocks.by_id(mc_world::BlockStateId(3)).unwrap(),
            pose,
            Direction::East,
        ),
        Some(mc_world::BlockStateId(4))
    );

    assert_eq!(
        placed_sign_edit(
            &blocks,
            &BlockEditBatchOutcome {
                applied: vec![AppliedBlockEdit {
                    pos: mc_world::BlockPos { x: 1, y: 2, z: 3 },
                    previous: mc_world::BlockStateId(0),
                    new_state: mc_world::BlockStateId(2),
                }],
                resulting_tokens: HashMap::from([(
                    mc_world::BlockPos { x: 1, y: 2, z: 3 },
                    mc_world::BlockMutationToken {
                        chunk_instance_id: 7,
                        version: 11,
                    },
                )]),
                ..BlockEditBatchOutcome::default()
            },
        ),
        Some(PendingSignEdit {
            position: mc_world::BlockPos { x: 1, y: 2, z: 3 },
            state: mc_world::BlockStateId(2),
            token: mc_world::BlockMutationToken {
                chunk_instance_id: 7,
                version: 11,
            },
            is_front_text: true,
        })
    );
}

#[test]
fn sign_update_nbt_matches_vanilla_plain_text_shape() {
    let tag = sign_block_entity_update_nbt(
        &[
            "Hello".to_string(),
            "World".to_string(),
            String::new(),
            "!".to_string(),
        ],
        true,
    );

    assert_eq!(
        tag,
        Tag::Compound(vec![
            (
                "front_text".into(),
                Tag::Compound(vec![
                    (
                        "messages".into(),
                        Tag::List(ListTag {
                            element_type: mc_nbt::tag_type::STRING,
                            elements: vec![
                                Tag::String("Hello".into()),
                                Tag::String("World".into()),
                                Tag::String(String::new()),
                                Tag::String("!".into()),
                            ],
                        }),
                    ),
                    ("color".into(), Tag::String("black".into())),
                    ("has_glowing_text".into(), Tag::Byte(0)),
                ]),
            ),
            (
                "back_text".into(),
                Tag::Compound(vec![
                    (
                        "messages".into(),
                        Tag::List(ListTag {
                            element_type: mc_nbt::tag_type::STRING,
                            elements: vec![
                                Tag::String(String::new()),
                                Tag::String(String::new()),
                                Tag::String(String::new()),
                                Tag::String(String::new()),
                            ],
                        }),
                    ),
                    ("color".into(), Tag::String("black".into())),
                    ("has_glowing_text".into(), Tag::Byte(0)),
                ]),
            ),
            ("is_waxed".into(), Tag::Byte(0)),
        ])
    );
}

#[test]
fn campfire_update_nbt_contains_visible_cooking_items_only() {
    let porkchop = Identifier::parse("minecraft:porkchop").unwrap();
    let cooked_porkchop = Identifier::parse("minecraft:cooked_porkchop").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: porkchop.clone(),
            protocol_id: 10,
        },
        ItemReport {
            id: cooked_porkchop,
            protocol_id: 11,
        },
    ]);
    let mut cooking = CampfireCookingState::default();
    assert!(cooking.insert(ItemStack::new(10, 1), ItemStack::new(11, 1), 2));

    assert_eq!(
        campfire_block_entity_update_nbt(&items, &cooking),
        Some(Tag::Compound(vec![(
            "Items".into(),
            Tag::List(ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: vec![Tag::Compound(vec![
                    ("Slot".into(), Tag::Int(0)),
                    ("id".into(), Tag::String(porkchop.as_str().to_string())),
                    ("count".into(), Tag::Int(1)),
                ])],
            }),
        )]))
    );

    assert!(!cooking.tick().changed);
    let tick = cooking.tick();
    assert!(tick.changed);
    assert_eq!(tick.completed, vec![ItemStack::new(11, 1)]);
    assert_eq!(
        campfire_block_entity_update_nbt(&items, &cooking),
        Some(Tag::Compound(vec![(
            "Items".into(),
            Tag::List(ListTag::empty()),
        )]))
    );
}

fn bed_occupancy_test_registry() -> mc_world::BlockRegistry {
    mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:red_bed").unwrap(),
            properties: prop_schema(&[
                ("facing", &["north"]),
                ("occupied", &["false", "true"]),
                ("part", &["head", "foot"]),
            ]),
            states: vec![
                state(
                    1,
                    true,
                    &[("facing", "north"), ("occupied", "false"), ("part", "foot")],
                ),
                state(
                    2,
                    false,
                    &[("facing", "north"), ("occupied", "false"), ("part", "head")],
                ),
                state(
                    3,
                    false,
                    &[("facing", "north"), ("occupied", "true"), ("part", "foot")],
                ),
                state(
                    4,
                    false,
                    &[("facing", "north"), ("occupied", "true"), ("part", "head")],
                ),
            ],
        },
    ])
    .unwrap()
}

#[test]
fn bed_respawn_pose_uses_block_above_bed() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:red_bed").unwrap(),
            properties: prop_schema(&[("facing", &["north"])]),
            states: vec![state(1, true, &[("facing", "north")])],
        },
    ])
    .unwrap();
    let pose = bed_respawn_pose(
        mc_world::BlockPos { x: 3, y: 64, z: -2 },
        blocks.by_id(mc_world::BlockStateId(1)).unwrap(),
    );

    assert_eq!((pose.x, pose.y, pose.z, pose.yaw), (3.5, 65.0, -1.5, 180.0));
}

#[test]
fn bed_halves_share_the_head_position_as_reservation_key() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:red_bed").unwrap(),
            properties: prop_schema(&[
                ("facing", &["north"]),
                ("occupied", &["false"]),
                ("part", &["head", "foot"]),
            ]),
            states: vec![
                state(
                    1,
                    true,
                    &[("facing", "north"), ("occupied", "false"), ("part", "foot")],
                ),
                state(
                    2,
                    false,
                    &[("facing", "north"), ("occupied", "false"), ("part", "head")],
                ),
            ],
        },
    ])
    .unwrap();
    let foot = mc_world::BlockPos { x: 3, y: 64, z: 2 };
    let head = mc_world::BlockPos { x: 3, y: 64, z: 1 };

    assert_eq!(
        canonical_bed_position(foot, blocks.by_id(mc_world::BlockStateId(1)).unwrap()),
        head
    );
    assert_eq!(
        canonical_bed_position(head, blocks.by_id(mc_world::BlockStateId(2)).unwrap()),
        head
    );
}

#[tokio::test]
async fn bed_head_and_foot_clicks_share_the_exact_head_centered_respawn_pose() {
    let blocks = Arc::new(bed_occupancy_test_registry());
    let state = interaction_state_for_blocks(blocks);
    insert_fluid_test_chunk(&state).await;
    let head = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    let foot = mc_world::BlockPos { x: 3, y: 64, z: 2 };
    {
        let mut world = state.world.lock().await;
        world.set_block_at(head, BlockStateId(2)).unwrap();
        world.set_block_at(foot, BlockStateId(1)).unwrap();
    }

    let (head_pose, head_canonical) =
        plan_loaded_bed_interaction(&state.world_read, &state.blocks, head)
            .expect("head click plans a respawn");
    let (foot_pose, foot_canonical) =
        plan_loaded_bed_interaction(&state.world_read, &state.blocks, foot)
            .expect("foot click plans a respawn");

    assert_eq!(head_canonical, head);
    assert_eq!(foot_canonical, head);
    assert_eq!(
        (head_pose.x, head_pose.y, head_pose.z, head_pose.yaw),
        (3.5, 65.0, 1.5, 180.0)
    );
    assert_eq!(
        (foot_pose.x, foot_pose.y, foot_pose.z, foot_pose.yaw),
        (head_pose.x, head_pose.y, head_pose.z, head_pose.yaw)
    );
}

#[tokio::test]
async fn bed_planning_rejects_a_mismatched_second_half() {
    let blocks = Arc::new(bed_occupancy_test_registry());
    let state = interaction_state_for_blocks(blocks);
    insert_fluid_test_chunk(&state).await;
    let head = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    let foot = mc_world::BlockPos { x: 3, y: 64, z: 2 };
    {
        let mut world = state.world.lock().await;
        world.set_block_at(head, BlockStateId(2)).unwrap();
        world
            .set_block_at(foot, BlockStateId(2))
            .expect("place mismatched head in foot position");
    }

    assert!(
        plan_loaded_bed_interaction(&state.world_read, &state.blocks, head).is_none(),
        "loaded interaction must reject a second head in the foot position"
    );
    assert!(
        plan_bed_occupied_edits(&state.world_read, &state.blocks, head, true).is_none(),
        "occupancy planning must reject mismatched halves"
    );
}

#[tokio::test]
async fn bed_occupancy_stale_token_rejects_the_whole_edit_set() {
    let blocks = Arc::new(bed_occupancy_test_registry());
    let state = interaction_state_for_blocks(blocks);
    insert_fluid_test_chunk(&state).await;
    let head = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    let foot = mc_world::BlockPos { x: 3, y: 64, z: 2 };
    {
        let mut world = state.world.lock().await;
        world.set_block_at(head, BlockStateId(2)).unwrap();
        world.set_block_at(foot, BlockStateId(1)).unwrap();
    }
    let (edits, preconditions) =
        plan_bed_occupied_edits(&state.world_read, &state.blocks, head, true)
            .expect("matching bed halves plan occupancy edits");

    let outcome = {
        let mut world = state.world.lock().await;
        world.set_block_at(foot, BlockStateId(3)).unwrap();
        world.set_block_at(foot, BlockStateId(1)).unwrap();
        apply_block_edit_batch_to_storage_conditionally(&mut world, None, &edits, &preconditions)
    };

    assert!(
        outcome.is_none(),
        "ABA mutation must stale the occupancy plan"
    );
    let world = state.world.lock().await;
    assert_eq!(world.get_cached_block(head), Some(BlockStateId(2)));
    assert_eq!(world.get_cached_block(foot), Some(BlockStateId(1)));
}

#[tokio::test]
async fn bed_mixed_occupancy_aba_on_unchanged_half_rejects_the_edit() {
    let blocks = Arc::new(bed_occupancy_test_registry());
    let state = interaction_state_for_blocks(blocks);
    insert_fluid_test_chunk(&state).await;
    let head = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    let foot = mc_world::BlockPos { x: 3, y: 64, z: 2 };
    {
        let mut world = state.world.lock().await;
        world.set_block_at(head, BlockStateId(2)).unwrap();
        world.set_block_at(foot, BlockStateId(3)).unwrap();
    }
    let (edits, preconditions) =
        plan_bed_occupied_edits(&state.world_read, &state.blocks, head, true)
            .expect("mixed occupancy still plans the stale half update");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].pos, head);
    assert_eq!(preconditions.len(), 2);

    let outcome = {
        let mut world = state.world.lock().await;
        world.set_block_at(foot, BlockStateId(1)).unwrap();
        world.set_block_at(foot, BlockStateId(3)).unwrap();
        apply_block_edit_batch_to_storage_conditionally(&mut world, None, &edits, &preconditions)
    };

    assert!(
        outcome.is_none(),
        "ABA on the unchanged half must stale the whole bed plan"
    );
    let world = state.world.lock().await;
    assert_eq!(world.get_cached_block(head), Some(BlockStateId(2)));
    assert_eq!(world.get_cached_block(foot), Some(BlockStateId(3)));
}

#[tokio::test]
async fn bed_interaction_planning_does_not_wait_for_world_writer() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            BlockReport {
                id: Identifier::parse("minecraft:red_bed").unwrap(),
                properties: prop_schema(&[("facing", &["north"])]),
                states: vec![state(1, true, &[("facing", "north")])],
            },
        ])
        .unwrap(),
    );
    let state = interaction_state_for_blocks(blocks);
    insert_fluid_test_chunk(&state).await;
    let position = mc_world::BlockPos { x: 3, y: 64, z: 2 };
    state
        .world
        .lock()
        .await
        .set_block_at(position, mc_world::BlockStateId(1))
        .expect("place bed");

    let world = Arc::clone(&state.world);
    let world_writer = world.lock().await;
    let (pose, canonical_bed) =
        plan_loaded_bed_interaction(&state.world_read, &state.blocks, position)
            .expect("published bed should remain interactive");

    assert_eq!((pose.x, pose.y, pose.z, pose.yaw), (3.5, 65.0, 2.5, 180.0));
    assert_eq!(canonical_bed, position);
    drop(world_writer);
}

#[tokio::test]
async fn bed_obstruction_uses_suffocation_above_both_halves() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            BlockReport {
                id: Identifier::parse("minecraft:red_bed").unwrap(),
                properties: prop_schema(&[("facing", &["north"]), ("part", &["head", "foot"])]),
                states: vec![
                    state(1, true, &[("facing", "north"), ("part", "foot")]),
                    state(2, false, &[("facing", "north"), ("part", "head")]),
                ],
            },
            simple_block(3, "minecraft:stone"),
            simple_block(4, "minecraft:barrier"),
            simple_block(5, "minecraft:oak_slab"),
        ])
        .unwrap(),
    );
    let mut interaction = interaction_state_for_blocks(Arc::clone(&blocks));
    interaction.block_light = Some(Arc::new(
        mc_data::block_light::BlockLightTable::from_arrays_with_suffocating(
            "test",
            vec![0; 6],
            vec![0, 0, 0, 15, 0, 15],
            vec![true, true, true, false, true, false],
            vec![false, false, false, true, true, false],
        ),
    ));
    insert_fluid_test_chunk(&interaction).await;
    let head = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    let foot = mc_world::BlockPos { x: 3, y: 64, z: 2 };
    {
        let mut world = interaction.world.lock().await;
        world.set_block_at(head, BlockStateId(2)).unwrap();
        world.set_block_at(foot, BlockStateId(1)).unwrap();
        world
            .set_block_at(mc_world::BlockPos { x: 3, y: 65, z: 2 }, BlockStateId(3))
            .unwrap();
    }

    assert!(bed_sleep_is_obstructed(
        &interaction.world_read,
        &interaction.blocks,
        interaction.block_light.as_deref(),
        head
    ));

    interaction
        .world
        .lock()
        .await
        .set_block_at(mc_world::BlockPos { x: 3, y: 65, z: 2 }, BlockStateId(5))
        .unwrap();
    assert!(!bed_sleep_is_obstructed(
        &interaction.world_read,
        &interaction.blocks,
        interaction.block_light.as_deref(),
        head
    ));

    interaction
        .world
        .lock()
        .await
        .set_block_at(mc_world::BlockPos { x: 3, y: 65, z: 1 }, BlockStateId(4))
        .unwrap();
    assert!(bed_sleep_is_obstructed(
        &interaction.world_read,
        &interaction.blocks,
        interaction.block_light.as_deref(),
        head
    ));
}

#[test]
fn nearby_monster_blocks_survival_sleep_but_not_creative_sleep() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            BlockReport {
                id: Identifier::parse("minecraft:red_bed").unwrap(),
                properties: prop_schema(&[("facing", &["north"])]),
                states: vec![state(1, true, &[("facing", "north")])],
            },
        ])
        .unwrap(),
    );
    let state = interaction_state_for_blocks(blocks);
    let bed = mc_world::BlockPos { x: 0, y: 64, z: 0 };
    state.sessions.spawn_command_entity(
        &simulation::SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_owned(),
        Vec3::new(7.5, 68.5, 7.5),
    );

    let hostile_nearby = state.sessions.has_rest_preventing_hostile_near_bed(bed);
    assert!(bed_sleep_is_blocked_by_monster(
        GameMode::Survival,
        hostile_nearby
    ));
    assert!(!bed_sleep_is_blocked_by_monster(
        GameMode::Creative,
        hostile_nearby
    ));
}

#[tokio::test]
async fn safe_bed_wake_uses_flat_floor_and_skips_unsafe_cells_in_vanilla_order() {
    let reports = vec![
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:red_bed").unwrap(),
            properties: prop_schema(&[("facing", &["north"])]),
            states: vec![state(1, true, &[("facing", "north")])],
        },
        simple_block(2, "minecraft:stone"),
        fluid_block(3, "minecraft:water", 0),
        simple_block(4, "minecraft:campfire"),
    ];
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&reports).unwrap());
    let mut state = interaction_state_for_blocks(blocks);
    state.block_facts = Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
        &reports,
    ));
    insert_fluid_test_chunk(&state).await;
    let bed = mc_world::BlockPos { x: 0, y: 64, z: 0 };
    {
        let mut world = state.world.lock().await;
        for x in -3..=3 {
            for z in -3..=3 {
                world
                    .set_block_at(mc_world::BlockPos { x, y: 63, z }, BlockStateId(2))
                    .unwrap();
            }
        }
        world.set_block_at(bed, BlockStateId(1)).unwrap();
        world
            .set_block_at(mc_world::BlockPos { x: 1, y: 64, z: 0 }, BlockStateId(3))
            .unwrap();
        world
            .set_block_at(mc_world::BlockPos { x: 1, y: 64, z: 1 }, BlockStateId(4))
            .unwrap();
        world
            .set_block_at(mc_world::BlockPos { x: 1, y: 64, z: 2 }, BlockStateId(2))
            .unwrap();
    }
    let sleeping_pose = PlayerPose::new(0.5, 65.0, 0.5);

    let wake = safe_bed_wake_pose(
        &state.world_read,
        &state.blocks,
        &state.block_facts,
        bed,
        sleeping_pose,
    );

    assert_eq!((wake.x, wake.y, wake.z), (0.5, 64.0, 2.5));
    assert!(wake.flags.on_ground);
}

#[tokio::test]
async fn safe_bed_wake_uses_above_head_after_surrounding_candidates_are_blocked() {
    let reports = vec![
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:red_bed").unwrap(),
            properties: prop_schema(&[("facing", &["north"])]),
            states: vec![state(1, true, &[("facing", "north")])],
        },
        simple_block(2, "minecraft:stone"),
    ];
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&reports).unwrap());
    let mut state = interaction_state_for_blocks(blocks);
    state.block_facts = Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
        &reports,
    ));
    insert_fluid_test_chunk(&state).await;
    let bed = mc_world::BlockPos { x: 0, y: 64, z: 0 };
    let foot = mc_world::BlockPos { x: 0, y: 64, z: 1 };
    {
        let mut world = state.world.lock().await;
        for x in -3..=3 {
            for z in -3..=3 {
                world
                    .set_block_at(mc_world::BlockPos { x, y: 63, z }, BlockStateId(2))
                    .unwrap();
                world
                    .set_block_at(mc_world::BlockPos { x, y: 64, z }, BlockStateId(2))
                    .unwrap();
            }
        }
        world.set_block_at(bed, BlockStateId(1)).unwrap();
        world.set_block_at(foot, BlockStateId(1)).unwrap();
    }

    let wake = safe_bed_wake_pose(
        &state.world_read,
        &state.blocks,
        &state.block_facts,
        bed,
        PlayerPose::new(0.5, 65.0, 0.5),
    );

    assert_eq!((wake.x, wake.y, wake.z), (0.5, 65.0, 0.5));
    assert!(wake.flags.on_ground);
}

#[test]
fn sleep_skip_targets_the_next_morning() {
    assert_eq!(next_morning_time(12_542), 24_000);
    assert_eq!(next_morning_time(47_999), 48_000);
    assert_eq!(next_morning_time(u64::MAX), u64::MAX);
}

#[test]
fn common_container_paper_cuts_resolve_to_existing_menus() {
    assert_eq!(
        super::containers::furnace_menu_title_for_block_id("minecraft:furnace"),
        Some("Furnace")
    );
    assert_eq!(
        super::containers::furnace_menu_title_for_block_id("minecraft:smoker"),
        Some("Smoker")
    );
    assert_eq!(
        super::containers::furnace_menu_title_for_block_id("minecraft:blast_furnace"),
        Some("Blast Furnace")
    );
    assert_eq!(FurnaceKind::Furnace.menu_type(), FURNACE_MENU_TYPE_ID);
    assert_eq!(FurnaceKind::Smoker.menu_type(), SMOKER_MENU_TYPE_ID);
    assert_eq!(
        FurnaceKind::BlastFurnace.menu_type(),
        BLAST_FURNACE_MENU_TYPE_ID
    );
    assert_eq!(ENCHANTING_MENU_TYPE_ID, 13);
    assert_eq!(STONECUTTER_MENU_TYPE_ID, 24);
    assert_eq!(
        super::containers::unsupported_survival_station_for_block_id("minecraft:enchanting_table"),
        None
    );
    assert_eq!(
        super::containers::unsupported_survival_station_for_block_id("minecraft:stonecutter"),
        None
    );

    let expected_unsupported_stations = [
        ("minecraft:brewing_stand", "brewing stand"),
        ("minecraft:anvil", "anvil"),
        ("minecraft:chipped_anvil", "anvil"),
        ("minecraft:damaged_anvil", "anvil"),
        ("minecraft:smithing_table", "smithing table"),
        ("minecraft:grindstone", "grindstone"),
        ("minecraft:loom", "loom"),
        ("minecraft:cartography_table", "cartography table"),
        ("minecraft:composter", "composter"),
        ("minecraft:cauldron", "cauldron"),
        ("minecraft:water_cauldron", "cauldron"),
        ("minecraft:lava_cauldron", "cauldron"),
        ("minecraft:powder_snow_cauldron", "cauldron"),
        ("minecraft:lectern", "lectern"),
        ("minecraft:fletching_table", "fletching table"),
        ("minecraft:beacon", "beacon"),
        ("minecraft:crafter", "crafter"),
    ];
    for (block_id, station) in expected_unsupported_stations {
        assert_eq!(
            super::containers::unsupported_survival_station_for_block_id(block_id),
            Some(station),
            "{block_id} must be covered by the M87 safe-rejection policy"
        );
    }
}

#[test]
fn cauldron_variants_are_safe_interaction_targets() {
    let cauldron_variants = [
        "minecraft:cauldron",
        "minecraft:water_cauldron",
        "minecraft:lava_cauldron",
        "minecraft:powder_snow_cauldron",
    ];

    for block_id in cauldron_variants {
        assert_eq!(
            super::containers::unsupported_survival_station_for_block_id(block_id),
            Some("cauldron"),
            "{block_id} must not fall through into adjacent block placement"
        );
    }
}

fn interaction_state_for_items(items: Arc<ItemRegistry>) -> InteractionState {
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&[]).unwrap());
    let storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let world_read = storage.read_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let item_to_block = ItemToBlockTable::build(&items, &blocks);
    let tags = Arc::new(mc_data::tags::solaris_required_item_tags(&items));
    InteractionState {
        world,
        world_read,
        blocks,
        block_light: None,
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        water: None,
        sessions: Arc::new(SessionRegistry::new()),
        simulation: simulation_channel().0,
        session_id: 1,
        workspace: LightWorkspace::new(),
        light_cache: LightCache::new(),
        compression: Compression::Disabled,
        selected_hotbar_slot: 0,
        inventory: PlayerInventory::empty(),
        carried_item: ItemStack::EMPTY,
        player_persistence: Arc::new(Mutex::new(PlayerPersistedState::new_default(
            PlayerPose::new(0.5, 64.0, 0.5),
        ))),
        inventory_state_id: 1,
        inventory_quickcraft: QuickCraftState::default(),
        items,
        item_facts: Arc::new(ItemFactsTable::default()),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        item_to_block,
        tags,
        recipes: Vec::new(),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        script_zones: None,
        next_container_id: FURNACE_CONTAINER_ID_MIN,
        active_container: None,
        pending_break: None,
        delayed_break: None,
        pending_use: None,
        pending_sign_edit: None,
        shield_use: None,
        last_entity_attack_tick: None,
    }
}

#[tokio::test]
async fn disconnected_cursor_is_preserved_when_simulation_owner_is_unavailable() {
    let item = Identifier::parse("minecraft:dirt").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: item,
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(items);
    for slot in 9..=44 {
        state.inventory.slots[slot] = ItemStack::new(10, 64);
    }
    state.carried_item = ItemStack::new(10, 2);
    state.entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("DisconnectCursor"),
        name: "DisconnectCursor".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    saved.carried_item = state.carried_item.clone();
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, owner) = simulation_channel();
    drop(owner);
    state.simulation = simulation.for_session(session_id);

    settle_disconnected_cursor(&mut state, &saved).await;

    assert_eq!(state.carried_item, ItemStack::new(10, 2));
    assert_eq!(saved.lock().unwrap().carried_item, ItemStack::new(10, 2));
    assert!(state.sessions.persisted_entity_records().is_empty());
}

#[tokio::test]
async fn disconnected_cursor_settlement_commits_inventory_and_drop_in_one_owner_turn() {
    let item = Identifier::parse("minecraft:dirt").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: item,
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(items);
    for slot in 9..=44 {
        state.inventory.slots[slot] = ItemStack::new(10, 64);
    }
    state.carried_item = ItemStack::new(10, 2);
    state.entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());

    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("AtomicDisconnectCursor"),
        name: "AtomicDisconnectCursor".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    saved.carried_item = state.carried_item.clone();
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    let simulation_probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);

    settle_disconnected_cursor(&mut state, &saved).await;
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(simulation_probe.snapshot().processed, 1);
    assert!(state.carried_item.is_empty());
    assert!(saved.lock().unwrap().carried_item.is_empty());
    let drops = state.sessions.persisted_entity_records();
    assert_eq!(drops.len(), 1);
    assert_eq!(
        drops[0].snapshot.item_stack,
        Some(EntityItemStack::new(10, 2))
    );
    assert_eq!(drops[0].snapshot.position, Vec3::new(4.5, 66.0, 6.5));
}

#[tokio::test]
async fn crafting_table_close_commits_returned_inputs_and_all_drops_in_one_owner_turn() {
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let cobblestone = Identifier::parse("minecraft:cobblestone").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: dirt,
            protocol_id: 10,
        },
        ItemReport {
            id: cobblestone,
            protocol_id: 11,
        },
    ]));
    let mut state = interaction_state_for_items(items);
    for slot in 9..=44 {
        state.inventory.slots[slot] = ItemStack::new(10, 64);
    }
    state.inventory.slots[9] = ItemStack::new(10, 63);
    state.entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    let mut window = CraftingTableWindow::new(7);
    window.input[0] = ItemStack::new(10, 2);
    window.input[1] = ItemStack::new(11, 3);
    let crafting_table_input = crafting_table_input_projection(&window.input);
    state.active_container = Some(ActiveContainer::CraftingTable(Box::new(window)));

    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("AtomicCraftingClose"),
        name: "AtomicCraftingClose".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    saved.crafting_table_input = crafting_table_input;
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    let simulation_probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);

    store_active_container(&mut state, pose).await.unwrap();
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(simulation_probe.snapshot().processed, 1);
    assert!(state.active_container.is_none());
    assert_eq!(
        saved.lock().unwrap().inventory.slots[9],
        ItemStack::new(10, 64)
    );
    let mut drops = state
        .sessions
        .persisted_entity_records()
        .into_iter()
        .map(|record| record.snapshot.item_stack.unwrap())
        .collect::<Vec<_>>();
    drops.sort_by_key(|stack| stack.item_id);
    assert_eq!(
        drops,
        vec![EntityItemStack::new(10, 1), EntityItemStack::new(11, 3)]
    );
}

#[tokio::test]
async fn inventory_crafting_close_commits_returned_inputs_and_drop_in_one_owner_turn() {
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:dirt").unwrap(),
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(items);
    for slot in 9..=44 {
        state.inventory.slots[slot] = ItemStack::new(10, 64);
    }
    state.inventory.slots[9] = ItemStack::new(10, 63);
    state.inventory.slots[1] = ItemStack::new(10, 2);
    state.entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());

    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("AtomicInventoryCraftingClose"),
        name: "AtomicInventoryCraftingClose".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    let simulation_probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);

    store_inventory_crafting_inputs(&mut state, pose)
        .await
        .unwrap();
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(simulation_probe.snapshot().processed, 1);
    let saved = saved.lock().unwrap();
    assert!(saved.inventory.slots[1].is_empty());
    assert_eq!(saved.inventory.slots[9], ItemStack::new(10, 64));
    drop(saved);
    let drops = state.sessions.persisted_entity_records();
    assert_eq!(drops.len(), 1);
    assert_eq!(
        drops[0].snapshot.item_stack,
        Some(EntityItemStack::new(10, 1))
    );
}

#[tokio::test]
async fn login_recovers_persisted_container_inputs_and_cursor_in_one_owner_turn() {
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:dirt").unwrap(),
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(items);
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let mut recovered = PlayerPersistedState::new_default(pose);
    recovered.carried_item = ItemStack::new(10, 1);
    let mut crafting_table_input = std::array::from_fn(|_| ItemStack::EMPTY);
    crafting_table_input[0] = ItemStack::new(10, 2);
    recovered.crafting_table_input = Some(Box::new(crafting_table_input));
    let mut enchanting_table_input = std::array::from_fn(|_| ItemStack::EMPTY);
    enchanting_table_input[0] = ItemStack::new(10, 3);
    recovered.enchanting_table_input = Some(Box::new(enchanting_table_input));
    state.inventory = recovered.inventory.clone();
    state.carried_item = recovered.carried_item.clone();

    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("RecoveredContainerLogin"),
        name: "RecoveredContainerLogin".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let saved = Arc::new(Mutex::new(recovered.clone()));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    let simulation_probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);

    settle_recovered_player_inventory(&mut state, &recovered)
        .await
        .unwrap();
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(simulation_probe.snapshot().processed, 1);
    assert_eq!(
        state
            .inventory
            .slots
            .iter()
            .map(|stack| stack.count.max(0))
            .sum::<i32>(),
        6
    );
    assert!(state.carried_item.is_empty());
    let saved = saved.lock().unwrap();
    assert_eq!(saved.inventory.slots, state.inventory.slots);
    assert!(saved.carried_item.is_empty());
    assert!(saved.crafting_table_input.is_none());
    assert!(saved.enchanting_table_input.is_none());
    assert!(state.sessions.persisted_entity_records().is_empty());
}

#[tokio::test]
async fn cancelled_connection_cleanup_retains_owner_state_for_checkpoint() {
    let sessions = Arc::new(SessionRegistry::new());
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("CancelledOwnerState"),
        name: "CancelledOwnerState".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = sessions.register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut state = PlayerPersistedState::new_default(pose);
    state.carried_item = ItemStack::new(10, 3);
    sessions.register_player_persistence(session_id, Arc::new(Mutex::new(state)));

    let observed = sessions.player_save_generation();
    let mut save_requested = Box::pin(sessions.wait_for_player_save_request(observed));
    std::future::poll_fn(|cx| {
        assert!(
            save_requested.as_mut().poll(cx).is_pending(),
            "save request must wait for connection cleanup"
        );
        Poll::Ready(())
    })
    .await;
    let cleanup =
        RegisteredSessionCleanup::new(Arc::clone(&sessions), session_id, None, None, None);
    drop(cleanup);
    tokio::time::timeout(Duration::from_secs(1), save_requested)
        .await
        .expect("connection cleanup must push a player save request");

    let snapshots = sessions.persisted_player_states();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].0, profile.uuid);
    assert_eq!(snapshots[0].1.carried_item, ItemStack::new(10, 3));
    assert_eq!(
        sessions
            .recoverable_player_state(profile.uuid)
            .unwrap()
            .carried_item,
        ItemStack::new(10, 3)
    );
}

#[tokio::test]
async fn periodic_checkpoint_persists_cancelled_connection_owner_state() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:dirt").unwrap(),
        protocol_id: 10,
    }]));
    let mut config = play_loop_slow_client_test_config();
    config.items = Arc::clone(&items);
    config.world = Some(Arc::new(tokio::sync::Mutex::new(
        mc_world::WorldStorage::open(tmp.path(), Arc::clone(&config.blocks))
            .unwrap()
            .with_item_registry(Arc::clone(&items)),
    )));
    let sessions = Arc::new(SessionRegistry::new());
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("CheckpointCancelledOwnerState"),
        name: "CheckpointCancelledOwnerState".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = sessions.register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut state = PlayerPersistedState::new_default(pose);
    state.carried_item = ItemStack::new(10, 3);
    sessions.register_player_persistence(session_id, Arc::new(Mutex::new(state)));
    drop(RegisteredSessionCleanup::new(
        Arc::clone(&sessions),
        session_id,
        None,
        None,
        None,
    ));

    let shutdown = crate::server::ShutdownHandle::default();
    let (simulation, mut owner) = simulation_channel();
    let mut save = std::pin::pin!(crate::server::save_periodic_checkpoint(
        &config,
        sessions.as_ref(),
        &simulation,
        &shutdown,
    ));
    let command_ready = tokio::select! {
        report = &mut save => panic!("checkpoint completed before owner barrier: {report:?}"),
        ready = owner.wait_for_command() => ready,
    };
    assert!(command_ready);
    assert_eq!(
        owner
            .process_tick_with_world(&sessions, config.world.as_ref(), None, 1)
            .processed,
        1
    );

    let report = save.await.expect("checkpoint was not superseded");
    assert!(report.is_ok(), "checkpoint errors: {:?}", report.errors);
    assert_eq!(report.players_saved, 1);
    let loaded = load_player_state(
        tmp.path(),
        profile.uuid,
        &items,
        PlayerPersistedState::new_default(pose),
    )
    .unwrap()
    .unwrap();
    assert_eq!(loaded.carried_item, ItemStack::new(10, 3));
    assert!(sessions.persisted_player_states().is_empty());
}

#[tokio::test]
async fn disconnect_settles_table_grid_inventory_grid_and_cursor_in_one_owner_turn() {
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:dirt").unwrap(),
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(items);
    for slot in 9..=44 {
        state.inventory.slots[slot] = ItemStack::new(10, 64);
    }
    state.inventory.slots[9] = ItemStack::new(10, 63);
    state.inventory.slots[1] = ItemStack::new(10, 2);
    state.carried_item = ItemStack::new(10, 1);
    state.entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    let mut window = CraftingTableWindow::new(7);
    window.input[0] = ItemStack::new(10, 2);
    let crafting_table_input = crafting_table_input_projection(&window.input);
    state.active_container = Some(ActiveContainer::CraftingTable(Box::new(window)));

    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("AtomicCraftingDisconnect"),
        name: "AtomicCraftingDisconnect".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    saved.carried_item = state.carried_item.clone();
    saved.crafting_table_input = crafting_table_input;
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    let simulation_probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);

    assert!(settle_disconnected_inventory(&mut state, &saved).await);
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(simulation_probe.snapshot().processed, 1);
    assert!(state.active_container.is_none());
    let saved = saved.lock().unwrap();
    assert!(saved.inventory.slots[1].is_empty());
    assert_eq!(saved.inventory.slots[9], ItemStack::new(10, 64));
    assert!(saved.carried_item.is_empty());
    drop(saved);
    let drops = state.sessions.persisted_entity_records();
    assert_eq!(drops.len(), 3);
    assert_eq!(
        drops
            .iter()
            .map(|record| record.snapshot.item_stack.as_ref().unwrap().count)
            .sum::<i32>(),
        4
    );
}

#[tokio::test]
async fn disconnect_recovers_crafting_grid_after_connection_projection_is_lost() {
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:dirt").unwrap(),
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(items);
    state.inventory.slots[9] = ItemStack::new(10, 1);

    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("RecoveredCraftingDisconnect"),
        name: "RecoveredCraftingDisconnect".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    state.simulation = simulation.for_session(session_id);

    let mut writer = Vec::new();
    let window = handle_crafting_container_click(
        &mut state,
        &mut writer,
        Box::new(CraftingTableWindow::new(7)),
        None,
        GameMode::Survival,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 10,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();
    let window = handle_crafting_container_click(
        &mut state,
        &mut writer,
        window,
        None,
        GameMode::Survival,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 3,
            slot_num: 1,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();
    assert_eq!(window.input[0], ItemStack::new(10, 1));
    drop(window);

    assert!(settle_disconnected_inventory(&mut state, &saved).await);
    let _ = stop.send(());
    task.await.unwrap();

    let saved = saved.lock().unwrap();
    assert_eq!(
        saved
            .inventory
            .slots
            .iter()
            .map(|stack| stack.count.max(0))
            .sum::<i32>(),
        1,
        "the owner aggregate must recover a grid item after the connection projection is gone"
    );
    assert!(saved.carried_item.is_empty());
}

#[tokio::test]
async fn disconnect_recovers_enchanting_inputs_after_connection_projection_is_lost() {
    let items = Arc::new(mc_data::items::solaris_required_items());
    let pickaxe = items
        .id_of(&Identifier::parse("minecraft:stone_pickaxe").unwrap())
        .unwrap();
    let mut state = interaction_state_for_items(Arc::clone(&items));
    state.item_facts = Arc::new(mc_data::item_components::solaris_required_item_facts());
    state.inventory.slots[9] = ItemStack::new(pickaxe, 1);

    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("RecoveredEnchantingDisconnect"),
        name: "RecoveredEnchantingDisconnect".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    state.simulation = simulation.for_session(session_id);

    let mut writer = Vec::new();
    let xp = XpState::default();
    let window = handle_enchanting_container_click(
        &mut state,
        &mut writer,
        EnchantingTableWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 }),
        &xp,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 2,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();
    let window = handle_enchanting_container_click(
        &mut state,
        &mut writer,
        window,
        &xp,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 2,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();
    assert_eq!(window.inputs[0], ItemStack::new(pickaxe, 1));
    drop(window);

    assert!(settle_disconnected_inventory(&mut state, &saved).await);
    let _ = stop.send(());
    task.await.unwrap();

    let saved = saved.lock().unwrap();
    assert_eq!(
        saved
            .inventory
            .slots
            .iter()
            .map(|stack| stack.count.max(0))
            .sum::<i32>(),
        1,
        "the owner aggregate must recover an enchanting input after the connection projection is gone"
    );
    assert!(saved.carried_item.is_empty());
}

#[tokio::test]
async fn stale_crafting_click_rebuilds_grid_from_owner_projection() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, ShapelessRecipe,
    };

    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let stone = Identifier::parse("minecraft:stone").unwrap();
    let output = Identifier::parse("minecraft:test_output").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: dirt.clone(),
            protocol_id: 10,
        },
        ItemReport {
            id: stone.clone(),
            protocol_id: 11,
        },
        ItemReport {
            id: output.clone(),
            protocol_id: 12,
        },
    ]));
    let mut state = interaction_state_for_items(items);
    state.recipes.push(Recipe {
        id: Identifier::parse("minecraft:test_recipe").unwrap(),
        kind: RecipeKind::Shapeless(ShapelessRecipe {
            ingredients: vec![Ingredient {
                alternatives: vec![IngredientAlternative::Item(stone)],
            }],
        }),
        result: RecipeResult {
            item: output,
            count: 1,
        },
    });
    let mut local_window = CraftingTableWindow::new(7);
    local_window.input[0] = ItemStack::new(11, 1);
    refresh_crafting_result(&state, &mut local_window);
    assert_eq!(local_window.result, ItemStack::new(12, 1));

    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("StaleCraftOwner"),
        name: "StaleCraftOwner".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    let mut authoritative_input = std::array::from_fn(|_| ItemStack::EMPTY);
    authoritative_input[0] = ItemStack::new(10, 1);
    saved.crafting_table_input = Some(Box::new(authoritative_input.clone()));
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    state.simulation = simulation.for_session(session_id);
    let one = NonZeroUsize::new(1).unwrap();
    let (script_boundary, mut script_endpoint) = mc_script::script_boundary_pair(one, one);
    let script_events = ScriptGameplayEventPublisher::new(
        ScriptEventSink::new(script_boundary.clone()),
        ScriptPlayerId::new(session_id),
        profile.uuid.to_string(),
        &profile.name,
        CommandPermissions::from_op(false),
        "minecraft:overworld",
    );

    let mut writer = Vec::new();
    let window = handle_crafting_container_click(
        &mut state,
        &mut writer,
        Box::new(local_window),
        Some(&script_events),
        GameMode::Survival,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::Actual {
                item_id: 12,
                count: 1,
                components: mc_protocol::packets::play::HashedStackComponentHashes::empty(),
            },
        },
    )
    .await
    .unwrap();
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(window.state_id, 1);
    assert_eq!(window.input, authoritative_input);
    assert!(state.carried_item.is_empty());
    script_boundary
        .try_enqueue_event(ScriptEvent::server_tick(89))
        .unwrap();
    assert!(matches!(
        script_endpoint.recv_event().await.unwrap().kind(),
        mc_script::ScriptEventKind::ServerTick { tick: 89 }
    ));
}

#[test]
fn quick_move_crafted_event_counts_every_output_batch() {
    let result = ItemStack::new(2, 4);
    let mut before = PlayerInventory::empty();
    before.slots[9] = ItemStack::new(2, 5);
    let mut after = before.clone();
    after.slots[9] = ItemStack::new(2, 17);

    assert_eq!(
        crafted_item_from_inventory_delta(&result, &before, &after),
        Some(CraftedItem {
            item_id: 2,
            count: 12,
            craft_count: 3,
        })
    );
}

#[tokio::test]
async fn crafting_table_result_commit_publishes_once_before_fifo_fence() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, ShapelessRecipe,
    };

    let ingredient = Identifier::parse("minecraft:test_ingredient").unwrap();
    let output = Identifier::parse("minecraft:test_output").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: ingredient.clone(),
            protocol_id: 1,
        },
        ItemReport {
            id: output.clone(),
            protocol_id: 2,
        },
    ]));
    let mut state = interaction_state_for_items(items);
    state.recipes.push(Recipe {
        id: Identifier::parse("minecraft:test_recipe").unwrap(),
        kind: RecipeKind::Shapeless(ShapelessRecipe {
            ingredients: vec![Ingredient {
                alternatives: vec![IngredientAlternative::Item(ingredient)],
            }],
        }),
        result: RecipeResult {
            item: output,
            count: 4,
        },
    });
    let mut window = CraftingTableWindow::new(7);
    window.input[0] = ItemStack::new(1, 1);
    refresh_crafting_result(&state, &mut window);
    assert_eq!(window.result, ItemStack::new(2, 4));

    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("CraftEventOwner"),
        name: "CraftEventOwner".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    saved.crafting_table_input = crafting_table_input_projection(&window.input);
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    state.simulation = simulation.for_session(session_id);

    let one = NonZeroUsize::new(1).unwrap();
    let (script_boundary, mut script_endpoint) = mc_script::script_boundary_pair(one, one);
    let script_events = ScriptGameplayEventPublisher::new(
        ScriptEventSink::new(script_boundary.clone()),
        ScriptPlayerId::new(session_id),
        profile.uuid.to_string(),
        &profile.name,
        CommandPermissions::from_op(false),
        "minecraft:overworld",
    );
    let carried = mc_protocol::packets::play::HashedStack::Actual {
        item_id: 2,
        count: 4,
        components: mc_protocol::packets::play::HashedStackComponentHashes::empty(),
    };
    let mut writer = Vec::new();
    let window = handle_crafting_container_click(
        &mut state,
        &mut writer,
        Box::new(window),
        Some(&script_events),
        GameMode::Survival,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: carried.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(window.state_id, 4);
    assert!(window.input.iter().all(ItemStack::is_empty));
    assert_eq!(state.carried_item, ItemStack::new(2, 4));
    assert!(matches!(
        script_endpoint.recv_event().await.unwrap().kind(),
        mc_script::ScriptEventKind::PlayerItemCrafted {
            item_id,
            count: 4,
            craft_count: 1,
            source: ScriptCraftingSource::CraftingTable,
            ..
        } if item_id == "minecraft:test_output"
    ));

    let mut window = handle_crafting_container_click(
        &mut state,
        &mut writer,
        window,
        Some(&script_events),
        GameMode::Survival,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 4,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: carried.clone(),
        },
    )
    .await
    .unwrap();
    script_boundary
        .try_enqueue_event(ScriptEvent::server_tick(90))
        .unwrap();
    assert!(matches!(
        script_endpoint.recv_event().await.unwrap().kind(),
        mc_script::ScriptEventKind::ServerTick { tick: 90 }
    ));

    window.input[0] = ItemStack::new(1, 1);
    refresh_crafting_result(&state, &mut window);
    saved.lock().unwrap().crafting_table_input = crafting_table_input_projection(&window.input);
    script_boundary.close_event_admission();
    window = handle_crafting_container_click(
        &mut state,
        &mut writer,
        window,
        Some(&script_events),
        GameMode::Survival,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 4,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::Actual {
                item_id: 2,
                count: 8,
                components: mc_protocol::packets::play::HashedStackComponentHashes::empty(),
            },
        },
    )
    .await
    .unwrap();
    assert_eq!(window.state_id, 7);
    assert!(window.input.iter().all(ItemStack::is_empty));
    assert_eq!(state.carried_item, ItemStack::new(2, 8));

    let _ = stop.send(());
    task.await.unwrap();
}

#[tokio::test]
async fn inventory_result_paths_publish_only_after_owner_commit() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, ShapelessRecipe,
    };

    let ingredient = Identifier::parse("minecraft:test_ingredient").unwrap();
    let output = Identifier::parse("minecraft:test_output").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: ingredient.clone(),
            protocol_id: 1,
        },
        ItemReport {
            id: output.clone(),
            protocol_id: 2,
        },
    ]));
    let mut state = interaction_state_for_items(items);
    state.recipes.push(Recipe {
        id: Identifier::parse("minecraft:test_recipe").unwrap(),
        kind: RecipeKind::Shapeless(ShapelessRecipe {
            ingredients: vec![Ingredient {
                alternatives: vec![IngredientAlternative::Item(ingredient)],
            }],
        }),
        result: RecipeResult {
            item: output,
            count: 4,
        },
    });
    state.inventory.slots[1] = ItemStack::new(1, 1);
    refresh_inventory_crafting_result(&mut state);
    assert_eq!(state.inventory.slots[0], ItemStack::new(2, 4));

    let pose = PlayerPose::new(1.5, 65.0, 2.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("InvCraftOwner"),
        name: "InvCraftOwner".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    state.simulation = simulation.for_session(session_id);

    let one = NonZeroUsize::new(1).unwrap();
    let (script_boundary, mut script_endpoint) = mc_script::script_boundary_pair(one, one);
    let script_events = ScriptGameplayEventPublisher::new(
        ScriptEventSink::new(script_boundary.clone()),
        ScriptPlayerId::new(session_id),
        profile.uuid.to_string(),
        &profile.name,
        CommandPermissions::from_op(false),
        "minecraft:overworld",
    );
    let xp = XpState::default();
    let script_player_id = ScriptPlayerId::new(session_id);
    let script_context = no_script_player_context(session_id);
    let mut writer = Vec::new();
    let mismatch_state_id = state.inventory_state_id;

    handle_container_click(
        &mut state,
        &mut writer,
        ContainerClickContext {
            game_mode: GameMode::Survival,
            survival_state: SurvivalState::FULL,
            xp_state: &xp,
            player_pose: pose,
            script_events: Some(&script_events),
            scripts: None,
            script_player_id,
            script_context: script_context.clone(),
        },
        ServerboundContainerClick {
            container_id: 0,
            state_id: mismatch_state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::Actual {
                item_id: 1,
                count: 1,
                components: mc_protocol::packets::play::HashedStackComponentHashes::empty(),
            },
        },
    )
    .await
    .unwrap();
    assert_eq!(state.inventory.slots[1], ItemStack::new(1, 1));
    assert!(state.carried_item.is_empty());
    script_boundary
        .try_enqueue_event(ScriptEvent::server_tick(91))
        .unwrap();
    assert!(matches!(
        script_endpoint.recv_event().await.unwrap().kind(),
        mc_script::ScriptEventKind::ServerTick { tick: 91 }
    ));

    for slot in 9..=44 {
        state.inventory.slots[slot] = ItemStack::new(99, 64);
    }
    saved.lock().unwrap().inventory = state.inventory.clone();
    let full_state_id = state.inventory_state_id;
    handle_container_click(
        &mut state,
        &mut writer,
        ContainerClickContext {
            game_mode: GameMode::Survival,
            survival_state: SurvivalState::FULL,
            xp_state: &xp,
            player_pose: pose,
            script_events: Some(&script_events),
            scripts: None,
            script_player_id,
            script_context: script_context.clone(),
        },
        ServerboundContainerClick {
            container_id: 0,
            state_id: full_state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();
    assert_eq!(state.inventory.slots[1], ItemStack::new(1, 1));
    script_boundary
        .try_enqueue_event(ScriptEvent::server_tick(92))
        .unwrap();
    assert!(matches!(
        script_endpoint.recv_event().await.unwrap().kind(),
        mc_script::ScriptEventKind::ServerTick { tick: 92 }
    ));

    for slot in 9..=44 {
        state.inventory.slots[slot] = ItemStack::EMPTY;
    }
    saved.lock().unwrap().inventory = state.inventory.clone();
    let success_state_id = state.inventory_state_id;
    handle_container_click(
        &mut state,
        &mut writer,
        ContainerClickContext {
            game_mode: GameMode::Survival,
            survival_state: SurvivalState::FULL,
            xp_state: &xp,
            player_pose: pose,
            script_events: Some(&script_events),
            scripts: None,
            script_player_id,
            script_context,
        },
        ServerboundContainerClick {
            container_id: 0,
            state_id: success_state_id.wrapping_sub(1),
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::Actual {
                item_id: 2,
                count: 4,
                components: mc_protocol::packets::play::HashedStackComponentHashes::empty(),
            },
        },
    )
    .await
    .unwrap();
    assert!(state.inventory.slots[1].is_empty());
    assert_eq!(state.carried_item, ItemStack::new(2, 4));
    assert!(matches!(
        script_endpoint.recv_event().await.unwrap().kind(),
        mc_script::ScriptEventKind::PlayerItemCrafted {
            item_id,
            count: 4,
            craft_count: 1,
            source: ScriptCraftingSource::Inventory,
            ..
        } if item_id == "minecraft:test_output"
    ));

    let _ = stop.send(());
    task.await.unwrap();
}

fn stonecutter_test_recipe() -> mc_data::recipes::Recipe {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, StonecuttingRecipe,
    };

    Recipe {
        id: Identifier::parse("minecraft:test_stonecutter").unwrap(),
        kind: RecipeKind::Stonecutting(StonecuttingRecipe {
            ingredient: Ingredient {
                alternatives: vec![IngredientAlternative::Item(
                    Identifier::parse("minecraft:cobblestone").unwrap(),
                )],
            },
        }),
        result: RecipeResult {
            item: Identifier::parse("minecraft:cobblestone_slab").unwrap(),
            count: 2,
        },
    }
}

fn stonecutter_test_items() -> Arc<ItemRegistry> {
    Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            protocol_id: 0,
        },
        ItemReport {
            id: Identifier::parse("minecraft:cobblestone").unwrap(),
            protocol_id: 10,
        },
        ItemReport {
            id: Identifier::parse("minecraft:cobblestone_slab").unwrap(),
            protocol_id: 11,
        },
        ItemReport {
            id: Identifier::parse("minecraft:dirt").unwrap(),
            protocol_id: 12,
        },
    ]))
}

fn register_stonecutter_owner(
    state: &mut InteractionState,
    name: &str,
    pose: PlayerPose,
    input: &ItemStack,
) -> (LoggedInProfile, SessionId, Arc<Mutex<PlayerPersistedState>>) {
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid(name),
        name: name.to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut persisted = PlayerPersistedState::new_default(pose);
    persisted.inventory = state.inventory.clone();
    persisted.carried_item = state.carried_item.clone();
    persisted.crafting_table_input = stonecutter_input_projection(input);
    let persisted = Arc::new(Mutex::new(persisted));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&persisted));
    state.sessions.set_active_shield(
        session_id,
        state.shield_use.as_ref().map(|shield| ActiveShield {
            started_tick: shield.started_tick,
            slot: shield.slot,
            expected_stack: shield.stack.clone(),
        }),
    );
    state.session_id = session_id;
    (profile, session_id, persisted)
}

#[test]
fn stonecutter_invalid_selection_and_input_fail_closed() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    state.recipes.push(stonecutter_test_recipe());
    let mut window = StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 });
    window.input = ItemStack::new(12, 1);

    assert!(!select_stonecutter_recipe(&state, &mut window, 0));
    assert!(window.result.is_empty());

    window.input = ItemStack::new(10, 1);
    assert!(!select_stonecutter_recipe(&state, &mut window, 1));
    assert!(window.result.is_empty());
}

#[test]
fn stonecutter_selection_uses_the_filtered_advertised_offer_order() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    let mut air = stonecutter_test_recipe();
    air.result.item = Identifier::parse("minecraft:air").unwrap();
    let mut over_stack = stonecutter_test_recipe();
    over_stack.result.count = 65;
    state
        .recipes
        .extend([air, over_stack, stonecutter_test_recipe()]);
    let mut window = StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 });
    window.input = ItemStack::new(10, 1);

    assert!(select_stonecutter_recipe(&state, &mut window, 0));
    assert_eq!(window.result, ItemStack::new(11, 2));
    assert!(!select_stonecutter_recipe(&state, &mut window, 1));
    assert!(window.result.is_empty());
}

#[test]
fn stonecutter_quick_move_rejects_input_with_no_advertised_offer() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    let mut unsupported = stonecutter_test_recipe();
    unsupported.result.item = Identifier::parse("minecraft:missing_result").unwrap();
    state.recipes.push(unsupported);
    state.inventory.slots[9] = ItemStack::new(10, 1);
    let mut window = StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 });

    assert!(apply_stonecutter_quick_move_click(
        &mut state,
        &mut window,
        2,
    ));
    assert!(state.inventory.slots[9].is_empty());
    assert_eq!(state.inventory.slots[36], ItemStack::new(10, 1));
    assert!(window.input.is_empty());
}

#[test]
fn stonecutter_quick_move_crafts_until_input_is_exhausted() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    state.recipes.push(stonecutter_test_recipe());
    let mut window = StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 });
    window.input = ItemStack::new(10, 2);
    assert!(select_stonecutter_recipe(&state, &mut window, 0));

    assert!(apply_stonecutter_quick_move_click(
        &mut state,
        &mut window,
        1
    ));

    assert!(window.input.is_empty());
    assert!(window.result.is_empty());
    assert_eq!(state.inventory.slots[9], ItemStack::new(11, 4));
    assert_eq!(
        window.input.count + state.inventory.slots[9].count / 2,
        2,
        "all craftable cobblestone must debit in the same candidate that credits the slabs",
    );
}

#[test]
fn stonecutter_quick_move_stops_at_exact_result_capacity() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    state.recipes.push(stonecutter_test_recipe());
    state.item_facts = Arc::new(ItemFactsTable::from_entries([(
        Identifier::parse("minecraft:cobblestone_slab").unwrap(),
        mc_data::item_components::ItemFacts {
            max_stack_size: Some(16),
            ..mc_data::item_components::ItemFacts::default()
        },
    )]));
    for slot in 9..=44 {
        state.inventory.slots[slot] = ItemStack::new(12, 64);
    }
    state.inventory.slots[9] = ItemStack::new(11, 12);
    let mut window = StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 });
    window.input = ItemStack::new(10, 4);
    assert!(select_stonecutter_recipe(&state, &mut window, 0));

    assert!(apply_stonecutter_quick_move_click(
        &mut state,
        &mut window,
        1
    ));

    assert_eq!(window.input, ItemStack::new(10, 2));
    assert_eq!(window.result, ItemStack::new(11, 2));
    assert_eq!(state.inventory.slots[9], ItemStack::new(11, 16));
    assert_eq!(
        window.input.count + (state.inventory.slots[9].count - 12) / 2,
        4,
    );
}

#[tokio::test]
async fn stonecutter_result_pickup_commits_input_and_cursor_through_owner() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    state.recipes.push(stonecutter_test_recipe());
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let input = ItemStack::new(10, 2);
    let (_, session_id, persisted) =
        register_stonecutter_owner(&mut state, "StonecutterPickupOwner", pose, &input);
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    let probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);
    let mut window = StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 });
    window.input = input;
    assert!(select_stonecutter_recipe(&state, &mut window, 0));

    let mut writer = Vec::new();
    let window = handle_stonecutter_container_click(
        &mut state,
        &mut writer,
        window,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 1,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(probe.snapshot().processed, 1);
    assert_eq!(window.state_id, 2);
    assert_eq!(window.input, ItemStack::new(10, 1));
    assert_eq!(state.carried_item, ItemStack::new(11, 2));
    let persisted = persisted.lock().unwrap();
    assert_eq!(persisted.carried_item, state.carried_item);
    assert_eq!(
        stonecutter_input_from_projection(persisted.crafting_table_input.clone()),
        window.input,
    );
}

#[tokio::test]
async fn stonecutter_result_quick_move_commits_all_outputs_in_one_owner_turn() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    state.recipes.push(stonecutter_test_recipe());
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let input = ItemStack::new(10, 4);
    let (_, session_id, persisted) =
        register_stonecutter_owner(&mut state, "StonecutterQuickMoveOwner", pose, &input);
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    let probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);
    let mut window = StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 });
    window.input = input;
    assert!(select_stonecutter_recipe(&state, &mut window, 0));

    let mut writer = Vec::new();
    let window = handle_stonecutter_container_click(
        &mut state,
        &mut writer,
        window,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 1,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(probe.snapshot().processed, 1);
    assert_eq!(window.state_id, 2);
    assert!(window.input.is_empty());
    assert_eq!(state.inventory.slots[9], ItemStack::new(11, 8));
    let persisted = persisted.lock().unwrap();
    assert_eq!(persisted.inventory.slots, state.inventory.slots);
    assert!(persisted.crafting_table_input.is_none());
}

#[tokio::test]
async fn stonecutter_output_plan_rejects_stale_owner_snapshot_and_resyncs() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    state.recipes.push(stonecutter_test_recipe());
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let input = ItemStack::new(10, 2);
    let (_, session_id, persisted) =
        register_stonecutter_owner(&mut state, "StaleStonecutterOutput", pose, &input);
    let sessions = Arc::clone(&state.sessions);
    let (simulation, mut owner) = simulation_channel();
    let probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);
    let mut window = StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 });
    window.input = input;
    assert!(select_stonecutter_recipe(&state, &mut window, 0));
    let mut writer = Vec::new();
    let mut click = Box::pin(handle_stonecutter_container_click(
        &mut state,
        &mut writer,
        window,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 1,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    ));
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(click.as_mut(), cx).is_pending(),
            "stonecutter output must wait for its owner commit",
        );
        assert_eq!(probe.snapshot().depth, 1);
        Poll::Ready(())
    })
    .await;
    persisted.lock().unwrap().inventory.slots[9] = ItemStack::new(12, 1);
    assert_eq!(owner.process_tick(&sessions, 1).processed, 1);

    let window = click.await.unwrap();
    assert_eq!(window.state_id, 1);
    assert_eq!(window.input, ItemStack::new(10, 2));
    assert_eq!(state.inventory.slots[9], ItemStack::new(12, 1));
    assert!(state.carried_item.is_empty());
    let packets = decode_container_set_content_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].state_id, 1);
    assert_eq!(packets[0].items[0], ItemStack::new(10, 2));
    assert!(packets[0].items[1].is_empty());
}

#[tokio::test]
async fn stonecutter_output_plan_rejects_stale_session_without_conservation_loss() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    state.recipes.push(stonecutter_test_recipe());
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let input = ItemStack::new(10, 2);
    let (_, session_id, persisted) =
        register_stonecutter_owner(&mut state, "StaleStonecutterSession", pose, &input);
    let sessions = Arc::clone(&state.sessions);
    let (simulation, mut owner) = simulation_channel();
    let probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);
    let mut window = StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 });
    window.input = input;
    assert!(select_stonecutter_recipe(&state, &mut window, 0));
    let mut writer = Vec::new();
    let mut click = Box::pin(handle_stonecutter_container_click(
        &mut state,
        &mut writer,
        window,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 1,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    ));
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(click.as_mut(), cx).is_pending(),
            "stonecutter output must be planned before stale-session rejection",
        );
        assert_eq!(probe.snapshot().depth, 1);
        Poll::Ready(())
    })
    .await;
    let _ = sessions.unregister(session_id);
    assert_eq!(owner.process_tick(&sessions, 1).processed, 0);

    assert!(matches!(
        click.await,
        Err(ConnectionError::RuntimeUnavailable {
            operation: "committing stonecutter input"
        })
    ));
    assert_eq!(probe.snapshot().rejected_stale_session, 1);
    assert!(state.inventory.slots[9].is_empty());
    assert!(state.carried_item.is_empty());
    let persisted = persisted.lock().unwrap();
    assert!(persisted.inventory.slots[9].is_empty());
    assert_eq!(
        stonecutter_input_from_projection(persisted.crafting_table_input.clone()),
        ItemStack::new(10, 2),
    );
}

#[tokio::test]
async fn stonecutter_disconnect_rejoin_conserves_crafted_output_and_remaining_input() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    state.recipes.push(stonecutter_test_recipe());
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let input = ItemStack::new(10, 2);
    let (profile, session_id, persisted) =
        register_stonecutter_owner(&mut state, "StonecutterRejoin", pose, &input);
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    let probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);
    let mut window = StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 });
    window.input = input;
    assert!(select_stonecutter_recipe(&state, &mut window, 0));
    let mut writer = Vec::new();
    let window = handle_stonecutter_container_click(
        &mut state,
        &mut writer,
        window,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 1,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();
    state.active_container = Some(ActiveContainer::Stonecutter(window));

    assert!(settle_disconnected_inventory(&mut state, &persisted).await);
    let _ = stop.send(());
    task.await.unwrap();
    assert_eq!(probe.snapshot().processed, 2);
    let _ = state
        .sessions
        .unregister_preserving_player_state(session_id);
    let recovered = state
        .sessions
        .recoverable_player_state(profile.uuid)
        .expect("settled stonecutter state must be recoverable on rejoin");
    let (tx, _rx) = mpsc::channel(8);
    let (rejoined, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    state
        .sessions
        .register_player_persistence(rejoined, Arc::new(Mutex::new(recovered.clone())));
    let (inventory, carried_item) = state
        .sessions
        .player_container_state(rejoined)
        .expect("rejoined player container state");

    assert_eq!(inventory.slots[9], ItemStack::new(10, 1));
    assert_eq!(inventory.slots[10], ItemStack::new(11, 2));
    assert!(carried_item.is_empty());
    assert!(recovered.crafting_table_input.is_none());
    assert_eq!(
        inventory.slots[9].count + inventory.slots[10].count / 2,
        2,
        "disconnect and rejoin must conserve the two original cobblestone",
    );
}

#[tokio::test]
async fn stale_stonecutter_click_rebuilds_input_from_owner_projection() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    state.recipes.push(stonecutter_test_recipe());
    state.inventory.slots[9] = ItemStack::new(12, 1);
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("StaleStonecutterProjection"),
        name: "StaleStonecutterProjection".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    let mut authoritative_input = std::array::from_fn(|_| ItemStack::EMPTY);
    authoritative_input[0] = ItemStack::new(10, 2);
    saved.crafting_table_input = Some(Box::new(authoritative_input));
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    state.simulation = simulation.for_session(session_id);

    let mut writer = Vec::new();
    let window = handle_stonecutter_container_click(
        &mut state,
        &mut writer,
        StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 }),
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 2,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(window.state_id, 1);
    assert_eq!(window.input, ItemStack::new(10, 2));
    assert_eq!(state.inventory.slots[9], ItemStack::new(12, 1));
    assert!(state.carried_item.is_empty());
}

#[tokio::test]
async fn stonecutter_close_reopen_conserves_input_through_one_owner_turn() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:stonecutter"),
        ])
        .unwrap(),
    );
    let mut state = interaction_state_for_blocks(blocks);
    state.items = stonecutter_test_items();
    state.recipes.push(stonecutter_test_recipe());
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let position = mc_world::BlockPos { x: 0, y: 64, z: 0 };
    {
        let mut storage = state.world.lock().await;
        let chunk = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage.set_block_at(position, BlockStateId(1)).unwrap();
    }
    let mut window = StonecutterWindow::at_position(7, position);
    window.input = ItemStack::new(10, 2);
    state.active_container = Some(ActiveContainer::Stonecutter(window));

    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("AtomicStonecutterClose"),
        name: "AtomicStonecutterClose".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.crafting_table_input = stonecutter_input_projection(&ItemStack::new(10, 2));
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    let probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);

    store_active_container(&mut state, pose).await.unwrap();
    let _ = stop.send(());
    task.await.unwrap();

    let mut writer = Vec::new();
    assert!(
        open_stonecutter_container(&mut state, &mut writer, pose, 8, position)
            .await
            .unwrap()
    );

    assert_eq!(probe.snapshot().processed, 1);
    let Some(ActiveContainer::Stonecutter(window)) = state.active_container.as_ref() else {
        panic!("stonecutter must reopen");
    };
    assert!(window.input.is_empty());
    let saved = saved.lock().unwrap();
    assert_eq!(saved.inventory.slots[9], ItemStack::new(10, 2));
    assert!(saved.crafting_table_input.is_none());
}

#[tokio::test]
async fn stale_enchanting_click_rebuilds_inputs_from_owner_projection() {
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: Identifier::parse("minecraft:dirt").unwrap(),
            protocol_id: 10,
        },
        ItemReport {
            id: Identifier::parse("minecraft:stone").unwrap(),
            protocol_id: 11,
        },
    ]));
    let mut state = interaction_state_for_items(items);
    state.inventory.slots[9] = ItemStack::new(11, 1);

    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("StaleEnchantingProjection"),
        name: "StaleEnchantingProjection".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    let authoritative_input = [ItemStack::new(10, 1), ItemStack::EMPTY];
    saved.enchanting_table_input = Some(Box::new(authoritative_input.clone()));
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, saved);
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    state.simulation = simulation.for_session(session_id);

    let mut writer = Vec::new();
    let window = handle_enchanting_container_click(
        &mut state,
        &mut writer,
        EnchantingTableWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 }),
        &XpState::default(),
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 2,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(window.state_id, 1);
    assert_eq!(window.inputs, authoritative_input);
    assert_eq!(state.inventory.slots[9], ItemStack::new(11, 1));
    assert!(state.carried_item.is_empty());
}

#[tokio::test]
async fn disconnect_settlement_fails_closed_when_owner_is_unavailable() {
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:dirt").unwrap(),
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(items);
    state.inventory.slots[1] = ItemStack::new(10, 2);
    state.carried_item = ItemStack::new(10, 1);
    let mut window = CraftingTableWindow::new(7);
    window.input[0] = ItemStack::new(10, 3);
    let crafting_table_input = crafting_table_input_projection(&window.input);
    state.active_container = Some(ActiveContainer::CraftingTable(Box::new(window)));

    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("UnavailableCraftingDisconnect"),
        name: "UnavailableCraftingDisconnect".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    saved.carried_item = state.carried_item.clone();
    saved.crafting_table_input = crafting_table_input;
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, owner) = simulation_channel();
    drop(owner);
    state.simulation = simulation.for_session(session_id);

    assert!(!settle_disconnected_inventory(&mut state, &saved).await);

    let Some(ActiveContainer::CraftingTable(window)) = &state.active_container else {
        panic!("failed settlement must not discard the connection-local crafting grid");
    };
    assert_eq!(window.input[0], ItemStack::new(10, 3));
    assert_eq!(state.inventory.slots[1], ItemStack::new(10, 2));
    assert_eq!(state.carried_item, ItemStack::new(10, 1));
    let saved = saved.lock().unwrap();
    assert_eq!(saved.inventory.slots[1], ItemStack::new(10, 2));
    assert_eq!(saved.carried_item, ItemStack::new(10, 1));
    assert!(state.sessions.persisted_entity_records().is_empty());
}

#[test]
fn use_max_recipe_is_bounded_when_output_recreates_ingredient() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, ShapelessRecipe,
    };

    let item = Identifier::parse("minecraft:loop_item").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: item.clone(),
        protocol_id: 1,
    }]));
    let mut state = interaction_state_for_items(items);
    state.inventory.slots[9] = ItemStack::new(1, 1);
    let recipe = Recipe {
        id: Identifier::parse("minecraft:loop_recipe").unwrap(),
        kind: RecipeKind::Shapeless(ShapelessRecipe {
            ingredients: vec![Ingredient {
                alternatives: vec![IngredientAlternative::Item(item.clone())],
            }],
        }),
        result: RecipeResult { item, count: 1 },
    };

    let outcome = craft_recipe(&mut state, &recipe, true).expect("one bounded craft");

    assert!(!outcome.changed_slots.is_empty());
    assert_eq!(
        outcome.crafted,
        CraftedItem {
            item_id: 1,
            count: 1,
            craft_count: 1,
        }
    );
    assert_eq!(state.inventory.slots[9], ItemStack::new(1, 1));
}

#[test]
fn use_max_recipe_reports_large_aggregate_without_partial_mutation_failure() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, ShapelessRecipe,
    };

    let ingredient = Identifier::parse("minecraft:test_ingredient").unwrap();
    let output = Identifier::parse("minecraft:test_output").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: ingredient.clone(),
            protocol_id: 1,
        },
        ItemReport {
            id: output.clone(),
            protocol_id: 2,
        },
    ]));
    let mut state = interaction_state_for_items(items);
    state.item_facts = Arc::new(ItemFactsTable::from_entries([(
        output.clone(),
        mc_data::item_components::ItemFacts {
            max_stack_size: Some(2_000_000_000),
            ..mc_data::item_components::ItemFacts::default()
        },
    )]));
    for slot in 9..=11 {
        state.inventory.slots[slot] = ItemStack::new(1, 1);
    }
    let recipe = Recipe {
        id: Identifier::parse("minecraft:test_large_output").unwrap(),
        kind: RecipeKind::Shapeless(ShapelessRecipe {
            ingredients: vec![Ingredient {
                alternatives: vec![IngredientAlternative::Item(ingredient)],
            }],
        }),
        result: RecipeResult {
            item: output,
            count: 1_500_000_000,
        },
    };

    let outcome = craft_recipe(&mut state, &recipe, true).expect("three complete crafts");

    assert_eq!(outcome.crafted.item_id, 2);
    assert_eq!(outcome.crafted.count, 4_500_000_000);
    assert_eq!(outcome.crafted.craft_count, 3);
    assert_eq!(
        state.inventory.slots[9..=44]
            .iter()
            .map(|stack| i64::from(stack.count.max(0)))
            .sum::<i64>(),
        4_500_000_000
    );
}

#[tokio::test]
async fn placed_recipe_commits_inventory_and_publishes_aggregate_craft() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, ShapelessRecipe,
    };

    let ingredient = Identifier::parse("minecraft:test_ingredient").unwrap();
    let output = Identifier::parse("minecraft:test_output").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: ingredient.clone(),
            protocol_id: 1,
        },
        ItemReport {
            id: output.clone(),
            protocol_id: 2,
        },
    ]));
    let mut state = interaction_state_for_items(items);
    state.inventory.slots[9] = ItemStack::new(1, 1);
    state.recipes.push(Recipe {
        id: Identifier::parse("minecraft:test_recipe").unwrap(),
        kind: RecipeKind::Shapeless(ShapelessRecipe {
            ingredients: vec![Ingredient {
                alternatives: vec![IngredientAlternative::Item(ingredient)],
            }],
        }),
        result: RecipeResult {
            item: output,
            count: 1,
        },
    });
    let session_id = register_interaction_player(&mut state, "RecipeOwner");
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    let simulation_probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);
    let mut writer = Vec::new();
    let one = NonZeroUsize::new(1).unwrap();
    let (script_boundary, mut script_endpoint) = mc_script::script_boundary_pair(one, one);
    let script_events = ScriptGameplayEventPublisher::new(
        ScriptEventSink::new(script_boundary.clone()),
        ScriptPlayerId::new(session_id),
        "recipe-owner",
        "RecipeOwner",
        CommandPermissions::from_op(false),
        "minecraft:overworld",
    );

    handle_place_recipe(
        &mut state,
        &mut writer,
        Some(&script_events),
        PlayerPose::new(0.5, 64.0, 0.5),
        GameMode::Survival,
        SurvivalState::FULL,
        ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: 0,
            use_max_items: false,
        },
    )
    .await
    .unwrap();

    let (owner_inventory, owner_carried_item) = state
        .sessions
        .player_container_state(session_id)
        .expect("registered owner inventory");
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(simulation_probe.snapshot().processed, 1);
    assert_eq!(state.inventory.slots[9], ItemStack::new(2, 1));
    assert_eq!(owner_inventory.slots, state.inventory.slots);
    assert_eq!(owner_carried_item, state.carried_item);
    assert!(matches!(
        script_endpoint.recv_event().await.unwrap().kind(),
        mc_script::ScriptEventKind::PlayerItemCrafted {
            item_id,
            count: 1,
            craft_count: 1,
            source: ScriptCraftingSource::Inventory,
            game_mode: mc_script::ScriptGameMode::Survival,
            ..
        } if item_id == "minecraft:test_output"
    ));

    handle_place_recipe(
        &mut state,
        &mut writer,
        Some(&script_events),
        PlayerPose::new(0.5, 64.0, 0.5),
        GameMode::Survival,
        SurvivalState::FULL,
        ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: 0,
            use_max_items: true,
        },
    )
    .await
    .unwrap();
    script_boundary
        .try_enqueue_event(ScriptEvent::server_tick(88))
        .unwrap();
    assert!(matches!(
        script_endpoint.recv_event().await.unwrap().kind(),
        mc_script::ScriptEventKind::ServerTick { tick: 88 }
    ));
}

fn register_interaction_player(state: &mut InteractionState, name: &str) -> SessionId {
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid(name),
        name: name.to_owned(),
    };
    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut persisted = PlayerPersistedState::new_default(pose);
    persisted.inventory = state.inventory.clone();
    state
        .sessions
        .register_player_persistence(session_id, Arc::new(Mutex::new(persisted)));
    state.session_id = session_id;
    session_id
}

#[tokio::test]
async fn rejected_visible_block_edit_resyncs_authoritative_cached_state() {
    let state = interaction_state_for_items(Arc::new(ItemRegistry::from_report(&[])));
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = state.world.lock().await;
        let cpos = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage.set_block_at(pos, BlockStateId(1)).unwrap();
    }
    let mut writer = Vec::new();
    let world = Arc::clone(&state.world);
    let world_writer = world.lock().await;
    let edits = [BlockEdit {
        pos,
        new_state: BlockStateId(0),
    }];

    let mut resync = Box::pin(send_loaded_block_edit_resyncs(&state, &mut writer, &edits));
    std::future::poll_fn(|cx| match std::future::Future::poll(resync.as_mut(), cx) {
        std::task::Poll::Ready(result) => std::task::Poll::Ready(result),
        std::task::Poll::Pending => {
            panic!("loaded block resync must not wait for the world writer")
        }
    })
    .await
    .unwrap();
    drop(resync);
    drop(world_writer);

    let mut buf = bytes::BytesMut::from(writer.as_slice());
    let mut frame = mc_protocol::frame::try_decode_frame(&mut buf, Compression::Disabled)
        .unwrap()
        .expect("authoritative block update");
    assert_eq!(frame.id, BlockUpdate::ID);
    let update = BlockUpdate::decode(&mut frame.body).unwrap();
    assert_eq!(update.position, pack_block_pos(pos.x, pos.y, pos.z));
    assert_eq!(update.state_id, 1);
    assert!(buf.is_empty());
}

#[tokio::test]
async fn rejected_use_item_on_resync_does_not_wait_for_world_writer() {
    let mut state = interaction_state_for_items(Arc::new(ItemRegistry::from_report(&[])));
    let clicked = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let target = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    {
        let mut storage = state.world.lock().await;
        let cpos = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage.set_block_at(clicked, BlockStateId(1)).unwrap();
        storage.set_block_at(target, BlockStateId(2)).unwrap();
    }
    let world = Arc::clone(&state.world);
    let world_writer = world.lock().await;
    let mut writer = Vec::new();
    let mut resync = Box::pin(reject_use_item_on_with_resync(
        &mut state,
        &mut writer,
        InteractionHand::MainHand,
        17,
        clicked,
        target,
        UseItemOnNoOpReason::ConcurrentMutation,
        UseItemOnResyncOptions::WITH_HELD_ITEM,
    ));

    std::future::poll_fn(|cx| match std::future::Future::poll(resync.as_mut(), cx) {
        std::task::Poll::Ready(result) => std::task::Poll::Ready(result),
        std::task::Poll::Pending => {
            panic!("UseItemOn resync must not wait for the world writer")
        }
    })
    .await
    .unwrap();
    drop(resync);
    drop(world_writer);

    let mut buf = bytes::BytesMut::from(writer.as_slice());
    let mut first = mc_protocol::frame::try_decode_frame(&mut buf, Compression::Disabled)
        .unwrap()
        .expect("clicked block update");
    assert_eq!(first.id, BlockUpdate::ID);
    assert_eq!(BlockUpdate::decode(&mut first.body).unwrap().state_id, 1);
    let mut second = mc_protocol::frame::try_decode_frame(&mut buf, Compression::Disabled)
        .unwrap()
        .expect("target block update");
    assert_eq!(second.id, BlockUpdate::ID);
    assert_eq!(BlockUpdate::decode(&mut second.body).unwrap().state_id, 2);
    let ack = mc_protocol::frame::try_decode_frame(&mut buf, Compression::Disabled)
        .unwrap()
        .expect("block changed acknowledgement");
    assert_eq!(ack.id, BlockChangedAck::ID);
    assert!(buf.is_empty());
}

fn shield_item_state() -> InteractionState {
    let shield = Identifier::parse("minecraft:shield").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: shield,
        protocol_id: 77,
    }]));
    interaction_state_for_items(items)
}

fn decode_container_set_slot_packets(bytes: &[u8]) -> Vec<ClientboundContainerSetSlot> {
    let mut buf = bytes::BytesMut::from(bytes);
    let mut packets = Vec::new();
    while let Some(mut frame) =
        mc_protocol::frame::try_decode_frame(&mut buf, Compression::Disabled).unwrap()
    {
        if frame.id == ClientboundContainerSetSlot::ID {
            packets.push(ClientboundContainerSetSlot::decode(&mut frame.body).unwrap());
        }
    }
    assert!(buf.is_empty());
    packets
}

fn decode_container_set_content_packets(bytes: &[u8]) -> Vec<ClientboundContainerSetContent> {
    let mut buf = bytes::BytesMut::from(bytes);
    let mut packets = Vec::new();
    while let Some(mut frame) =
        mc_protocol::frame::try_decode_frame(&mut buf, Compression::Disabled).unwrap()
    {
        if frame.id == ClientboundContainerSetContent::ID {
            packets.push(ClientboundContainerSetContent::decode(&mut frame.body).unwrap());
        }
    }
    assert!(buf.is_empty());
    packets
}

fn decode_player_position_sync_packets(bytes: &[u8]) -> Vec<SynchronizePlayerPosition> {
    let mut buf = bytes::BytesMut::from(bytes);
    let mut packets = Vec::new();
    while let Some(mut frame) =
        mc_protocol::frame::try_decode_frame(&mut buf, Compression::Disabled).unwrap()
    {
        if frame.id == SynchronizePlayerPosition::ID {
            packets.push(SynchronizePlayerPosition::decode(&mut frame.body).unwrap());
        }
    }
    assert!(buf.is_empty());
    packets
}

#[test]
fn furnace_window_swap_and_throw_mutate_menu_slots() {
    let coal = Identifier::parse("minecraft:coal").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: coal,
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(Arc::clone(&items));
    let coal_id = items
        .id_of(&Identifier::parse("minecraft:coal").unwrap())
        .unwrap();
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(coal_id, 4);
    let mut furnace = FurnaceBlockEntity::default();

    assert!(apply_furnace_swap_click(
        &mut state,
        &mut furnace,
        FurnaceKind::Furnace,
        1,
        0,
    ));
    assert_eq!(
        furnace_slot_to_stack(&furnace.slots[1]),
        ItemStack::new(coal_id, 4)
    );
    assert!(state.inventory.slots[PlayerInventory::HOTBAR_BASE].is_empty());

    let dropped = apply_furnace_throw_click(&mut state, &mut furnace, 1, 0).unwrap();
    assert_eq!(dropped, ItemStack::new(coal_id, 1));
    assert_eq!(
        furnace_slot_to_stack(&furnace.slots[1]),
        ItemStack::new(coal_id, 3)
    );
}

#[test]
fn furnace_uses_vanilla_common_fuel_times_and_returns_lava_bucket() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let raw_food = Identifier::parse("minecraft:raw_food").unwrap();
    let cooked_food = Identifier::parse("minecraft:cooked_food").unwrap();
    let item_names = [
        raw_food.clone(),
        cooked_food.clone(),
        Identifier::parse("minecraft:stick").unwrap(),
        Identifier::parse("minecraft:birch_planks").unwrap(),
        Identifier::parse("minecraft:wooden_pickaxe").unwrap(),
        Identifier::parse("minecraft:coal").unwrap(),
        Identifier::parse("minecraft:lava_bucket").unwrap(),
        Identifier::parse("minecraft:bucket").unwrap(),
        Identifier::parse("minecraft:oak_stairs").unwrap(),
        Identifier::parse("minecraft:oak_slab").unwrap(),
        Identifier::parse("minecraft:chest").unwrap(),
        Identifier::parse("minecraft:oak_door").unwrap(),
        Identifier::parse("minecraft:oak_boat").unwrap(),
        Identifier::parse("minecraft:white_wool").unwrap(),
        Identifier::parse("minecraft:white_carpet").unwrap(),
        Identifier::parse("minecraft:dried_kelp_block").unwrap(),
        Identifier::parse("minecraft:bamboo").unwrap(),
        Identifier::parse("minecraft:warped_planks").unwrap(),
        Identifier::parse("minecraft:warped_stairs").unwrap(),
    ];
    let reports = item_names
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, id)| ItemReport {
            id,
            protocol_id: u32::try_from(index + 1).unwrap(),
        })
        .collect::<Vec<_>>();
    let items = ItemRegistry::from_report(&reports);
    let raw_food_id = items.id_of(&raw_food).unwrap();
    let recipe = Recipe {
        id: Identifier::parse("minecraft:test_food").unwrap(),
        kind: RecipeKind::Smelting(SmeltingRecipe {
            ingredient: Ingredient {
                alternatives: vec![IngredientAlternative::Item(raw_food)],
            },
            cooking_time: 200,
            experience_milli: 0,
        }),
        result: RecipeResult {
            item: cooked_food,
            count: 1,
        },
    };
    let tags = mc_data::tags::solaris_required_item_tags(&items);

    for (fuel_name, expected_ticks) in [
        ("minecraft:stick", 100),
        ("minecraft:birch_planks", 300),
        ("minecraft:wooden_pickaxe", 200),
        ("minecraft:coal", 1600),
        ("minecraft:lava_bucket", 20_000),
        ("minecraft:oak_stairs", 300),
        ("minecraft:oak_slab", 150),
        ("minecraft:chest", 300),
        ("minecraft:oak_door", 200),
        ("minecraft:oak_boat", 1_200),
        ("minecraft:white_wool", 100),
        ("minecraft:white_carpet", 67),
        ("minecraft:dried_kelp_block", 4_001),
        ("minecraft:bamboo", 50),
    ] {
        let fuel_id = items.id_of(&Identifier::parse(fuel_name).unwrap()).unwrap();
        let mut furnace = FurnaceBlockEntity {
            burn_total: 0,
            ..FurnaceBlockEntity::default()
        };
        furnace.slots[0] = stack_to_furnace_slot(&ItemStack::new(raw_food_id, 1));
        furnace.slots[1] = stack_to_furnace_slot(&ItemStack::new(fuel_id, 1));

        let _ = tick_furnace(
            std::slice::from_ref(&recipe),
            &items,
            &tags,
            &mut furnace,
            FurnaceKind::Furnace,
        );

        assert_eq!(
            furnace.burn_total, expected_ticks,
            "wrong burn duration for {fuel_name}"
        );
        assert_eq!(furnace.burn_remaining, expected_ticks);
        if fuel_name == "minecraft:lava_bucket" {
            assert_eq!(
                furnace_slot_to_stack(&furnace.slots[1]),
                ItemStack::new(
                    items
                        .id_of(&Identifier::parse("minecraft:bucket").unwrap())
                        .unwrap(),
                    1,
                )
            );
        } else {
            assert!(furnace.slots[1].is_empty());
        }
    }

    let coal_id = items
        .id_of(&Identifier::parse("minecraft:coal").unwrap())
        .unwrap();
    assert_eq!(
        furnace_fuel_ticks(&tags, FurnaceKind::Smoker, coal_id),
        Some(800)
    );
    assert_eq!(
        furnace_fuel_ticks(&tags, FurnaceKind::BlastFurnace, coal_id),
        Some(800)
    );

    let warped_planks = items
        .id_of(&Identifier::parse("minecraft:warped_planks").unwrap())
        .unwrap();
    let mut furnace = FurnaceBlockEntity {
        burn_total: 0,
        ..FurnaceBlockEntity::default()
    };
    furnace.slots[0] = stack_to_furnace_slot(&ItemStack::new(raw_food_id, 1));
    furnace.slots[1] = stack_to_furnace_slot(&ItemStack::new(warped_planks, 1));
    let _ = tick_furnace(
        std::slice::from_ref(&recipe),
        &items,
        &tags,
        &mut furnace,
        FurnaceKind::Furnace,
    );
    assert_eq!(furnace.burn_total, 0);
    assert_eq!(
        furnace_slot_to_stack(&furnace.slots[1]),
        ItemStack::new(warped_planks, 1)
    );

    let warped_stairs = items
        .id_of(&Identifier::parse("minecraft:warped_stairs").unwrap())
        .unwrap();
    furnace.slots[1] = stack_to_furnace_slot(&ItemStack::new(warped_stairs, 1));
    let _ = tick_furnace(
        std::slice::from_ref(&recipe),
        &items,
        &tags,
        &mut furnace,
        FurnaceKind::Furnace,
    );
    assert_eq!(furnace.burn_total, 0);
    assert_eq!(
        furnace_slot_to_stack(&furnace.slots[1]),
        ItemStack::new(warped_stairs, 1)
    );
}

#[test]
fn furnace_cools_partial_progress_when_the_fuel_slot_is_empty() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let raw_food = Identifier::parse("minecraft:raw_food").unwrap();
    let cooked_food = Identifier::parse("minecraft:cooked_food").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: raw_food.clone(),
            protocol_id: 1,
        },
        ItemReport {
            id: cooked_food.clone(),
            protocol_id: 2,
        },
    ]);
    let recipe = Recipe {
        id: Identifier::parse("minecraft:test_food").unwrap(),
        kind: RecipeKind::Smelting(SmeltingRecipe {
            ingredient: Ingredient {
                alternatives: vec![IngredientAlternative::Item(raw_food.clone())],
            },
            cooking_time: 200,
            experience_milli: 0,
        }),
        result: RecipeResult {
            item: cooked_food,
            count: 1,
        },
    };
    let mut furnace = FurnaceBlockEntity {
        burn_remaining: 0,
        burn_total: 0,
        cook_progress: 50,
        cook_total: 200,
        ..FurnaceBlockEntity::default()
    };
    furnace.slots[0] = stack_to_furnace_slot(&ItemStack::new(items.id_of(&raw_food).unwrap(), 1));

    let _ = tick_furnace(
        std::slice::from_ref(&recipe),
        &items,
        &mc_data::tags::TagsData::default(),
        &mut furnace,
        FurnaceKind::Furnace,
    );

    assert_eq!(furnace.cook_progress, 48);
}

#[test]
fn completed_furnace_cook_records_the_recipe_for_experience() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let raw_iron = Identifier::parse("minecraft:raw_iron").unwrap();
    let iron_ingot = Identifier::parse("minecraft:iron_ingot").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: raw_iron.clone(),
            protocol_id: 1,
        },
        ItemReport {
            id: iron_ingot.clone(),
            protocol_id: 2,
        },
    ]);
    let recipe = Recipe {
        id: Identifier::parse("minecraft:iron_ingot_from_smelting_raw_iron").unwrap(),
        kind: RecipeKind::Smelting(SmeltingRecipe {
            ingredient: Ingredient {
                alternatives: vec![IngredientAlternative::Item(raw_iron)],
            },
            cooking_time: 200,
            experience_milli: 700,
        }),
        result: RecipeResult {
            item: iron_ingot,
            count: 1,
        },
    };
    let mut furnace = FurnaceBlockEntity {
        burn_remaining: 2,
        cook_progress: 199,
        ..FurnaceBlockEntity::default()
    };
    furnace.slots[0] = stack_to_furnace_slot(&ItemStack::new(1, 1));

    let _ = tick_furnace(
        std::slice::from_ref(&recipe),
        &items,
        &TagsData::default(),
        &mut furnace,
        FurnaceKind::Furnace,
    );

    assert_eq!(
        furnace
            .recipes_used
            .get("minecraft:iron_ingot_from_smelting_raw_iron"),
        Some(&1)
    );
}

#[test]
fn taking_furnace_output_awards_only_recorded_furnace_recipe_experience() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let ingredient = Ingredient {
        alternatives: vec![IngredientAlternative::Item(
            Identifier::parse("minecraft:raw_iron").unwrap(),
        )],
    };
    let result = RecipeResult {
        item: Identifier::parse("minecraft:iron_ingot").unwrap(),
        count: 1,
    };
    let furnace_recipe = Recipe {
        id: Identifier::parse("minecraft:test_furnace").unwrap(),
        kind: RecipeKind::Smelting(SmeltingRecipe {
            ingredient: ingredient.clone(),
            cooking_time: 200,
            experience_milli: 1_000,
        }),
        result: result.clone(),
    };
    let campfire_recipe = Recipe {
        id: Identifier::parse("minecraft:test_campfire").unwrap(),
        kind: RecipeKind::CampfireCooking(SmeltingRecipe {
            ingredient,
            cooking_time: 600,
            experience_milli: 1_000,
        }),
        result,
    };
    let recipes_used = BTreeMap::from([
        ("minecraft:test_furnace".to_string(), 2),
        ("minecraft:test_campfire".to_string(), 5),
    ]);
    let mut before = FurnaceBlockEntity::default();
    before.slots[2] = stack_to_furnace_slot(&ItemStack::new(2, 3));
    let mut after = before.clone();
    after.slots[2].count = 2;

    assert!(furnace_output_was_taken(&before, &after));
    assert_eq!(
        furnace_experience_award(&[furnace_recipe, campfire_recipe], &recipes_used, 0),
        2
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_furnace_tick_does_not_wait_for_world_writer() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:furnace"),
        ])
        .unwrap(),
    );
    let storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let mut state = interaction_state_for_items(Arc::new(ItemRegistry::default()));
    state.blocks = Arc::clone(&blocks);
    state.world = Arc::clone(&world);
    state.world_read = world_read;
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = world.lock().await;
        let cpos = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage.set_block_at(position, BlockStateId(1)).unwrap();
        storage
            .set_furnace_block_entity(position, FurnaceBlockEntity::default())
            .unwrap();
    }

    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(69),
        name: "IdleFurnace".to_string(),
    };
    let (tx, _rx) = mpsc::channel(1);
    let (session_id, _) = state.sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 65.0, 0.5),
    );
    state.sessions.mark_loaded(session_id, (0, 0));

    let config = ServerConfig {
        blocks: Arc::clone(&state.blocks),
        world: Some(Arc::clone(&world)),
        tags: Arc::clone(&state.tags),
        recipes: Arc::new(state.recipes.clone()),
        items: Arc::clone(&state.items),
        block_facts: Arc::clone(&state.block_facts),
        ..play_loop_slow_client_test_config()
    };
    let world_guard = world.lock().await;
    let (_simulation, owner) = simulation_channel();
    let mut tick = Box::pin(owner.run_furnace_ticks(
        &config,
        &state.sessions,
        Some(&state.world_read),
        Some(&world_mutation),
    ));
    std::future::poll_fn(|cx| match std::future::Future::poll(tick.as_mut(), cx) {
        Poll::Ready(updated) => {
            assert_eq!(updated, 0);
            Poll::Ready(())
        }
        Poll::Pending => panic!("idle furnace tick waited for the world writer"),
    })
    .await;
    drop(world_guard);
}

#[tokio::test]
async fn active_furnace_tick_updates_resident_state_without_world_writer() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            furnace_block(1, 2),
        ])
        .unwrap(),
    );
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let cpos = ChunkPos { x: 0, z: 0 };
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(position, BlockStateId(1)).unwrap();
    storage
        .set_furnace_block_entity(
            position,
            FurnaceBlockEntity {
                burn_remaining: 10,
                burn_total: 10,
                ..FurnaceBlockEntity::default()
            },
        )
        .unwrap();
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(74),
        name: "DurableFurnace".to_string(),
    };
    let (tx, _rx) = mpsc::channel(1);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 65.0, 0.5),
    );
    sessions.mark_loaded(session_id, (0, 0));
    let config = ServerConfig {
        blocks: Arc::clone(&blocks),
        world: Some(Arc::clone(&world)),
        ..play_loop_slow_client_test_config()
    };
    let (_simulation, owner) = simulation_channel();
    let world_writer = world.lock().await;
    let updated = tokio::time::timeout(
        Duration::from_secs(1),
        owner.run_furnace_ticks(&config, &sessions, Some(&world_read), Some(&world_mutation)),
    )
    .await
    .expect("resident furnace tick completion event");
    assert_eq!(updated, 1);
    assert_eq!(world_read.get_cached_block(position), Some(BlockStateId(2)));
    assert_eq!(
        world_read
            .furnace_snapshots(&[cpos])
            .into_iter()
            .find(|(candidate, _)| *candidate == position)
            .expect("resident furnace")
            .1
            .burn_remaining,
        9
    );
    drop(world_writer);
}

#[tokio::test]
async fn active_furnace_tick_publishes_lit_block_and_light() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            furnace_block(1, 2),
        ])
        .unwrap(),
    );
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let cpos = ChunkPos { x: 0, z: 0 };
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(position, BlockStateId(1)).unwrap();
    storage
        .set_baked_light(cpos, &mc_world::light::ChunkLight::filled(15, 0))
        .unwrap();
    storage
        .set_furnace_block_entity(
            position,
            FurnaceBlockEntity {
                burn_remaining: 10,
                burn_total: 10,
                ..FurnaceBlockEntity::default()
            },
        )
        .unwrap();
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(75),
        name: "LitFurnace".to_string(),
    };
    let (tx, mut rx) = mpsc::channel(8);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 65.0, 0.5),
    );
    sessions.mark_loaded(session_id, (0, 0));
    let config = ServerConfig {
        blocks: Arc::clone(&blocks),
        block_light: Some(Arc::new(
            mc_data::block_light::BlockLightTable::from_arrays(
                "furnace lit test",
                vec![0, 0, 13],
                vec![0, 15, 15],
                vec![true, false, false],
            ),
        )),
        world: Some(world),
        ..play_loop_slow_client_test_config()
    };
    let (_simulation, owner) = simulation_channel();

    assert_eq!(
        owner
            .run_furnace_ticks(&config, &sessions, Some(&world_read), Some(&world_mutation))
            .await,
        1
    );
    let commands = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(commands.iter().any(|command| matches!(
        command,
        OutboundCommand::BlockDeltas(deltas)
            if deltas == &[BlockDelta {
                x: position.x,
                y: position.y,
                z: position.z,
                state_id: BlockStateId(2),
            }]
    )));
    assert!(commands.iter().any(|command| matches!(
        command,
        OutboundCommand::LightUpdates(updates)
            if updates.iter().any(|update| update.pos == cpos)
    )));
}

#[test]
fn furnace_tick_block_state_tracks_burning_state() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        furnace_block(1, 2),
    ])
    .unwrap();
    let burning = FurnaceBlockEntity {
        burn_remaining: 1,
        ..FurnaceBlockEntity::default()
    };

    assert_eq!(
        furnace_tick_block_state(&blocks, BlockStateId(1), &burning),
        BlockStateId(2)
    );
    assert_eq!(
        furnace_tick_block_state(&blocks, BlockStateId(2), &FurnaceBlockEntity::default()),
        BlockStateId(1)
    );
}

#[tokio::test]
async fn stale_furnace_tick_wave_replans_against_resident_state() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:furnace"),
        ])
        .unwrap(),
    );
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let cpos = ChunkPos { x: 0, z: 0 };
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(position, BlockStateId(1)).unwrap();
    let current = FurnaceBlockEntity {
        burn_remaining: 9,
        burn_total: 10,
        ..FurnaceBlockEntity::default()
    };
    storage.set_furnace_block_entity(position, current).unwrap();
    let world_read = storage.read_view();
    let mutation = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        blocks: Arc::clone(&blocks),
        world: Some(world),
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let stale_before = FurnaceBlockEntity {
        burn_remaining: 10,
        burn_total: 10,
        ..FurnaceBlockEntity::default()
    };
    let mut stale_after = stale_before.clone();
    stale_after.burn_remaining = 9;

    let updates = commit_resident_furnace_tick_wave(
        &config,
        &sessions,
        &mutation,
        vec![FurnaceTickPlan {
            position,
            block_state: BlockStateId(1),
            after_block_state: BlockStateId(1),
            kind: FurnaceKind::Furnace,
            before: stale_before,
            after: stale_after,
            slots_changed: false,
            data_changed: vec![(0, 9)],
        }],
    );
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].1.burn_remaining, 8);
    assert_eq!(
        world_read
            .furnace_snapshots(&[cpos])
            .into_iter()
            .find(|(candidate, _)| *candidate == position)
            .expect("replanned resident furnace")
            .1
            .burn_remaining,
        8
    );
}

#[test]
fn active_furnace_tick_releases_world_writer_between_commits() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:furnace"),
        ])
        .unwrap(),
    );
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let cpos = ChunkPos { x: 0, z: 0 };
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    for position in [
        mc_world::BlockPos { x: 1, y: 64, z: 1 },
        mc_world::BlockPos { x: 2, y: 64, z: 1 },
    ] {
        storage.set_block_at(position, BlockStateId(1)).unwrap();
        storage
            .set_furnace_block_entity(
                position,
                FurnaceBlockEntity {
                    burn_remaining: 10,
                    burn_total: 10,
                    ..FurnaceBlockEntity::default()
                },
            )
            .unwrap();
    }
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));

    let mut state = interaction_state_for_items(Arc::new(ItemRegistry::default()));
    state.blocks = Arc::clone(&blocks);
    state.world = Arc::clone(&world);
    state.world_read = world_read.clone();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(73),
        name: "FurnaceWriterBoundary".to_string(),
    };
    let (tx, _rx) = mpsc::channel(1);
    let (session_id, _) = state.sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 65.0, 0.5),
    );
    state.sessions.mark_loaded(session_id, (0, 0));

    let config = ServerConfig {
        blocks: Arc::clone(&state.blocks),
        world: Some(Arc::clone(&world)),
        tags: Arc::clone(&state.tags),
        recipes: Arc::new(state.recipes.clone()),
        items: Arc::clone(&state.items),
        block_facts: Arc::clone(&state.block_facts),
        ..play_loop_slow_client_test_config()
    };
    let sessions = Arc::clone(&state.sessions);
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    sessions.install_server_furnace_commit_probe(reached_tx, resume_rx);

    let tick_sessions = Arc::clone(&sessions);
    let tick_thread = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                let (_simulation, owner) = simulation_channel();
                owner
                    .run_furnace_ticks(
                        &config,
                        &tick_sessions,
                        Some(&world_read),
                        Some(&world_mutation),
                    )
                    .await
            })
    });

    reached_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("first furnace commit reaches the exact probe");
    let writer_is_available = world.try_lock().is_ok();
    resume_tx
        .send(())
        .expect("release the exact furnace commit probe");
    assert_eq!(tick_thread.join().expect("furnace tick thread"), 2);
    assert!(
        writer_is_available,
        "furnace tick must release the world writer after each independent commit"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_furnace_tick_pushes_to_all_viewers_without_losing_click() {
    let coal = Identifier::parse("minecraft:coal").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: coal,
        protocol_id: 10,
    }]));
    let coal_id = items
        .id_of(&Identifier::parse("minecraft:coal").unwrap())
        .unwrap();
    let mut ticker = interaction_state_for_items(Arc::clone(&items));
    let mut clicker = interaction_state_for_items(items);
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:furnace"),
        ])
        .unwrap(),
    );
    let storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    ticker.blocks = Arc::clone(&blocks);
    ticker.world = Arc::clone(&world);
    ticker.world_read = world_read.clone();
    clicker.blocks = blocks;
    clicker.world = world;
    clicker.world_read = world_read;
    clicker.sessions = Arc::clone(&ticker.sessions);
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = ticker.world.lock().await;
        let cpos = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let furnace = FurnaceBlockEntity {
            burn_remaining: 10,
            burn_total: 10,
            ..FurnaceBlockEntity::default()
        };
        storage.set_block_at(position, BlockStateId(1)).unwrap();
        storage.set_furnace_block_entity(position, furnace).unwrap();
    }

    let ticker_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(70),
        name: "FurnaceTicker".to_string(),
    };
    let clicker_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(71),
        name: "FurnaceClicker".to_string(),
    };
    let (ticker_tx, mut ticker_rx) = mpsc::channel(8);
    let (clicker_tx, mut clicker_rx) = mpsc::channel(8);
    let (ticker_id, _) = ticker.sessions.register(
        &ticker_profile,
        (0, 0),
        0,
        HashSet::new(),
        ticker_tx,
        PlayerPose::new(0.5, 65.0, 0.5),
    );
    let (clicker_id, _) = ticker.sessions.register(
        &clicker_profile,
        (0, 0),
        0,
        HashSet::new(),
        clicker_tx,
        PlayerPose::new(0.5, 65.0, 0.5),
    );
    ticker.session_id = ticker_id;
    clicker.session_id = clicker_id;
    ticker.sessions.mark_loaded(ticker_id, (0, 0));
    ticker.sessions.mark_loaded(clicker_id, (0, 0));
    assert_eq!(
        ticker.sessions.register_furnace_viewer(ticker_id, position),
        1
    );
    assert_eq!(
        ticker
            .sessions
            .register_furnace_viewer(clicker_id, position),
        1
    );
    ticker.active_container = Some(ActiveContainer::Furnace(FurnaceWindow::new(
        position,
        7,
        FurnaceKind::Furnace,
    )));
    clicker.carried_item = ItemStack::new(coal_id, 1);

    let config = ServerConfig {
        blocks: Arc::clone(&ticker.blocks),
        world: Some(Arc::clone(&ticker.world)),
        tags: Arc::clone(&ticker.tags),
        recipes: Arc::new(ticker.recipes.clone()),
        items: Arc::clone(&ticker.items),
        block_facts: Arc::clone(&ticker.block_facts),
        ..play_loop_slow_client_test_config()
    };

    let shared_world = Arc::clone(&ticker.world);
    let world_guard = shared_world.lock().await;
    let (_simulation, owner) = simulation_channel();
    let mut tick = Box::pin(owner.run_furnace_ticks(
        &config,
        &ticker.sessions,
        Some(&ticker.world_read),
        Some(&world_mutation),
    ));
    let tick_result =
        std::future::poll_fn(|cx| match std::future::Future::poll(tick.as_mut(), cx) {
            Poll::Ready(updated) => Poll::Ready(updated),
            Poll::Pending => panic!("active furnace tick waited for the world writer"),
        })
        .await;
    assert_eq!(tick_result, 1);
    drop(world_guard);

    let mut clicker_writer = Vec::new();
    let clicker_window = handle_furnace_container_click(
        &mut clicker,
        &mut clicker_writer,
        FurnaceWindow::new(position, 8, FurnaceKind::Furnace),
        PlayerPose::new(0.5, 65.0, 0.5),
        ServerboundContainerClick {
            container_id: 8,
            state_id: 1,
            slot_num: 1,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .expect("furnace click succeeds");
    assert_eq!(clicker_window.state_id, 2);
    assert!(clicker.carried_item.is_empty());
    let ticker_commands = std::iter::from_fn(|| ticker_rx.try_recv().ok()).collect::<Vec<_>>();
    let clicker_commands = std::iter::from_fn(|| clicker_rx.try_recv().ok()).collect::<Vec<_>>();
    for commands in [&ticker_commands, &clicker_commands] {
        assert!(commands.iter().any(|command| matches!(
            command,
            OutboundCommand::FurnaceData { position: update_position, changed }
                if *update_position == position && changed.contains(&(0, 9))
        )));
    }
    let mut storage = ticker.world.lock().await;
    let furnace = storage
        .furnace_block_entity(position)
        .unwrap()
        .expect("furnace remains present");
    assert_eq!(furnace.burn_remaining, 9);
    assert_eq!(
        furnace_slot_to_stack(&furnace.slots[1]),
        ItemStack::new(coal_id, 1),
        "tick data update must not overwrite the queued client slot mutation"
    );
}

#[tokio::test]
async fn owner_furnace_tick_keeps_running_after_window_closes() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:furnace"),
        ])
        .unwrap(),
    );
    let storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let mut state = interaction_state_for_items(Arc::new(ItemRegistry::default()));
    state.blocks = Arc::clone(&blocks);
    state.world = Arc::clone(&world);
    state.world_read = world_read;
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = world.lock().await;
        let cpos = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage.set_block_at(position, BlockStateId(1)).unwrap();
        storage
            .set_furnace_block_entity(
                position,
                FurnaceBlockEntity {
                    burn_remaining: 10,
                    burn_total: 10,
                    ..FurnaceBlockEntity::default()
                },
            )
            .unwrap();
    }

    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(72),
        name: "FurnaceOwner".to_string(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state.sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 65.0, 0.5),
    );
    state.sessions.mark_loaded(session_id, (0, 0));

    let config = ServerConfig {
        blocks: Arc::clone(&state.blocks),
        world: Some(Arc::clone(&world)),
        tags: Arc::clone(&state.tags),
        recipes: Arc::new(state.recipes.clone()),
        items: Arc::clone(&state.items),
        block_facts: Arc::clone(&state.block_facts),
        ..play_loop_slow_client_test_config()
    };
    let (_simulation, owner) = simulation_channel();

    assert_eq!(
        owner
            .run_furnace_ticks(
                &config,
                &state.sessions,
                Some(&state.world_read),
                Some(&world_mutation),
            )
            .await,
        1
    );
    let mut storage = world.lock().await;
    let furnace = storage
        .furnace_block_entity(position)
        .unwrap()
        .expect("furnace remains present");
    assert_eq!(furnace.burn_remaining, 9);
}

#[tokio::test]
async fn stale_furnace_click_after_peer_mutation_resyncs_without_mutating_storage() {
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let stone = Identifier::parse("minecraft:stone").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: dirt,
            protocol_id: 10,
        },
        ItemReport {
            id: stone,
            protocol_id: 11,
        },
    ]));
    let mut state = interaction_state_for_items(Arc::clone(&items));
    let dirt_id = items
        .id_of(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    let stone_id = items
        .id_of(&Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = state.world.lock().await;
        let cpos = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let mut furnace = FurnaceBlockEntity::default();
        furnace.slots[0] = stack_to_furnace_slot(&ItemStack::new(dirt_id, 5));
        storage.set_furnace_block_entity(position, furnace).unwrap();
    }

    let window = FurnaceWindow::new(position, 7, FurnaceKind::Furnace);
    {
        let mut storage = state.world.lock().await;
        let mut furnace = storage
            .furnace_block_entity(position)
            .unwrap()
            .expect("test furnace exists");
        furnace.slots[0] = stack_to_furnace_slot(&ItemStack::new(stone_id, 2));
        storage
            .set_furnace_block_entity(position, furnace.clone())
            .unwrap();
        let _ = state.sessions.server_furnace_slot_dispatches_except(
            position,
            99,
            furnace_slot_stacks(&furnace),
        );
    }

    let mut writer = Vec::new();
    let returned = handle_furnace_container_click(
        &mut state,
        &mut writer,
        window,
        PlayerPose::new(0.5, 65.0, 0.5),
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();

    assert_eq!(returned.state_id, 2);
    assert!(state.carried_item.is_empty());
    {
        let mut storage = state.world.lock().await;
        let furnace = storage
            .furnace_block_entity(position)
            .unwrap()
            .expect("test furnace exists");
        assert_eq!(
            furnace_slot_to_stack(&furnace.slots[0]),
            ItemStack::new(stone_id, 2)
        );
    }
    let packets = decode_container_set_content_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].state_id, 2);
    assert_eq!(packets[0].items[0], ItemStack::new(stone_id, 2));
    assert!(packets[0].carried_item.is_empty());
}

#[test]
fn chest_window_swap_and_throw_mutate_storage_slots() {
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let stone = Identifier::parse("minecraft:stone").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: dirt,
            protocol_id: 10,
        },
        ItemReport {
            id: stone,
            protocol_id: 11,
        },
    ]));
    let mut state = interaction_state_for_items(Arc::clone(&items));
    let dirt_id = items
        .id_of(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    let stone_id = items
        .id_of(&Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(stone_id, 2);
    let mut view = ChestView {
        chests: vec![ChestBlockEntity::default()],
    };
    view.chests[0].slots[0] = stack_to_furnace_slot(&ItemStack::new(dirt_id, 5));

    assert!(apply_chest_swap_click(&mut state, &mut view, 0, 0));
    assert_eq!(
        furnace_slot_to_stack(&view.chests[0].slots[0]),
        ItemStack::new(stone_id, 2)
    );
    assert_eq!(
        state.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(dirt_id, 5)
    );

    let dropped = apply_chest_throw_click(&mut state, &mut view, 0, 0).unwrap();
    assert_eq!(dropped, ItemStack::new(stone_id, 1));
    assert_eq!(
        furnace_slot_to_stack(&view.chests[0].slots[0]),
        ItemStack::new(stone_id, 1)
    );
}

#[tokio::test]
async fn stale_chest_click_after_peer_mutation_resyncs_without_mutating_storage() {
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let stone = Identifier::parse("minecraft:stone").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: dirt,
            protocol_id: 10,
        },
        ItemReport {
            id: stone,
            protocol_id: 11,
        },
    ]));
    let mut state = interaction_state_for_items(Arc::clone(&items));
    let dirt_id = items
        .id_of(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    let stone_id = items
        .id_of(&Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = state.world.lock().await;
        let cpos = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let mut chest = ChestBlockEntity::default();
        chest.slots[0] = stack_to_furnace_slot(&ItemStack::new(dirt_id, 5));
        storage.set_chest_block_entity(position, chest).unwrap();
    }

    let window = ChestWindow::new(vec![position], 7);
    {
        let mut storage = state.world.lock().await;
        let mut chest = storage
            .chest_block_entity(position)
            .unwrap()
            .expect("test chest exists");
        chest.slots[0] = stack_to_furnace_slot(&ItemStack::new(stone_id, 2));
        storage.set_chest_block_entity(position, chest).unwrap();
    }
    let _ = state
        .sessions
        .try_chest_slot_dispatches(position, 1, 1, 99, vec![ItemStack::new(stone_id, 2)])
        .expect("peer mutation claims initial chest state");

    let mut writer = Vec::new();
    let returned = handle_chest_container_click(
        &mut state,
        &mut writer,
        window,
        PlayerPose::new(0.5, 65.0, 0.5),
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();

    assert_eq!(returned.state_id, 2);
    assert!(state.carried_item.is_empty());
    {
        let mut storage = state.world.lock().await;
        let chest = storage
            .chest_block_entity(position)
            .unwrap()
            .expect("test chest exists");
        assert_eq!(
            furnace_slot_to_stack(&chest.slots[0]),
            ItemStack::new(stone_id, 2)
        );
    }
    let packets = decode_container_set_content_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].state_id, 2);
    assert_eq!(packets[0].items[0], ItemStack::new(stone_id, 2));
    assert!(packets[0].carried_item.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chest_commit_snapshot_pairs_world_contents_with_viewer_state_id() {
    let state = interaction_state_for_items(Arc::new(ItemRegistry::from_report(&[])));
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let mut initial = ChestBlockEntity::default();
    initial.slots[0] = FurnaceSlot {
        item_id: 10,
        count: 2,
        damage: None,
        enchantments: Vec::new(),
    };
    {
        let mut storage = state.world.lock().await;
        let cpos = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage
            .set_chest_block_entity(position, initial.clone())
            .unwrap();
    }

    let world = Arc::clone(&state.world);
    let sessions = Arc::clone(&state.sessions);
    let mut guard = world.lock().await;
    let window = ChestWindow::new(vec![position], 7);
    let mut snapshot = Box::pin(load_chest_commit_snapshot(&state, &window));
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(snapshot.as_mut(), cx).is_pending(),
            "snapshot must wait for the held world lock"
        );
        std::task::Poll::Ready(())
    })
    .await;
    let mut updated = initial;
    updated.slots[0].count = 1;
    guard
        .set_chest_block_entity(position, updated.clone())
        .unwrap();
    let (state_id, _) = sessions
        .try_chest_slot_dispatches(position, 1, 1, 99, vec![ItemStack::new(10, 1)])
        .unwrap();
    assert_eq!(state_id, 2);
    drop(guard);

    let (view, observed_state_id) = snapshot.await.unwrap();
    assert_eq!(observed_state_id, 2);
    assert_eq!(view.chests, vec![updated]);
}

#[tokio::test]
async fn shared_chest_same_version_click_commits_once_and_conserves_items() {
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: dirt,
        protocol_id: 10,
    }]));
    let dirt_id = items
        .id_of(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    let mut actor = interaction_state_for_items(Arc::clone(&items));
    let mut observer = interaction_state_for_items(items);
    observer.world = Arc::clone(&actor.world);
    observer.sessions = Arc::clone(&actor.sessions);
    observer.session_id = 2;

    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = actor.world.lock().await;
        let cpos = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let mut chest = ChestBlockEntity::default();
        chest.slots[0] = stack_to_furnace_slot(&ItemStack::new(dirt_id, 2));
        storage.set_chest_block_entity(position, chest).unwrap();
    }

    let click = |container_id| ServerboundContainerClick {
        container_id,
        state_id: 1,
        slot_num: 0,
        button_num: 1,
        container_input: ContainerInput::Pickup,
        changed_slots: Vec::new(),
        carried_item: mc_protocol::packets::play::HashedStack::Actual {
            item_id: dirt_id,
            count: 1,
            components: mc_protocol::packets::play::HashedStackComponentHashes::empty(),
        },
    };
    let mut actor_writer = Vec::new();
    let actor_window = handle_chest_container_click(
        &mut actor,
        &mut actor_writer,
        ChestWindow::new(vec![position], 7),
        PlayerPose::new(0.5, 65.0, 0.5),
        click(7),
    )
    .await
    .unwrap();
    let mut observer_writer = Vec::new();
    let observer_window = handle_chest_container_click(
        &mut observer,
        &mut observer_writer,
        ChestWindow::new(vec![position], 8),
        PlayerPose::new(0.5, 65.0, 0.5),
        click(8),
    )
    .await
    .unwrap();

    assert_eq!(actor_window.state_id, 3);
    assert_eq!(observer_window.state_id, 3);
    assert_eq!(actor.carried_item, ItemStack::new(dirt_id, 1));
    assert!(observer.carried_item.is_empty());
    let chest_count = {
        let mut storage = actor.world.lock().await;
        let chest = storage
            .chest_block_entity(position)
            .unwrap()
            .expect("shared chest exists");
        assert_eq!(chest.slots[0].item_id, dirt_id);
        chest.slots[0].count
    };
    assert_eq!(chest_count, 1);
    assert_eq!(
        chest_count + actor.carried_item.count + observer.carried_item.count,
        2,
        "the rejected stale click must neither duplicate nor delete the shared stack"
    );
    let observer_packets = decode_container_set_content_packets(&observer_writer);
    assert_eq!(observer_packets.len(), 1);
    assert_eq!(observer_packets[0].state_id, 3);
    assert_eq!(observer_packets[0].items[0], ItemStack::new(dirt_id, 1));
    assert!(observer_packets[0].carried_item.is_empty());
}

fn spawn_test_simulation_owner(
    sessions: Arc<SessionRegistry>,
) -> (
    SimulationHandle,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let (simulation, mut owner) = simulation_channel();
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = &mut stop_rx => {
                    owner.shutdown();
                    break;
                }
                ready = owner.wait_for_command() => {
                    if !ready {
                        break;
                    }
                    owner.process_tick(&sessions, SIMULATION_COMMAND_BATCH_LIMIT);
                }
            }
        }
    });
    (simulation, stop_tx, task)
}

fn start_survival_test_owner(
    state: &mut InteractionState,
    name: &str,
    survival: SurvivalState,
    xp: &XpState,
) -> (
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let (session_id, _) = register_survival_test_player(state, name, survival, xp);
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    state.simulation = simulation.for_session(session_id);
    (stop, task)
}

fn register_survival_test_player(
    state: &mut InteractionState,
    name: &str,
    survival: SurvivalState,
    xp: &XpState,
) -> (SessionId, Arc<Mutex<PlayerPersistedState>>) {
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid(name),
        name: name.to_owned(),
    };
    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let (outbound_tx, _outbound_rx) = mpsc::channel(8);
    let (session_id, _) =
        state
            .sessions
            .register(&profile, (0, 0), 0, HashSet::new(), outbound_tx, pose);
    let mut persisted = PlayerPersistedState::new_default(pose);
    persisted.survival = survival;
    persisted.inventory = state.inventory.clone();
    persisted.carried_item = state.carried_item.clone();
    persisted.selected_hotbar_slot = state.selected_hotbar_slot;
    persisted.xp = xp.clone();
    let persisted = Arc::new(Mutex::new(persisted));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&persisted));
    state.sessions.set_active_shield(
        session_id,
        state.shield_use.as_ref().map(|shield| ActiveShield {
            started_tick: shield.started_tick,
            slot: shield.slot,
            expected_stack: shield.stack.clone(),
        }),
    );
    state.session_id = session_id;
    state.entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    state.sessions.configure_arrow_kill_rewards(
        item_entity_type_id(&state.entity_types),
        xp_orb_entity_type_id(&state.entity_types),
        arrow_entity_type_id(&state.entity_types),
        Arc::clone(&state.items),
        Arc::clone(&state.item_facts),
        Arc::clone(&state.loot),
    );
    state.sessions.configure_player_combat(
        item_entity_type_id(&state.entity_types),
        xp_orb_entity_type_id(&state.entity_types),
        Arc::clone(&state.items),
        Arc::clone(&state.item_facts),
    );
    (session_id, persisted)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_pickup_tasks_conserve_item_and_xp_entities() {
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: dirt,
        protocol_id: 10,
    }]));
    let dirt_id = items
        .id_of(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    let mut alice = interaction_state_for_items(Arc::clone(&items));
    let mut bob = interaction_state_for_items(items);
    bob.sessions = Arc::clone(&alice.sessions);
    let (simulation, simulation_stop_tx, simulation_task) =
        spawn_test_simulation_owner(Arc::clone(&alice.sessions));
    let simulation_probe = simulation.clone();
    let alice_id = register_interaction_player(&mut alice, "PickupTaskAlice");
    let bob_id = register_interaction_player(&mut bob, "PickupTaskBob");
    alice.simulation = simulation.for_session(alice_id);
    bob.simulation = simulation.for_session(bob_id);
    alice.sessions.spawn_item_drop(
        1,
        Vec3::new(0.5, 64.0, 0.5),
        EntityItemStack::new(dirt_id, 3),
    );
    alice.sessions.advance_world_time(ITEM_PICKUP_DELAY_TICKS);

    let item_gate = Arc::new(tokio::sync::Barrier::new(3));
    let alice_item_task = {
        let gate = Arc::clone(&item_gate);
        tokio::spawn(async move {
            let mut writer = Vec::new();
            gate.wait().await;
            pickup_nearby_items(&mut alice, &mut writer, PlayerPose::new(0.5, 64.0, 0.5))
                .await
                .expect("Alice item pickup task succeeds");
            alice
        })
    };
    let bob_item_task = {
        let gate = Arc::clone(&item_gate);
        tokio::spawn(async move {
            let mut writer = Vec::new();
            gate.wait().await;
            pickup_nearby_items(&mut bob, &mut writer, PlayerPose::new(0.5, 64.0, 0.5))
                .await
                .expect("Bob item pickup task succeeds");
            bob
        })
    };
    item_gate.wait().await;
    let mut alice = alice_item_task.await.expect("Alice item task joins");
    let mut bob = bob_item_task.await.expect("Bob item task joins");
    let inventory_item_count = |state: &InteractionState| {
        state
            .inventory
            .slots
            .iter()
            .filter(|stack| stack.item_id == dirt_id)
            .map(|stack| stack.count)
            .sum::<i32>()
    };
    assert_eq!(inventory_item_count(&alice) + inventory_item_count(&bob), 3);
    assert!(
        alice
            .sessions
            .nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
            .is_empty()
    );

    alice.sessions.spawn_xp_orb(2, Vec3::new(0.5, 64.0, 0.5), 5);
    let xp_gate = Arc::new(tokio::sync::Barrier::new(3));
    let alice_xp_task = {
        let gate = Arc::clone(&xp_gate);
        tokio::spawn(async move {
            let mut writer = Vec::new();
            let mut xp = XpState::default();
            gate.wait().await;
            pickup_nearby_xp(
                &mut alice,
                &mut writer,
                &mut xp,
                PlayerPose::new(0.5, 64.0, 0.5),
            )
            .await
            .expect("Alice XP pickup task succeeds");
            (alice, xp)
        })
    };
    let bob_xp_task = {
        let gate = Arc::clone(&xp_gate);
        tokio::spawn(async move {
            let mut writer = Vec::new();
            let mut xp = XpState::default();
            gate.wait().await;
            pickup_nearby_xp(
                &mut bob,
                &mut writer,
                &mut xp,
                PlayerPose::new(0.5, 64.0, 0.5),
            )
            .await
            .expect("Bob XP pickup task succeeds");
            (bob, xp)
        })
    };
    xp_gate.wait().await;
    let (alice, alice_xp) = alice_xp_task.await.expect("Alice XP task joins");
    let (_bob, bob_xp) = bob_xp_task.await.expect("Bob XP task joins");
    let _ = simulation_stop_tx.send(());
    simulation_task.await.expect("simulation owner joins");
    assert_eq!(alice_xp.total + bob_xp.total, 5);
    assert!(
        alice
            .sessions
            .nearby_experience_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
            .is_empty()
    );
    let simulation_snapshot = simulation_probe.snapshot();
    assert!(simulation_snapshot.enqueued >= 2);
    assert_eq!(simulation_snapshot.enqueued, simulation_snapshot.processed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grounded_arrow_pickup_credits_owner_inventory_and_writes_slot() {
    let arrow = Identifier::parse("minecraft:arrow").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: arrow.clone(),
        protocol_id: 10,
    }]));
    let arrow_item_id = items.id_of(&arrow).unwrap();
    let mut state = interaction_state_for_items(items);
    state.sessions.spawn_arrow_for_test(
        None,
        3,
        Vec3::new(1.5, 64.0, 0.5),
        Vec3::new(0.0, 0.0, 1.0),
        Rotation::ZERO,
    );
    let arrow_entity_id = state
        .sessions
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:arrow")
        .unwrap()
        .snapshot
        .id;
    state.sessions.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: arrow_entity_id,
            position: Vec3::new(1.5, 64.0, 0.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
    );
    let session_id = register_interaction_player(&mut state, "ArrowPickupConnection");
    let (simulation, stop_tx, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    state.simulation = simulation.for_session(session_id);

    let mut writer = Vec::new();
    pickup_nearby_arrows(&mut state, &mut writer, PlayerPose::new(0.5, 64.0, 0.5))
        .await
        .unwrap();
    let _ = stop_tx.send(());
    task.await.unwrap();

    assert_eq!(
        state.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(arrow_item_id, 1)
    );
    assert!(
        state
            .sessions
            .server_entity_snapshot(arrow_entity_id)
            .is_none()
    );
    let packets = decode_container_set_slot_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].slot, PlayerInventory::HOTBAR_BASE as i16);
    assert_eq!(packets[0].item_stack, ItemStack::new(arrow_item_id, 1));
}

#[tokio::test]
async fn full_simulation_queue_leaves_item_pickup_state_unchanged() {
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: dirt,
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(Arc::clone(&items));
    let dirt_id = items
        .id_of(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    register_interaction_player(&mut state, "FullQueuePickup");
    let (simulation, simulation_owner) = simulation::simulation_channel_with_capacity(1);
    let _blocked = simulation
        .enqueue(simulation::SimulationCommand::ClaimExperiencePickup {
            entity_id: EntityId(999),
            collector_session: 1,
        })
        .unwrap();
    state.simulation = simulation.for_session(state.session_id);
    state.sessions.spawn_item_drop(
        1,
        Vec3::new(0.5, 64.0, 0.5),
        EntityItemStack::new(dirt_id, 3),
    );
    state.sessions.advance_world_time(ITEM_PICKUP_DELAY_TICKS);

    let mut writer = Vec::new();
    pickup_nearby_items(&mut state, &mut writer, PlayerPose::new(0.5, 64.0, 0.5))
        .await
        .expect("queue pressure is a fail-closed no-pickup");

    assert_eq!(
        state
            .inventory
            .slots
            .iter()
            .map(|stack| stack.count)
            .sum::<i32>(),
        0
    );
    assert_eq!(
        state
            .sessions
            .nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)[0]
            .item_stack
            .as_ref()
            .unwrap()
            .count,
        3
    );
    assert_eq!(simulation.snapshot().rejected_full, 1);
    drop(simulation_owner);
}

fn assert_attack_damage_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.000_01,
        "expected attack damage {expected}, got {actual}"
    );
}

fn attack_strength_test_state() -> (InteractionState, u32, u32) {
    let sword_name = Identifier::parse("minecraft:stone_sword").unwrap();
    let axe_name = Identifier::parse("minecraft:stone_axe").unwrap();
    let shield_name = Identifier::parse("minecraft:shield").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: sword_name.clone(),
            protocol_id: 10,
        },
        ItemReport {
            id: axe_name.clone(),
            protocol_id: 11,
        },
        ItemReport {
            id: shield_name.clone(),
            protocol_id: 12,
        },
    ]));
    let sword = items.id_of(&sword_name).unwrap();
    let axe = items.id_of(&axe_name).unwrap();
    let mut state = interaction_state_for_items(items);
    state.item_facts = Arc::new(ItemFactsTable::from_entries([
        (
            sword_name,
            mc_data::item_components::ItemFacts {
                attack_speed_modifier: Some(-2.4),
                ..mc_data::item_components::ItemFacts::default()
            },
        ),
        (
            axe_name,
            mc_data::item_components::ItemFacts {
                attack_speed_modifier: Some(-3.2),
                ..mc_data::item_components::ItemFacts::default()
            },
        ),
        (
            shield_name,
            mc_data::item_components::ItemFacts {
                max_damage: Some(SHIELD_FALLBACK_MAX_DAMAGE as u32),
                ..mc_data::item_components::ItemFacts::default()
            },
        ),
    ]));
    (state, sword, axe)
}

#[test]
fn empty_hand_attack_strength_scales_partial_and_full_damage() {
    let (state, _, _) = attack_strength_test_state();

    assert_attack_damage_close(
        held_attack_speed(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
        ),
        4.0,
    );
    assert_attack_damage_close(
        held_attack_damage_at_tick(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
            Some(100),
            102,
        ),
        0.4,
    );
    assert_attack_damage_close(
        held_attack_damage_at_tick(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
            Some(100),
            105,
        ),
        1.0,
    );
    assert_attack_damage_close(
        held_attack_damage_at_tick(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
            None,
            0,
        ),
        1.0,
    );
}

#[test]
fn sword_attack_speed_modifier_scales_partial_and_full_damage() {
    let (mut state, sword, _) = attack_strength_test_state();
    *state.inventory.held_mut(0).unwrap() = ItemStack::new(sword, 1)
        .with_enchantment(Identifier::parse("minecraft:sharpness").unwrap(), 3);

    assert_attack_damage_close(
        held_attack_speed(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
        ),
        1.6,
    );
    assert_attack_damage_close(
        held_attack_damage_at_tick(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
            Some(100),
            106,
        ),
        3.121_599_7,
    );
    assert_attack_damage_close(
        held_attack_damage_at_tick(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
            Some(100),
            112,
        ),
        7.0,
    );
}

#[test]
fn axe_attack_speed_modifier_scales_partial_and_full_damage() {
    let (mut state, _, axe) = attack_strength_test_state();
    *state.inventory.held_mut(0).unwrap() = ItemStack::new(axe, 1);

    assert_attack_damage_close(
        held_attack_speed(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
        ),
        0.8,
    );
    assert_attack_damage_close(
        held_attack_damage_at_tick(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
            Some(100),
            112,
        ),
        2.8,
    );
    assert_attack_damage_close(
        held_attack_damage_at_tick(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
            Some(100),
            125,
        ),
        7.0,
    );
}

#[test]
fn attack_damage_scales_all_playable_modes_without_recording_before_validation() {
    let (mut state, sword, _) = attack_strength_test_state();
    *state.inventory.held_mut(0).unwrap() = ItemStack::new(sword, 1);

    for game_mode in [GameMode::Survival, GameMode::Adventure, GameMode::Creative] {
        state.last_entity_attack_tick = Some(100);
        let damage = begin_player_attack_attempt(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
            game_mode,
            state.last_entity_attack_tick,
            106,
        )
        .expect("non-spectator attack attempt");

        assert_attack_damage_close(damage, 2.081_599_7);
        assert_eq!(state.last_entity_attack_tick, Some(100));
    }

    state.last_entity_attack_tick = Some(100);
    assert_eq!(
        begin_player_attack_attempt(
            &state.item_facts,
            &state.items,
            state.inventory.held(state.selected_hotbar_slot).unwrap(),
            GameMode::Spectator,
            state.last_entity_attack_tick,
            106,
        ),
        None
    );
    assert_eq!(state.last_entity_attack_tick, Some(100));
}

#[test]
fn pvp_hurt_resistance_rejects_weaker_hits_and_applies_stronger_difference() {
    let mut resistance = PlayerHurtResistance::default();

    assert_eq!(
        resistance.resolve(100, 5.0),
        PlayerHurtResolution::Apply {
            amount: 5.0,
            fresh_hurt: true,
        }
    );
    assert_eq!(resistance.resolve(100, 1.0), PlayerHurtResolution::Rejected);
    assert_eq!(
        resistance.resolve(100, 7.0),
        PlayerHurtResolution::Apply {
            amount: 2.0,
            fresh_hurt: false,
        }
    );
    assert_eq!(resistance.resolve(109, 7.0), PlayerHurtResolution::Rejected);
    assert_eq!(
        resistance.resolve(110, 3.0),
        PlayerHurtResolution::Apply {
            amount: 3.0,
            fresh_hurt: true,
        }
    );
}

#[test]
fn queued_same_tick_pvp_hits_share_one_victim_hurt_resistance_state() {
    let mut resistance = PlayerHurtResistance::default();

    let first = resistance.resolve(42, 4.0);
    let second = resistance.resolve(42, 4.0);

    assert!(matches!(first, PlayerHurtResolution::Apply { .. }));
    assert_eq!(second, PlayerHurtResolution::Rejected);
}

#[test]
fn hurt_resistance_preview_changes_state_only_after_authority_commit() {
    let resistance = PlayerHurtResistance::default();
    let (first, committed) = resistance.preview(42, 4.0);
    let (retry, _) = resistance.preview(42, 4.0);

    assert!(matches!(first, PlayerHurtResolution::Apply { .. }));
    assert_eq!(
        retry, first,
        "a rejected commit must leave resistance unchanged"
    );
    assert_eq!(
        committed.preview(42, 4.0).0,
        PlayerHurtResolution::Rejected,
        "the next hit is rejected only after the transition is committed"
    );
}

#[tokio::test]
async fn adventure_player_accepts_pvp_damage() {
    let mut survival = SurvivalState::FULL;
    let mut xp = XpState::default();
    let mut writer = Vec::new();

    let applied = apply_player_damage(
        None,
        &mut writer,
        Compression::Disabled,
        &mut survival,
        &mut xp,
        GameMode::Adventure,
        PlayerDamageApplication {
            player_pose: PlayerPose::new(0.5, 64.0, 0.5),
            request: PlayerDamageRequest {
                kind: PlayerDamageKind::PlayerAttack,
                amount: 4.0,
                source_origin: Some(Vec3::new(0.5, 64.0, 1.5)),
            },
        },
    )
    .await
    .unwrap();

    assert!(applied);
    assert_eq!(survival.health, 16.0);
}

#[tokio::test]
async fn creative_and_spectator_players_reject_pvp_damage() {
    for game_mode in [GameMode::Creative, GameMode::Spectator] {
        let mut survival = SurvivalState::FULL;
        let mut xp = XpState::default();
        let mut writer = Vec::new();

        let applied = apply_player_damage(
            None,
            &mut writer,
            Compression::Disabled,
            &mut survival,
            &mut xp,
            game_mode,
            PlayerDamageApplication {
                player_pose: PlayerPose::new(0.5, 64.0, 0.5),
                request: PlayerDamageRequest {
                    kind: PlayerDamageKind::PlayerAttack,
                    amount: 4.0,
                    source_origin: Some(Vec3::new(0.5, 64.0, 1.5)),
                },
            },
        )
        .await
        .unwrap();

        assert!(!applied);
        assert_eq!(survival, SurvivalState::FULL);
        assert!(writer.is_empty());
    }
}

async fn run_pvp_commit_cost_case(
    damage_expected: bool,
    keep_target_queue: bool,
) -> (InteractionState, SurvivalState) {
    let (mut state, sword, _) = attack_strength_test_state();
    let sword_name = Identifier::parse("minecraft:stone_sword").unwrap();
    let shield_name = Identifier::parse("minecraft:shield").unwrap();
    state.item_facts = Arc::new(ItemFactsTable::from_entries([
        (
            sword_name,
            mc_data::item_components::ItemFacts {
                max_damage: Some(131),
                weapon: true,
                weapon_damage_per_attack: Some(1),
                attack_damage_modifier: Some(4.0),
                attack_speed_modifier: Some(-2.4),
                ..mc_data::item_components::ItemFacts::default()
            },
        ),
        (
            shield_name.clone(),
            mc_data::item_components::ItemFacts {
                max_damage: Some(SHIELD_FALLBACK_MAX_DAMAGE as u32),
                ..mc_data::item_components::ItemFacts::default()
            },
        ),
    ]));
    *state.inventory.held_mut(0).unwrap() = ItemStack::new(sword, 1);
    let mut survival = SurvivalState {
        exhaustion: 3.95,
        ..SurvivalState::FULL
    };
    let mut xp = XpState::default();
    let (stop, owner_task) =
        start_survival_test_owner(&mut state, "PvpCostAttacker", survival, &xp);

    let target_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(if damage_expected { 902 } else { 901 }),
        name: if damage_expected {
            "PvpCostAccepted".to_owned()
        } else {
            "PvpCostRejected".to_owned()
        },
    };
    let mut target_pose = PlayerPose::new(0.5, 64.0, 1.5);
    target_pose.yaw = 180.0;
    let (target_tx, target_rx) = mpsc::channel(8);
    let mut target_rx = keep_target_queue.then_some(target_rx);
    let (target_session, _) = state.sessions.register(
        &target_profile,
        (0, 0),
        0,
        HashSet::new(),
        target_tx,
        target_pose,
    );
    let mut target_state = PlayerPersistedState::new_default(target_pose);
    if !damage_expected {
        let shield = state.items.id_of(&shield_name).unwrap();
        target_state.inventory.slots[45] = ItemStack::new(shield, 1);
    }
    let target_state = Arc::new(Mutex::new(target_state));
    state
        .sessions
        .register_player_persistence(target_session, Arc::clone(&target_state));
    if !damage_expected {
        state.sessions.set_active_shield(
            target_session,
            Some(ActiveShield {
                started_tick: state.sessions.world_time(),
                slot: 45,
                expected_stack: target_state.lock().unwrap().inventory.slots[45].clone(),
            }),
        );
        state
            .sessions
            .advance_world_time(SHIELD_ACTIVATION_DELAY_TICKS);
    }
    let mut writer = Vec::new();

    tokio::time::timeout(
        Duration::from_secs(1),
        handle_attack(
            &mut state,
            &mut writer,
            GameMode::Survival,
            &mut survival,
            &mut xp,
            PlayerPose::new(0.5, 64.0, 0.5),
            ServerboundAttack {
                entity_id: i32::try_from(target_session).unwrap(),
            },
        ),
    )
    .await
    .expect("PvP authority must not wait for the target connection loop")
    .unwrap();

    assert!(
        state.last_entity_attack_tick.is_some(),
        "reachable target must pass PvP validation"
    );
    assert_eq!(
        target_state.lock().unwrap().survival.health < SurvivalState::FULL.health,
        damage_expected
    );
    if !damage_expected {
        assert!(
            target_state.lock().unwrap().inventory.slots[45]
                .damage
                .is_some(),
            "shield durability must commit before hurt resistance"
        );
    }
    if let Some(target_rx) = target_rx.as_mut() {
        let command = tokio::time::timeout(Duration::from_secs(1), target_rx.recv())
            .await
            .expect("PvP commit must publish to the target connection");
        let Some(OutboundCommand::PlayerDamageCommitted { publication, .. }) = command else {
            panic!("PvP commit must publish committed target state");
        };
        assert_eq!(publication.shield_blocked, !damage_expected);
        assert!(
            publication.knockback.is_some(),
            "fresh damage and a full shield block both publish target knockback"
        );
    }

    stop.send(()).unwrap();
    owner_task.await.unwrap();
    (state, survival)
}

#[tokio::test]
async fn authoritative_pvp_commit_gates_exhaustion_and_weapon_durability() {
    let (rejected, rejected_survival) = run_pvp_commit_cost_case(false, true).await;
    assert!(rejected.last_entity_attack_tick.is_some());
    assert_eq!(rejected_survival.saturation, 5.0);
    assert_eq!(rejected_survival.exhaustion, 3.95);
    assert_eq!(rejected.inventory.held(0).unwrap().damage, None);
    let rejected_persisted = rejected
        .sessions
        .persisted_player_states()
        .into_iter()
        .find(|(uuid, _, _)| *uuid == crate::login::offline_uuid("PvpCostAttacker"))
        .map(|(_, state, _)| state)
        .expect("rejected attacker authority state remains registered");
    assert_eq!(rejected_persisted.inventory.held(0).unwrap().damage, None);
    assert_eq!(rejected_persisted.survival, rejected_survival);

    let (accepted, accepted_survival) = run_pvp_commit_cost_case(true, true).await;
    assert!(accepted.last_entity_attack_tick.is_some());
    assert_eq!(accepted_survival.saturation, 4.0);
    assert!((accepted_survival.exhaustion - 0.05).abs() < 0.000_01);
    assert_eq!(accepted.inventory.held(0).unwrap().damage, Some(1));
    let persisted = accepted
        .sessions
        .persisted_player_states()
        .into_iter()
        .find(|(uuid, _, _)| *uuid == crate::login::offline_uuid("PvpCostAttacker"))
        .map(|(_, state, _)| state)
        .expect("attacker authority state remains registered");
    assert_eq!(persisted.inventory.held(0).unwrap().damage, Some(1));
    assert_eq!(persisted.survival, accepted_survival);
}

#[tokio::test]
async fn dropped_target_publication_does_not_undo_authoritative_pvp_costs() {
    let (attacker, survival) = run_pvp_commit_cost_case(true, false).await;

    assert!(attacker.last_entity_attack_tick.is_some());
    assert_eq!(survival.saturation, 4.0);
    assert!((survival.exhaustion - 0.05).abs() < 0.000_01);
    assert_eq!(attacker.inventory.held(0).unwrap().damage, Some(1));
}

#[tokio::test]
async fn server_entity_attack_commits_weapon_costs_in_authority() {
    let (mut state, sword, _) = attack_strength_test_state();
    *state.inventory.held_mut(0).unwrap() = ItemStack::new(sword, 1);
    let mut survival = SurvivalState::FULL;
    let mut xp = XpState::default();
    let (stop, owner_task) =
        start_survival_test_owner(&mut state, "MobCostAttacker", survival, &xp);
    state.sessions.spawn_command_entity(
        &simulation::SimulationAuthority::for_test(),
        54,
        "minecraft:zombie".to_owned(),
        Vec3::new(0.5, 64.0, 1.0),
    );
    let target = state
        .sessions
        .nearby_hostile_entities(Vec3::new(0.5, 64.0, 1.0), 2.25)
        .into_iter()
        .next()
        .expect("spawned zombie has an entity id");
    let mut writer = Vec::new();

    handle_attack(
        &mut state,
        &mut writer,
        GameMode::Survival,
        &mut survival,
        &mut xp,
        PlayerPose::new(0.5, 64.0, 0.5),
        ServerboundAttack {
            entity_id: target.id.0,
        },
    )
    .await
    .unwrap();

    assert_eq!(state.inventory.held(0).unwrap().damage, Some(1));
    let persisted = state
        .sessions
        .persisted_player_states()
        .into_iter()
        .find(|(uuid, _, _)| *uuid == crate::login::offline_uuid("MobCostAttacker"))
        .map(|(_, state, _)| state)
        .expect("attacker authority state remains registered");
    assert_eq!(persisted.inventory.held(0).unwrap().damage, Some(1));
    assert_eq!(persisted.survival, survival);

    stop.send(()).unwrap();
    owner_task.await.unwrap();
}

#[tokio::test]
async fn out_of_reach_attack_does_not_reset_attacker_strength() {
    let mut state = interaction_state_for_items(Arc::new(ItemRegistry::default()));
    let mut survival = SurvivalState::FULL;
    let mut xp = XpState::default();
    let (stop, owner_task) =
        start_survival_test_owner(&mut state, "OutOfReachAttack", survival, &xp);
    state.sessions.spawn_command_entity(
        &simulation::SimulationAuthority::for_test(),
        54,
        "minecraft:zombie".to_string(),
        Vec3::new(0.5, 64.0, 20.5),
    );
    let target = state
        .sessions
        .nearby_hostile_entities(Vec3::new(0.5, 64.0, 20.5), 2.25)
        .into_iter()
        .next()
        .expect("spawned zombie has an entity id");
    let last_attack_tick = state.sessions.simulation_tick() + 1;
    state.last_entity_attack_tick = Some(last_attack_tick);
    let mut writer = Vec::new();

    handle_attack(
        &mut state,
        &mut writer,
        GameMode::Survival,
        &mut survival,
        &mut xp,
        PlayerPose::new(0.5, 64.0, 0.5),
        ServerboundAttack {
            entity_id: target.id.0,
        },
    )
    .await
    .unwrap();

    assert_eq!(state.last_entity_attack_tick, Some(last_attack_tick));
    stop.send(()).unwrap();
    owner_task.await.unwrap();
}

#[tokio::test]
async fn reachable_mob_hurt_immunity_still_resets_attacker_strength() {
    let mut state = interaction_state_for_items(Arc::new(ItemRegistry::default()));
    let mut survival = SurvivalState::FULL;
    let mut xp = XpState::default();
    let (stop, owner_task) =
        start_survival_test_owner(&mut state, "ImmuneMobAttack", survival, &xp);
    state.sessions.spawn_command_entity(
        &simulation::SimulationAuthority::for_test(),
        54,
        "minecraft:zombie".to_string(),
        Vec3::new(0.5, 64.0, 1.0),
    );
    let target = state
        .sessions
        .nearby_hostile_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
        .into_iter()
        .next()
        .expect("spawned zombie is attackable");
    let mut writer = Vec::new();
    let pose = PlayerPose::new(0.5, 64.0, 0.5);

    handle_attack(
        &mut state,
        &mut writer,
        GameMode::Survival,
        &mut survival,
        &mut xp,
        pose,
        ServerboundAttack {
            entity_id: target.id.0,
        },
    )
    .await
    .unwrap();
    let first_attempt_tick = state.last_entity_attack_tick.unwrap();
    let writer_len_after_first_hit = writer.len();
    state.sessions.advance_world_time(1);

    handle_attack(
        &mut state,
        &mut writer,
        GameMode::Survival,
        &mut survival,
        &mut xp,
        pose,
        ServerboundAttack {
            entity_id: target.id.0,
        },
    )
    .await
    .unwrap();
    let immune_attempt_tick = state.last_entity_attack_tick.unwrap();

    assert!(immune_attempt_tick > first_attempt_tick);
    assert_eq!(writer.len(), writer_len_after_first_hit);
    stop.send(()).unwrap();
    owner_task.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_lethal_attacks_create_one_drop_and_one_xp_reward() {
    let rotten_flesh = Identifier::parse("minecraft:rotten_flesh").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: rotten_flesh,
        protocol_id: 10,
    }]));
    let rotten_flesh_id = items
        .id_of(&Identifier::parse("minecraft:rotten_flesh").unwrap())
        .unwrap();
    let entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    let mut alice = interaction_state_for_items(Arc::clone(&items));
    let mut bob = interaction_state_for_items(items);
    bob.sessions = Arc::clone(&alice.sessions);
    let (simulation, simulation_stop_tx, simulation_task) =
        spawn_test_simulation_owner(Arc::clone(&alice.sessions));
    alice.entity_types = Arc::clone(&entity_types);
    bob.entity_types = entity_types;
    alice.sessions.configure_arrow_kill_rewards(
        item_entity_type_id(&alice.entity_types),
        xp_orb_entity_type_id(&alice.entity_types),
        arrow_entity_type_id(&alice.entity_types),
        Arc::clone(&alice.items),
        Arc::clone(&alice.item_facts),
        Arc::clone(&alice.loot),
    );
    let alice_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(82),
        name: "LethalAlice".to_string(),
    };
    let bob_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(83),
        name: "LethalBob".to_string(),
    };
    let (alice_tx, _alice_rx) = mpsc::channel(16);
    let (bob_tx, _bob_rx) = mpsc::channel(16);
    let desired = HashSet::from([(0, 0)]);
    let (alice_id, _) = alice.sessions.register(
        &alice_profile,
        (0, 0),
        0,
        desired.clone(),
        alice_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let (bob_id, _) = alice.sessions.register(
        &bob_profile,
        (0, 0),
        0,
        desired,
        bob_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let spawn = PlayerPose::new(0.5, 64.0, 0.5);
    alice.sessions.register_player_persistence(
        alice_id,
        Arc::new(Mutex::new(PlayerPersistedState::new_default(spawn))),
    );
    alice.sessions.register_player_persistence(
        bob_id,
        Arc::new(Mutex::new(PlayerPersistedState::new_default(spawn))),
    );
    let _ = alice.sessions.mark_loaded(alice_id, (0, 0));
    let _ = alice.sessions.mark_loaded(bob_id, (0, 0));
    alice.session_id = alice_id;
    bob.session_id = bob_id;
    alice.simulation = simulation.for_session(alice_id);
    bob.simulation = simulation.for_session(bob_id);
    alice.sessions.spawn_command_entity(
        &simulation::SimulationAuthority::for_test(),
        54,
        "minecraft:zombie".to_string(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let target = alice
        .sessions
        .nearby_hostile_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
        .into_iter()
        .next()
        .expect("spawned zombie is attackable");
    let pre_damage = alice
        .sessions
        .damage_server_entity_for_test(target.id, 19.0)
        .expect("prime zombie to one health");
    assert_eq!(pre_damage.snapshot.health, 1.0);
    alice
        .sessions
        .advance_world_time(ENTITY_HURT_INVULNERABLE_TICKS);

    let gate = Arc::new(tokio::sync::Barrier::new(3));
    let alice_task = {
        let gate = Arc::clone(&gate);
        tokio::spawn(async move {
            let mut writer = Vec::new();
            let mut survival = SurvivalState::FULL;
            let mut xp = XpState::default();
            gate.wait().await;
            handle_attack(
                &mut alice,
                &mut writer,
                GameMode::Survival,
                &mut survival,
                &mut xp,
                PlayerPose::new(0.5, 64.0, 0.5),
                ServerboundAttack {
                    entity_id: target.id.0,
                },
            )
            .await
            .expect("Alice lethal attack task succeeds");
            (alice, xp)
        })
    };
    let bob_task = {
        let gate = Arc::clone(&gate);
        tokio::spawn(async move {
            let mut writer = Vec::new();
            let mut survival = SurvivalState::FULL;
            let mut xp = XpState::default();
            gate.wait().await;
            handle_attack(
                &mut bob,
                &mut writer,
                GameMode::Survival,
                &mut survival,
                &mut xp,
                PlayerPose::new(0.5, 64.0, 0.5),
                ServerboundAttack {
                    entity_id: target.id.0,
                },
            )
            .await
            .expect("Bob lethal attack task succeeds");
            (bob, xp)
        })
    };
    gate.wait().await;
    let (alice, _alice_xp) = alice_task.await.expect("Alice lethal task joins");
    let (_bob, _bob_xp) = bob_task.await.expect("Bob lethal task joins");
    let _ = simulation_stop_tx.send(());
    simulation_task.await.expect("simulation owner joins");

    assert!(alice.sessions.server_entity_snapshot(target.id).is_some());
    alice.sessions.advance_world_time(ITEM_PICKUP_DELAY_TICKS);
    let drops = alice
        .sessions
        .nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25);
    assert_eq!(drops.len(), 1);
    assert_eq!(
        drops[0].item_stack,
        Some(EntityItemStack::new(rotten_flesh_id, 1))
    );
    let experience = alice
        .sessions
        .nearby_experience_entities(Vec3::new(0.5, 64.0, 0.5), 2.25);
    assert_eq!(experience.len(), 1);
    assert_eq!(experience[0].experience_value, Some(5));
}

#[test]
fn furnace_like_recipe_lookup_uses_matching_cooking_category() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let iron_ore = Identifier::parse("minecraft:iron_ore").unwrap();
    let raw_iron = Identifier::parse("minecraft:raw_iron").unwrap();
    let beef = Identifier::parse("minecraft:beef").unwrap();
    let porkchop = Identifier::parse("minecraft:porkchop").unwrap();
    let iron_ingot = Identifier::parse("minecraft:iron_ingot").unwrap();
    let cooked_beef = Identifier::parse("minecraft:cooked_beef").unwrap();
    let cooked_porkchop = Identifier::parse("minecraft:cooked_porkchop").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: iron_ore.clone(),
            protocol_id: 10,
        },
        ItemReport {
            id: raw_iron.clone(),
            protocol_id: 11,
        },
        ItemReport {
            id: beef.clone(),
            protocol_id: 12,
        },
        ItemReport {
            id: porkchop.clone(),
            protocol_id: 13,
        },
        ItemReport {
            id: iron_ingot.clone(),
            protocol_id: 20,
        },
        ItemReport {
            id: cooked_beef.clone(),
            protocol_id: 21,
        },
        ItemReport {
            id: cooked_porkchop.clone(),
            protocol_id: 22,
        },
    ]);
    let ingredient = |item: Identifier| Ingredient {
        alternatives: vec![IngredientAlternative::Item(item)],
    };
    let cooking = |item: Identifier, cooking_time| SmeltingRecipe {
        ingredient: ingredient(item),
        cooking_time,
        experience_milli: 0,
    };
    let result = |item: Identifier| RecipeResult { item, count: 1 };
    let recipes = vec![
        Recipe {
            id: Identifier::parse("minecraft:test_smelting").unwrap(),
            kind: RecipeKind::Smelting(cooking(iron_ore, 200)),
            result: result(iron_ingot.clone()),
        },
        Recipe {
            id: Identifier::parse("minecraft:test_blasting").unwrap(),
            kind: RecipeKind::Blasting(cooking(raw_iron.clone(), 100)),
            result: result(iron_ingot),
        },
        Recipe {
            id: Identifier::parse("minecraft:test_smoking").unwrap(),
            kind: RecipeKind::Smoking(cooking(beef.clone(), 100)),
            result: result(cooked_beef),
        },
        Recipe {
            id: Identifier::parse("minecraft:test_campfire").unwrap(),
            kind: RecipeKind::CampfireCooking(cooking(porkchop.clone(), 600)),
            result: result(cooked_porkchop),
        },
    ];
    let tags = TagsData::default();

    assert_eq!(
        containers::find_cooking_recipe_for_item(&recipes, &items, &tags, FurnaceKind::Furnace, 10)
            .map(|recipe| recipe.id),
        Some(Identifier::parse("minecraft:test_smelting").unwrap())
    );
    assert!(
        containers::find_cooking_recipe_for_item(&recipes, &items, &tags, FurnaceKind::Furnace, 11)
            .is_none()
    );
    assert_eq!(
        containers::find_cooking_recipe_for_item(
            &recipes,
            &items,
            &tags,
            FurnaceKind::BlastFurnace,
            11
        )
        .map(|recipe| recipe.id),
        Some(Identifier::parse("minecraft:test_blasting").unwrap())
    );
    assert_eq!(
        containers::find_cooking_recipe_for_item(&recipes, &items, &tags, FurnaceKind::Smoker, 12)
            .map(|recipe| recipe.id),
        Some(Identifier::parse("minecraft:test_smoking").unwrap())
    );
    assert!(
        containers::find_cooking_recipe_for_item(&recipes, &items, &tags, FurnaceKind::Furnace, 13)
            .is_none()
    );
}

#[test]
fn campfire_recipe_lookup_uses_campfire_category() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let porkchop = Identifier::parse("minecraft:porkchop").unwrap();
    let beef = Identifier::parse("minecraft:beef").unwrap();
    let cooked_porkchop = Identifier::parse("minecraft:cooked_porkchop").unwrap();
    let cooked_beef = Identifier::parse("minecraft:cooked_beef").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: porkchop.clone(),
            protocol_id: 13,
        },
        ItemReport {
            id: beef.clone(),
            protocol_id: 14,
        },
        ItemReport {
            id: cooked_porkchop.clone(),
            protocol_id: 22,
        },
        ItemReport {
            id: cooked_beef.clone(),
            protocol_id: 23,
        },
    ]);
    let ingredient = |item: Identifier| Ingredient {
        alternatives: vec![IngredientAlternative::Item(item)],
    };
    let cooking = |item: Identifier, cooking_time| SmeltingRecipe {
        ingredient: ingredient(item),
        cooking_time,
        experience_milli: 0,
    };
    let result = |item: Identifier| RecipeResult { item, count: 1 };
    let recipes = vec![
        Recipe {
            id: Identifier::parse("minecraft:test_smoking").unwrap(),
            kind: RecipeKind::Smoking(cooking(beef.clone(), 100)),
            result: result(cooked_beef),
        },
        Recipe {
            id: Identifier::parse("minecraft:test_campfire").unwrap(),
            kind: RecipeKind::CampfireCooking(cooking(porkchop, 600)),
            result: result(cooked_porkchop),
        },
    ];
    let tags = TagsData::default();

    assert_eq!(
        containers::find_campfire_recipe_in(&recipes, &items, &tags, 13).map(|recipe| recipe.id),
        Some(Identifier::parse("minecraft:test_campfire").unwrap())
    );
    assert!(containers::find_campfire_recipe_in(&recipes, &items, &tags, 14).is_none());
}

#[test]
fn campfire_cooking_rejects_invalid_when_full() {
    let mut cooking = CampfireCookingState::default();

    for item_id in 1..=CAMPFIRE_COOKING_SLOT_COUNT as u32 {
        assert!(cooking.insert(ItemStack::new(item_id, 1), ItemStack::new(item_id, 1), 5));
    }
    assert!(!cooking.insert(ItemStack::new(99, 1), ItemStack::new(99, 1), 5));
}

#[test]
fn unlit_campfire_cools_every_active_slot_by_two_progress() {
    let mut cooking = CampfireCookingState::default();
    for item_id in 1..=CAMPFIRE_COOKING_SLOT_COUNT as u32 {
        assert!(cooking.insert(
            ItemStack::new(item_id, 1),
            ItemStack::new(item_id + 10, 1),
            10
        ));
    }
    cooking.slots[0].as_mut().unwrap().ticks_remaining = 9;
    cooking.slots[1].as_mut().unwrap().ticks_remaining = 7;
    cooking.slots[2].as_mut().unwrap().ticks_remaining = 10;
    cooking.slots[3].as_mut().unwrap().ticks_remaining = 1;

    assert!(cooking.cool_down());
    assert_eq!(
        cooking
            .slots
            .each_ref()
            .map(|slot| slot.as_ref().unwrap().ticks_remaining),
        [10, 9, 10, 3]
    );
}

#[tokio::test]
async fn full_campfire_consumes_valid_food_interaction_without_debit() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let position = mc_world::BlockPos { x: 0, y: 64, z: 0 };
    let mut state = campfire_test_interaction_state(position).await;
    let raw = Identifier::parse("minecraft:porkchop").unwrap();
    let cooked = Identifier::parse("minecraft:cooked_porkchop").unwrap();
    state.items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: raw.clone(),
            protocol_id: 13,
        },
        ItemReport {
            id: cooked.clone(),
            protocol_id: 22,
        },
    ]));
    state.item_to_block = ItemToBlockTable::build(&state.items, &state.blocks);
    state.recipes = vec![Recipe {
        id: Identifier::parse("minecraft:test_campfire").unwrap(),
        kind: RecipeKind::CampfireCooking(SmeltingRecipe {
            ingredient: Ingredient {
                alternatives: vec![IngredientAlternative::Item(raw)],
            },
            cooking_time: 100,
            experience_milli: 0,
        }),
        result: RecipeResult {
            item: cooked,
            count: 1,
        },
    }];
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(13, 5);
    for _ in 0..CAMPFIRE_COOKING_SLOT_COUNT {
        assert!(
            state
                .sessions
                .insert_campfire_cooking(
                    position,
                    ItemStack::new(13, 1),
                    ItemStack::new(22, 1),
                    100,
                )
                .is_some()
        );
    }
    let expected = state.sessions.campfire_cooking_state(position);
    let mut writer = Vec::new();

    assert!(
        handle_campfire_use_on(
            &mut state,
            &mut writer,
            GameMode::Survival,
            77,
            position,
            InteractionHand::MainHand,
        )
        .await
        .unwrap()
    );
    assert_eq!(
        state.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(13, 5)
    );
    assert_eq!(state.sessions.campfire_cooking_state(position), expected);
    let mut bytes = bytes::BytesMut::from(writer.as_slice());
    let mut frame = mc_protocol::frame::try_decode_frame(&mut bytes, Compression::Disabled)
        .unwrap()
        .expect("full campfire interaction acknowledgement");
    assert_eq!(frame.id, BlockChangedAck::ID);
    assert_eq!(
        BlockChangedAck::decode(&mut frame.body).unwrap().sequence,
        77
    );
    assert!(bytes.is_empty());
}

#[test]
fn campfire_cooking_moves_completed_output_to_pending_intent() {
    let mut cooking = CampfireCookingState::default();
    assert!(cooking.insert(ItemStack::new(41, 1), ItemStack::new(42, 1), 2));

    assert!(cooking.tick().completed.is_empty());
    assert_eq!(cooking.tick().completed, vec![ItemStack::new(42, 1)]);
    assert!(cooking.slots.iter().all(Option::is_none));
    assert_eq!(cooking.pending_outputs.len(), 1);
    assert_eq!(
        cooking.pending_outputs[0].stack,
        EntityItemStack::new(42, 1)
    );
}

#[test]
fn campfire_persistent_nbt_uses_vanilla_cooking_arrays_and_reads_legacy() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let porkchop = Identifier::parse("minecraft:porkchop").unwrap();
    let cooked_porkchop = Identifier::parse("minecraft:cooked_porkchop").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: porkchop.clone(),
            protocol_id: 13,
        },
        ItemReport {
            id: cooked_porkchop.clone(),
            protocol_id: 22,
        },
    ]);
    let recipes = vec![Recipe {
        id: Identifier::parse("minecraft:test_campfire").unwrap(),
        kind: RecipeKind::CampfireCooking(SmeltingRecipe {
            ingredient: Ingredient {
                alternatives: vec![IngredientAlternative::Item(porkchop.clone())],
            },
            cooking_time: 100,
            experience_milli: 0,
        }),
        result: RecipeResult {
            item: cooked_porkchop,
            count: 1,
        },
    }];
    let tags = TagsData::default();

    let mut cooking = CampfireCookingState::default();
    assert!(cooking.insert(ItemStack::new(13, 1), ItemStack::new(22, 1), 100));
    cooking.slots[0].as_mut().unwrap().ticks_remaining = 75;
    let tag = campfire_block_entity_persistent_nbt(
        "minecraft:campfire",
        mc_world::BlockPos { x: 1, y: 2, z: 3 },
        &items,
        &cooking,
    )
    .expect("persistent campfire tag");
    assert_eq!(
        compound_int_array_field(&tag, CAMPFIRE_NBT_COOKING_TIMES),
        Some(&[25, 0, 0, 0][..])
    );
    assert_eq!(
        compound_int_array_field(&tag, CAMPFIRE_NBT_COOKING_TOTAL_TIMES),
        Some(&[100, 0, 0, 0][..])
    );
    assert_eq!(
        compound_int_array_field(&tag, LEGACY_CAMPFIRE_NBT_REMAINING),
        None
    );

    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &tag).expect("encode vanilla campfire tag");
    let restored =
        campfire_cooking_state_from_persistent_nbt(&bytes, &recipes, &items, &tags).unwrap();
    let restored_slot = restored.slots[0].as_ref().unwrap();
    assert_eq!(restored_slot.ticks_remaining, 75);
    assert_eq!(restored_slot.cooking_time_total, 100);

    let legacy_tag = Tag::Compound(vec![
        ("id".into(), Tag::String("minecraft:campfire".into())),
        (
            "Items".into(),
            Tag::List(ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: vec![Tag::Compound(vec![
                    ("Slot".into(), Tag::Int(0)),
                    ("id".into(), Tag::String(porkchop.as_str().to_string())),
                    ("count".into(), Tag::Int(1)),
                ])],
            }),
        ),
        (
            LEGACY_CAMPFIRE_NBT_REMAINING.into(),
            Tag::IntArray(vec![33, 0, 0, 0]),
        ),
        (
            LEGACY_CAMPFIRE_NBT_TOTAL.into(),
            Tag::IntArray(vec![100, 0, 0, 0]),
        ),
    ]);
    let mut legacy_bytes = Vec::new();
    mc_nbt::write_network(&mut legacy_bytes, &legacy_tag).expect("encode legacy campfire tag");
    let restored_legacy =
        campfire_cooking_state_from_persistent_nbt(&legacy_bytes, &recipes, &items, &tags).unwrap();
    let restored_legacy_slot = restored_legacy.slots[0].as_ref().unwrap();
    assert_eq!(restored_legacy_slot.ticks_remaining, 33);
    assert_eq!(restored_legacy_slot.cooking_time_total, 100);
}

#[tokio::test]
async fn campfire_startup_hydration_only_reads_resident_chunks() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:campfire"),
        ])
        .unwrap(),
    );
    let porkchop = Identifier::parse("minecraft:porkchop").unwrap();
    let cooked_porkchop = Identifier::parse("minecraft:cooked_porkchop").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: porkchop.clone(),
            protocol_id: 13,
        },
        ItemReport {
            id: cooked_porkchop.clone(),
            protocol_id: 22,
        },
    ]));
    let recipes = Arc::new(vec![Recipe {
        id: Identifier::parse("minecraft:test_campfire").unwrap(),
        kind: RecipeKind::CampfireCooking(SmeltingRecipe {
            ingredient: Ingredient {
                alternatives: vec![IngredientAlternative::Item(porkchop)],
            },
            cooking_time: 100,
            experience_milli: 0,
        }),
        result: RecipeResult {
            item: cooked_porkchop,
            count: 1,
        },
    }]);
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let cpos = ChunkPos { x: 0, z: 0 };
    let mut cooking = CampfireCookingState::default();
    assert!(cooking.insert(ItemStack::new(13, 1), ItemStack::new(22, 1), 100));
    let bytes = campfire_block_entity_persistent_bytes("minecraft:campfire", pos, &items, &cooking)
        .expect("campfire persistence bytes");
    {
        let mut storage =
            mc_world::WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&blocks), 8)
                .unwrap()
                .with_item_registry(Arc::clone(&items));
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage.set_block_at(pos, BlockStateId(1)).unwrap();
        storage.set_opaque_block_entity(pos, bytes).unwrap();
        assert_eq!(storage.flush_dirty().unwrap(), 1);
    }

    let world = Arc::new(tokio::sync::Mutex::new(
        mc_world::WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&blocks), 8)
            .unwrap()
            .with_item_registry(Arc::clone(&items)),
    ));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        items,
        tags: Arc::new(TagsData::default()),
        recipes,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();

    assert_eq!(
        hydrate_persisted_campfire_cooking(&config, &sessions).await,
        0
    );
    assert!(sessions.campfire_cooking_state(pos).is_empty());

    world
        .lock()
        .await
        .get_chunk_without_generation(cpos)
        .unwrap()
        .expect("load persisted campfire chunk");
    assert_eq!(
        hydrate_persisted_campfire_cooking(&config, &sessions).await,
        1
    );
    assert!(!sessions.campfire_cooking_state(pos).is_empty());
}

#[tokio::test]
async fn campfire_tick_does_not_load_cold_chunks_and_is_durable_when_resident() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            BlockReport {
                id: Identifier::parse("minecraft:campfire").unwrap(),
                properties: prop_schema(&[("lit", &["true"])]),
                states: vec![state(1, true, &[("lit", "true")])],
            },
        ])
        .unwrap(),
    );
    let porkchop = Identifier::parse("minecraft:porkchop").unwrap();
    let cooked_porkchop = Identifier::parse("minecraft:cooked_porkchop").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: porkchop.clone(),
            protocol_id: 13,
        },
        ItemReport {
            id: cooked_porkchop.clone(),
            protocol_id: 22,
        },
    ]));
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let second_pos = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let cpos = ChunkPos { x: 0, z: 0 };
    {
        let mut storage =
            mc_world::WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&blocks), 8)
                .unwrap()
                .with_item_registry(Arc::clone(&items));
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage.set_block_at(pos, BlockStateId(1)).unwrap();
        storage.set_block_at(second_pos, BlockStateId(1)).unwrap();
        assert_eq!(storage.flush_dirty().unwrap(), 1);
    }

    let world = Arc::new(tokio::sync::Mutex::new(
        mc_world::WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&blocks), 8)
            .unwrap()
            .with_item_registry(Arc::clone(&items)),
    ));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        items,
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        recipes: Arc::new(vec![Recipe {
            id: Identifier::parse("minecraft:test_campfire").unwrap(),
            kind: RecipeKind::CampfireCooking(SmeltingRecipe {
                ingredient: Ingredient {
                    alternatives: vec![IngredientAlternative::Item(porkchop)],
                },
                cooking_time: 2,
                experience_milli: 0,
            }),
            result: RecipeResult {
                item: cooked_porkchop,
                count: 1,
            },
        }]),
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let mut cooking = CampfireCookingState::default();
    assert!(cooking.insert(ItemStack::new(13, 1), ItemStack::new(22, 1), 2));
    assert!(sessions.restore_campfire_cooking(pos, cooking));
    let (_simulation, owner) = simulation_channel();

    assert!(world.lock().await.cached_chunk_snapshot(cpos).is_none());
    assert_eq!(
        owner
            .run_campfire_cooking_ticks(&config, &sessions, None, None)
            .await,
        CampfireCookingTickReport::default()
    );
    assert!(world.lock().await.cached_chunk_snapshot(cpos).is_none());
    assert_eq!(
        sessions.campfire_cooking_state(pos).slots[0]
            .as_ref()
            .unwrap()
            .ticks_remaining,
        2
    );

    world
        .lock()
        .await
        .get_chunk_without_generation(cpos)
        .unwrap()
        .expect("load persisted campfire chunk");
    assert!(sessions.restore_campfire_cooking(second_pos, sessions.campfire_cooking_state(pos),));
    let (world_read, world_mutation) = {
        let storage = world.lock().await;
        (storage.read_view(), storage.mutation_view())
    };
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        tmp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);
    let writer = world.lock().await;
    let report = tokio::time::timeout(
        Duration::from_secs(1),
        owner.run_campfire_cooking_ticks(
            &config,
            &sessions,
            Some(&world_read),
            Some(&world_mutation),
        ),
    )
    .await
    .expect("resident campfire journal completion event");
    assert_eq!(
        report,
        CampfireCookingTickReport {
            persisted: 2,
            completed: 0,
            dropped: 0,
        }
    );
    assert_eq!(
        sessions.campfire_cooking_state(pos).slots[0]
            .as_ref()
            .unwrap()
            .ticks_remaining,
        1
    );
    let (reopened, pending) = super::world_journal::WorldChunkJournal::open(
        tmp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert_eq!(pending.len(), 1, "one campfire pass uses one WAL decision");
    let restored = reopened.decode_pending(&pending).unwrap();
    assert_eq!(restored.len(), 1);
    for position in [pos, second_pos] {
        let bytes = restored[0]
            .block_entities
            .get(&position)
            .expect("journaled campfire block entity");
        let cooking = campfire_cooking_state_from_persistent_nbt(
            bytes,
            &config.recipes,
            &config.items,
            &config.tags,
        )
        .expect("journaled campfire cooking state");
        assert_eq!(cooking.slots[0].as_ref().unwrap().ticks_remaining, 1);
    }
    drop(writer);
}

#[test]
fn shield_use_starts_blocking_state_for_shield_stack() {
    let stack = ItemStack::new(77, 1);

    let shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::MainHand,
        PlayerInventory::HOTBAR_BASE,
        stack.clone(),
        12,
        true,
    )
    .expect("shield stack should start shield use");

    assert_eq!(shield_use.started_tick, 12);
    assert_eq!(shield_use.slot, PlayerInventory::HOTBAR_BASE);
    assert_eq!(shield_use.stack, stack);
}

#[test]
fn shield_use_metadata_uses_vanilla_living_entity_flags() {
    let main_hand = ShieldUseState {
        hand: mc_protocol::packets::play::InteractionHand::MainHand,
        started_tick: 1,
        slot: PlayerInventory::HOTBAR_BASE,
        stack: ItemStack::new(77, 1),
    };
    let off_hand = ShieldUseState {
        hand: mc_protocol::packets::play::InteractionHand::OffHand,
        started_tick: 1,
        slot: 45,
        stack: ItemStack::new(77, 1),
    };

    assert_eq!(shield_use_flags(None), 0);
    assert_eq!(
        shield_use_flags(Some(&main_hand)),
        LIVING_ENTITY_FLAG_USING_ITEM
    );
    assert_eq!(
        shield_use_flags(Some(&off_hand)),
        LIVING_ENTITY_FLAG_USING_ITEM | LIVING_ENTITY_FLAG_OFF_HAND
    );
    assert_eq!(
        shield_use_entity_data_value(Some(&off_hand)),
        EntityDataValue::Byte {
            index: LIVING_ENTITY_DATA_FLAGS_INDEX,
            value: LIVING_ENTITY_FLAG_USING_ITEM | LIVING_ENTITY_FLAG_OFF_HAND,
        }
    );
}

#[test]
fn shield_non_shield_use_does_not_block() {
    let shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::MainHand,
        PlayerInventory::HOTBAR_BASE,
        ItemStack::new(77, 1),
        12,
        false,
    );

    assert!(shield_use.is_none());
    assert!(!shield_blocks_damage(
        Vec3::ZERO,
        0.0,
        Some(Vec3::new(0.0, 0.0, 1.0)),
        20,
        shield_use.as_ref(),
    ));
}

#[test]
fn shield_activation_delay_gates_damage() {
    let shield_use = ShieldUseState {
        hand: mc_protocol::packets::play::InteractionHand::MainHand,
        started_tick: 10,
        slot: PlayerInventory::HOTBAR_BASE,
        stack: ItemStack::new(77, 1),
    };

    assert!(!shield_blocks_damage(
        Vec3::ZERO,
        0.0,
        Some(Vec3::new(0.0, 0.0, 1.0)),
        14,
        Some(&shield_use),
    ));
    assert!(shield_blocks_damage(
        Vec3::ZERO,
        0.0,
        Some(Vec3::new(0.0, 0.0, 1.0)),
        15,
        Some(&shield_use),
    ));
}

#[test]
fn shield_blocks_frontal_mob_and_arrow_sources() {
    let shield_use = ShieldUseState {
        hand: mc_protocol::packets::play::InteractionHand::OffHand,
        started_tick: 1,
        slot: 45,
        stack: ItemStack::new(77, 1),
    };

    assert!(shield_blocks_damage(
        Vec3::ZERO,
        0.0,
        Some(Vec3::new(0.0, 0.0, 2.0)),
        10,
        Some(&shield_use),
    ));
    assert!(shield_blocks_damage(
        Vec3::ZERO,
        90.0,
        Some(Vec3::new(-2.0, 0.0, 0.0)),
        10,
        Some(&shield_use),
    ));
}

#[test]
fn shield_side_boundary_blocks_but_back_and_unknown_sources_do_not() {
    let shield_use = ShieldUseState {
        hand: mc_protocol::packets::play::InteractionHand::MainHand,
        started_tick: 1,
        slot: PlayerInventory::HOTBAR_BASE,
        stack: ItemStack::new(77, 1),
    };

    assert!(shield_blocks_damage(
        Vec3::ZERO,
        0.0,
        Some(Vec3::new(2.0, 0.0, 0.0)),
        10,
        Some(&shield_use),
    ));
    assert!(!shield_blocks_damage(
        Vec3::ZERO,
        0.0,
        Some(Vec3::new(0.0, 0.0, -2.0)),
        10,
        Some(&shield_use),
    ));
    assert!(!shield_blocks_damage(
        Vec3::ZERO,
        0.0,
        None,
        10,
        Some(&shield_use),
    ));
}

#[test]
fn shield_block_damages_active_shield_stack() {
    let mut state = shield_item_state();
    let slot = PlayerInventory::HOTBAR_BASE;
    state.inventory.slots[slot] = ItemStack::new(77, 1);
    state.shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::MainHand,
        slot,
        state.inventory.slots[slot].clone(),
        1,
        true,
    );

    let changed = damage_active_shield(&mut state, 3.75).expect("shield should take durability");

    assert_eq!(changed, (slot, ItemStack::new(77, 1).with_damage(4)));
    assert_eq!(
        state.inventory.slots[slot],
        ItemStack::new(77, 1).with_damage(4)
    );
    assert_eq!(
        state.shield_use.as_ref().unwrap().stack,
        state.inventory.slots[slot]
    );
}

#[test]
fn shield_block_removes_broken_active_shield() {
    let mut state = shield_item_state();
    state.inventory.slots[45] = ItemStack::new(77, 1).with_damage(SHIELD_FALLBACK_MAX_DAMAGE - 4);
    state.shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::OffHand,
        45,
        state.inventory.slots[45].clone(),
        1,
        true,
    );

    let changed = damage_active_shield(&mut state, 3.0).expect("shield break should update slot");

    assert_eq!(changed, (45, ItemStack::EMPTY));
    assert_eq!(state.inventory.slots[45], ItemStack::EMPTY);
    assert!(state.shield_use.is_none());
}

#[test]
fn permitted_game_mode_transition_clears_active_shield_immediately() {
    let mut state = shield_item_state();
    let slot = PlayerInventory::HOTBAR_BASE;
    state.inventory.slots[slot] = ItemStack::new(77, 1);
    state.shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::MainHand,
        slot,
        state.inventory.slots[slot].clone(),
        1,
        true,
    );

    super::command_execution::prepare_game_mode_transition(
        Some(&mut state),
        GameMode::Survival,
        GameMode::Creative,
        CommandPermissions::from_op(true),
    );

    assert!(state.shield_use.is_none());
}

#[test]
fn denied_or_noop_game_mode_transition_keeps_active_shield() {
    let mut state = shield_item_state();
    let slot = PlayerInventory::HOTBAR_BASE;
    state.inventory.slots[slot] = ItemStack::new(77, 1);
    state.shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::MainHand,
        slot,
        state.inventory.slots[slot].clone(),
        1,
        true,
    );

    super::command_execution::prepare_game_mode_transition(
        Some(&mut state),
        GameMode::Survival,
        GameMode::Creative,
        CommandPermissions::from_op(false),
    );
    assert!(state.shield_use.is_some());

    super::command_execution::prepare_game_mode_transition(
        Some(&mut state),
        GameMode::Survival,
        GameMode::Survival,
        CommandPermissions::from_op(true),
    );
    assert!(state.shield_use.is_some());
}

#[tokio::test]
async fn projectile_shield_block_writes_scaled_slot_update() {
    let mut state = shield_item_state();
    state.sessions.set_world_time(10);
    let slot = PlayerInventory::HOTBAR_BASE;
    state.inventory.slots[slot] = ItemStack::new(77, 1);
    state.shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::MainHand,
        slot,
        state.inventory.slots[slot].clone(),
        1,
        true,
    );
    let mut survival_state = SurvivalState::FULL;
    let mut xp_state = XpState::default();
    let (simulation_stop, simulation_task) =
        start_survival_test_owner(&mut state, "ProjectileShield", survival_state, &xp_state);
    let mut writer = Vec::new();

    let damage_applied = apply_player_damage(
        Some(&mut state),
        &mut writer,
        Compression::Disabled,
        &mut survival_state,
        &mut xp_state,
        GameMode::Survival,
        PlayerDamageApplication {
            player_pose: PlayerPose::new(0.0, 64.0, 0.0),
            request: PlayerDamageRequest {
                kind: PlayerDamageKind::Projectile,
                amount: 4.2,
                source_origin: Some(Vec3::new(0.0, 64.0, 2.0)),
            },
        },
    )
    .await
    .unwrap();
    simulation_stop.send(()).unwrap();
    simulation_task.await.unwrap();

    assert_eq!(survival_state.health, SurvivalState::FULL.health);
    assert!(
        !damage_applied,
        "a shielded hit must not authorize knockback"
    );
    assert_eq!(
        state.inventory.slots[slot],
        ItemStack::new(77, 1).with_damage(5)
    );
    let packets = decode_container_set_slot_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].slot, slot as i16);
    assert_eq!(packets[0].item_stack, ItemStack::new(77, 1).with_damage(5));
}

#[tokio::test]
async fn authoritative_pvp_shield_block_refreshes_local_identity_before_retry() {
    let mut state = shield_item_state();
    state.sessions.set_world_time(10);
    let shield_slot = PlayerInventory::HOTBAR_BASE;
    state.inventory.slots[shield_slot] = ItemStack::new(77, 1);
    state.shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::MainHand,
        shield_slot,
        state.inventory.slots[shield_slot].clone(),
        1,
        true,
    );
    let mut survival_state = SurvivalState::FULL;
    let mut xp_state = XpState::default();
    let (session_id, _) = register_survival_test_player(
        &mut state,
        "ProjectileShieldCasRace",
        survival_state,
        &xp_state,
    );
    let attacker_profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("ProjectileShieldCasAttacker"),
        name: "ProjectileShieldCasAttacker".to_owned(),
    };
    let attacker_pose = PlayerPose::new(0.5, 64.0, 2.5);
    let (attacker_tx, _attacker_rx) = mpsc::channel(8);
    let (attacker_session, _) = state.sessions.register(
        &attacker_profile,
        (0, 0),
        0,
        HashSet::new(),
        attacker_tx,
        attacker_pose,
    );
    state.sessions.register_player_persistence(
        attacker_session,
        Arc::new(Mutex::new(PlayerPersistedState::new_default(attacker_pose))),
    );
    let (simulation, mut owner) = simulation_channel();
    state.simulation = simulation.for_session(session_id);

    let sessions = Arc::clone(&state.sessions);
    let owner_sessions = Arc::clone(&sessions);
    let (request_queued_tx, request_queued_rx) = tokio::sync::oneshot::channel();
    let (process_request_tx, process_request_rx) = tokio::sync::oneshot::channel();
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();
    let owner_task = tokio::spawn(async move {
        assert!(owner.wait_for_command().await);
        request_queued_tx
            .send(())
            .expect("shield commit waiter remains active");
        process_request_rx
            .await
            .expect("test releases the queued shield commit");
        owner.process_tick(&owner_sessions, SIMULATION_COMMAND_BATCH_LIMIT);
        loop {
            tokio::select! {
                biased;
                _ = &mut stop_rx => {
                    owner.shutdown();
                    break;
                }
                ready = owner.wait_for_command() => {
                    if !ready {
                        break;
                    }
                    owner.process_tick(&owner_sessions, SIMULATION_COMMAND_BATCH_LIMIT);
                }
            }
        }
    });

    let mut writer = Vec::new();
    let damage_applied = {
        let damage = apply_player_damage(
            Some(&mut state),
            &mut writer,
            Compression::Disabled,
            &mut survival_state,
            &mut xp_state,
            GameMode::Survival,
            PlayerDamageApplication {
                player_pose: PlayerPose::new(0.0, 64.0, 0.0),
                request: PlayerDamageRequest {
                    kind: PlayerDamageKind::Projectile,
                    amount: 4.2,
                    source_origin: Some(Vec3::new(0.0, 64.0, 2.0)),
                },
            },
        );
        tokio::pin!(damage);
        tokio::select! {
            reached = request_queued_rx => {
                reached.expect("owner observes the queued shield commit");
            }
            result = &mut damage => {
                panic!("shield damage completed before its owner CAS race: {result:?}");
            }
        }

        let pvp_result = sessions.player_attack_entity(
            &simulation::SimulationAuthority::for_test(),
            session::PlayerEntityAttack {
                attacker_session,
                entity_id: EntityId(i32::try_from(session_id).unwrap()),
                amount: 4.0,
                attacker_costs: None,
                authority_tick: sessions.simulation_tick(),
            },
        );
        assert!(matches!(
            pvp_result,
            PlayerAttackResult::Damaged(outcome)
                if matches!(
                    *outcome,
                    EntityAttackOutcome::PlayerDamaged {
                        damage_applied: false,
                        ..
                    }
                )
        ));
        process_request_tx
            .send(())
            .expect("release the queued shield commit");

        damage.await.unwrap()
    };
    stop_tx.send(()).unwrap();
    owner_task.await.unwrap();

    assert!(
        !damage_applied,
        "the successfully retried shield blocks the hit"
    );
    assert_eq!(survival_state.health, SurvivalState::FULL.health);
    assert_eq!(
        state.inventory.slots[shield_slot],
        ItemStack::new(77, 1).with_damage(10)
    );
    assert_eq!(
        state.shield_use.as_ref().map(|shield| &shield.stack),
        Some(&state.inventory.slots[shield_slot]),
        "the rejected stale attempt must not clear the still-authoritative shield"
    );
    let packets = decode_container_set_slot_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].slot, shield_slot as i16);
    assert_eq!(packets[0].item_stack, state.inventory.slots[shield_slot]);
}

#[tokio::test]
async fn repeated_shield_cas_conflict_refreshes_owner_state_and_fails_closed() {
    let mut state = shield_item_state();
    state.sessions.set_world_time(10);
    let shield_slot = PlayerInventory::HOTBAR_BASE;
    let first_changed_slot = 10;
    let second_changed_slot = 11;
    state.inventory.slots[shield_slot] = ItemStack::new(77, 1);
    state.shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::MainHand,
        shield_slot,
        state.inventory.slots[shield_slot].clone(),
        1,
        true,
    );
    let mut survival_state = SurvivalState::FULL;
    let mut xp_state = XpState::default();
    let (session_id, persisted) = register_survival_test_player(
        &mut state,
        "ProjectileShieldRepeatedCasRace",
        survival_state,
        &xp_state,
    );
    let (simulation, mut owner) = simulation_channel();
    state.simulation = simulation.for_session(session_id);

    let sessions = Arc::clone(&state.sessions);
    let (first_queued_tx, first_queued_rx) = tokio::sync::oneshot::channel();
    let (process_first_tx, process_first_rx) = tokio::sync::oneshot::channel();
    let (second_queued_tx, second_queued_rx) = tokio::sync::oneshot::channel();
    let (process_second_tx, process_second_rx) = tokio::sync::oneshot::channel();
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();
    let owner_task = tokio::spawn(async move {
        assert!(owner.wait_for_command().await);
        first_queued_tx
            .send(())
            .expect("first shield commit waiter remains active");
        process_first_rx
            .await
            .expect("test releases the first shield commit");
        owner.process_tick(&sessions, SIMULATION_COMMAND_BATCH_LIMIT);

        assert!(owner.wait_for_command().await);
        second_queued_tx
            .send(())
            .expect("retry shield commit waiter remains active");
        process_second_rx
            .await
            .expect("test releases the retry shield commit");
        owner.process_tick(&sessions, SIMULATION_COMMAND_BATCH_LIMIT);

        loop {
            tokio::select! {
                biased;
                _ = &mut stop_rx => {
                    owner.shutdown();
                    break;
                }
                ready = owner.wait_for_command() => {
                    if !ready {
                        break;
                    }
                    owner.process_tick(&sessions, SIMULATION_COMMAND_BATCH_LIMIT);
                }
            }
        }
    });

    let mut writer = Vec::new();
    let error = {
        let damage = apply_player_damage(
            Some(&mut state),
            &mut writer,
            Compression::Disabled,
            &mut survival_state,
            &mut xp_state,
            GameMode::Survival,
            PlayerDamageApplication {
                player_pose: PlayerPose::new(0.0, 64.0, 0.0),
                request: PlayerDamageRequest {
                    kind: PlayerDamageKind::Projectile,
                    amount: 4.2,
                    source_origin: Some(Vec3::new(0.0, 64.0, 2.0)),
                },
            },
        );
        tokio::pin!(damage);

        tokio::select! {
            reached = first_queued_rx => {
                reached.expect("owner observes the first shield commit");
            }
            result = &mut damage => {
                panic!("shield damage completed before the first owner conflict: {result:?}");
            }
        }
        persisted.lock().unwrap().inventory.slots[first_changed_slot] = ItemStack::new(91, 1);
        process_first_tx
            .send(())
            .expect("release the first shield commit");

        tokio::select! {
            reached = second_queued_rx => {
                reached.expect("owner observes the bounded shield retry");
            }
            result = &mut damage => {
                panic!("shield damage completed before the retry owner conflict: {result:?}");
            }
        }
        persisted.lock().unwrap().inventory.slots[second_changed_slot] = ItemStack::new(92, 1);
        process_second_tx
            .send(())
            .expect("release the retry shield commit");

        damage
            .await
            .expect_err("a repeated exact-owner conflict must fail closed")
    };
    stop_tx.send(()).unwrap();
    owner_task.await.unwrap();

    assert!(matches!(
        error,
        ConnectionError::RuntimeUnavailable {
            operation: "committing shield durability after repeated owner state change"
        }
    ));
    assert_eq!(
        state.inventory.slots[first_changed_slot],
        ItemStack::new(91, 1)
    );
    assert_eq!(
        state.inventory.slots[second_changed_slot],
        ItemStack::new(92, 1)
    );
    assert_eq!(state.inventory.slots[shield_slot], ItemStack::new(77, 1));
    assert_eq!(
        state.shield_use.as_ref().map(|shield| &shield.stack),
        Some(&state.inventory.slots[shield_slot])
    );
    assert_eq!(survival_state, SurvivalState::FULL);
    assert!(writer.is_empty());
}

async fn campfire_test_interaction_state(pos: mc_world::BlockPos) -> InteractionState {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            BlockReport {
                id: Identifier::parse("minecraft:campfire").unwrap(),
                properties: prop_schema(&[("lit", &["true", "false"])]),
                states: vec![
                    state(1, true, &[("lit", "true")]),
                    state(2, false, &[("lit", "false")]),
                ],
            },
        ])
        .unwrap(),
    );
    let storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let world_read = storage.read_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    {
        let mut storage = world.lock().await;
        let cpos = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage.set_block_at(pos, BlockStateId(1)).unwrap();
    }
    let items = Arc::new(ItemRegistry::from_report(&[]));
    let item_to_block = ItemToBlockTable::build(&items, &blocks);
    InteractionState {
        world,
        world_read,
        blocks,
        block_light: None,
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        water: None,
        sessions: Arc::new(SessionRegistry::new()),
        simulation: simulation_channel().0,
        session_id: 1,
        workspace: LightWorkspace::new(),
        light_cache: LightCache::new(),
        compression: Compression::Disabled,
        selected_hotbar_slot: 0,
        inventory: PlayerInventory::empty(),
        carried_item: ItemStack::EMPTY,
        player_persistence: Arc::new(Mutex::new(PlayerPersistedState::new_default(
            PlayerPose::new(0.5, 64.0, 0.5),
        ))),
        inventory_state_id: 1,
        inventory_quickcraft: QuickCraftState::default(),
        items,
        item_facts: Arc::new(ItemFactsTable::default()),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        item_to_block,
        tags: Arc::new(TagsData::default()),
        recipes: Vec::new(),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        script_zones: None,
        next_container_id: FURNACE_CONTAINER_ID_MIN,
        active_container: None,
        pending_break: Some(PendingBreak {
            sequence: 0,
            position: pack_block_pos(pos.x, pos.y, pos.z),
            direction: Direction::Up,
            started_tick: 0,
            started_progress_per_tick: 0.01,
            held_hotbar_slot: 0,
            held_item: None,
            expected_target: None,
            stop_received: false,
        }),
        delayed_break: None,
        pending_use: None,
        pending_sign_edit: None,
        shield_use: None,
        last_entity_attack_tick: None,
    }
}

#[tokio::test]
async fn player_collision_allows_lit_campfire_overlap_for_contact_damage() {
    let state = campfire_test_interaction_state(mc_world::BlockPos { x: 0, y: 64, z: 0 }).await;

    assert!(!player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.0, 0.5)).await);
}

#[tokio::test]
async fn lit_campfire_contact_damage_uses_survival_death_path() {
    let mut state = campfire_test_interaction_state(mc_world::BlockPos { x: 0, y: 64, z: 0 }).await;
    let mut survival_state = SurvivalState {
        health: 1.0,
        ..SurvivalState::FULL
    };
    let mut xp_state = XpState {
        level: 5,
        progress: 0.0,
        total: 55,
        seed: 0,
    };
    let (simulation_stop, simulation_task) =
        start_survival_test_owner(&mut state, "CampfireDeath", survival_state, &xp_state);
    let mut writer = Vec::new();

    apply_contact_block_damage(
        Some(&mut state),
        &mut writer,
        Compression::Disabled,
        &mut survival_state,
        &mut xp_state,
        GameMode::Survival,
        PlayerPose::new(0.5, 65.0, 0.5),
    )
    .await
    .unwrap();
    simulation_stop.send(()).unwrap();
    simulation_task.await.unwrap();

    assert!(survival_state.is_dead());
    assert!(state.pending_break.is_none());
    assert_eq!(xp_state.total, 0);
    assert!(!writer.is_empty());
}

#[tokio::test]
async fn committed_campfire_death_survives_client_write_failure() {
    let mut state = campfire_test_interaction_state(mc_world::BlockPos { x: 0, y: 64, z: 0 }).await;
    let mut deaths = state.sessions.install_script_commit_event_outbox();
    let mut survival_state = SurvivalState {
        health: 1.0,
        ..SurvivalState::FULL
    };
    let mut xp_state = XpState::default();
    let (simulation_stop, simulation_task) =
        start_survival_test_owner(&mut state, "CampfireFail", survival_state, &xp_state);
    let (mut writer, reader) = tokio::io::duplex(64);
    drop(reader);

    let result = apply_contact_block_damage(
        Some(&mut state),
        &mut writer,
        Compression::Disabled,
        &mut survival_state,
        &mut xp_state,
        GameMode::Survival,
        PlayerPose::new(0.5, 65.0, 0.5),
    )
    .await;
    simulation_stop.send(()).unwrap();
    simulation_task.await.unwrap();

    assert!(
        result.is_err(),
        "closed client transport must reject publication"
    );
    assert!(survival_state.is_dead());
    let event = deaths
        .try_recv()
        .expect("owner commit must publish death before client transport");
    assert!(matches!(
        event.kind(),
        mc_script::ScriptEventKind::PlayerDied {
            context,
            game_mode: mc_script::ScriptGameMode::Survival,
            ..
        } if context.username() == "CampfireFail"
    ));
    assert!(matches!(
        deaths.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn lit_campfire_contact_damage_uses_player_width_edge_overlap() {
    let mut state = campfire_test_interaction_state(mc_world::BlockPos { x: 0, y: 64, z: 0 }).await;
    let mut survival_state = SurvivalState::FULL;
    let mut xp_state = XpState::default();
    let (simulation_stop, simulation_task) =
        start_survival_test_owner(&mut state, "CampfireEdge", survival_state, &xp_state);
    let mut writer = Vec::new();

    apply_contact_block_damage(
        Some(&mut state),
        &mut writer,
        Compression::Disabled,
        &mut survival_state,
        &mut xp_state,
        GameMode::Survival,
        PlayerPose::new(1.3, 64.0, 0.5),
    )
    .await
    .unwrap();
    simulation_stop.send(()).unwrap();
    simulation_task.await.unwrap();

    assert_eq!(survival_state.health, 19.0);
    assert!(!writer.is_empty());
}

#[tokio::test]
async fn pushed_hostile_damage_shield_block_writes_break_clear_slot_update() {
    let mut state = shield_item_state();
    state.sessions.set_world_time(10);
    let slot = 45;
    state.inventory.slots[slot] = ItemStack::new(77, 1).with_damage(SHIELD_FALLBACK_MAX_DAMAGE - 4);
    state.shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::OffHand,
        slot,
        state.inventory.slots[slot].clone(),
        1,
        true,
    );
    let mut survival_state = SurvivalState::FULL;
    let mut xp_state = XpState::default();
    let (simulation_stop, simulation_task) =
        start_survival_test_owner(&mut state, "HostileShield", survival_state, &xp_state);
    let mut writer = Vec::new();

    let damage_applied = apply_player_damage(
        Some(&mut state),
        &mut writer,
        Compression::Disabled,
        &mut survival_state,
        &mut xp_state,
        GameMode::Survival,
        PlayerDamageApplication {
            player_pose: PlayerPose::new(0.0, 64.0, 0.0),
            request: PlayerDamageRequest {
                kind: PlayerDamageKind::MobAttack,
                amount: 3.0,
                source_origin: Some(Vec3::new(0.0, 64.0, 1.0)),
            },
        },
    )
    .await
    .unwrap();
    simulation_stop.send(()).unwrap();
    simulation_task.await.unwrap();

    assert_eq!(survival_state.health, SurvivalState::FULL.health);
    assert_eq!(state.inventory.slots[slot], ItemStack::EMPTY);
    assert!(state.shield_use.is_none());
    assert!(
        !damage_applied,
        "a shielded hit must not authorize knockback"
    );
    let packets = decode_container_set_slot_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].slot, slot as i16);
    assert_eq!(packets[0].item_stack, ItemStack::EMPTY);
}

#[tokio::test]
async fn pushed_hostile_damage_uses_equipped_iron_chestplate_and_damages_armor() {
    let chestplate = Identifier::parse("minecraft:iron_chestplate").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: chestplate,
        protocol_id: 11,
    }]));
    let mut state = interaction_state_for_items(items);
    state.sessions.set_world_time(10);
    state.inventory.slots[6] = ItemStack::new(11, 1);
    let mut survival_state = SurvivalState::FULL;
    let mut xp_state = XpState::default();
    let (simulation_stop, simulation_task) =
        start_survival_test_owner(&mut state, "HostileArmor", survival_state, &xp_state);
    let mut writer = Vec::new();

    let damage_applied = apply_player_damage(
        Some(&mut state),
        &mut writer,
        Compression::Disabled,
        &mut survival_state,
        &mut xp_state,
        GameMode::Survival,
        PlayerDamageApplication {
            player_pose: PlayerPose::new(0.0, 64.0, 0.0),
            request: PlayerDamageRequest {
                kind: PlayerDamageKind::MobAttack,
                amount: 3.0,
                source_origin: Some(Vec3::new(0.0, 64.0, 1.0)),
            },
        },
    )
    .await
    .unwrap();
    simulation_stop.send(()).unwrap();
    simulation_task.await.unwrap();

    assert!((survival_state.health - 17.54).abs() < 0.001);
    assert!(
        damage_applied,
        "committed positive damage authorizes knockback"
    );
    assert_eq!(
        state.inventory.slots[6],
        ItemStack::new(11, 1).with_damage(1)
    );
    let packets = decode_container_set_slot_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].slot, 6);
    assert_eq!(packets[0].item_stack, ItemStack::new(11, 1).with_damage(1));
}

#[test]
fn grounded_player_melee_knockback_matches_vanilla_base_impulse() {
    let knockback = melee_knockback(0.0, 2.0, true, Vec3::new(0.0, 64.0, 0.0))
        .expect("distinct horizontal positions produce knockback");
    let motion = player_melee_knockback(knockback);

    assert!(motion.x.abs() < f64::EPSILON);
    assert!((motion.y - 0.4).abs() < f64::EPSILON);
    assert!((motion.z - 0.400_000_005_960_464_5).abs() < f64::EPSILON);
}

#[test]
fn player_melee_knockback_fails_closed_for_zero_horizontal_direction() {
    assert_eq!(
        melee_knockback(0.0, 0.0, true, Vec3::new(0.0, 64.0, 0.0)),
        None
    );
}

#[test]
fn shield_block_knockback_matches_vanilla_base_response() {
    let knockback = shield_block_knockback(0.0, 2.0, true, Vec3::new(0.0, 64.0, 0.0))
        .expect("distinct horizontal positions produce shield response");
    let motion = player_melee_knockback(knockback);

    assert!(motion.x.abs() < f64::EPSILON);
    assert!((motion.y - 0.4).abs() < f64::EPSILON);
    assert!((motion.z - 0.5).abs() < f64::EPSILON);
}

#[test]
fn older_victim_publication_preserves_newer_attacker_costs() {
    let (mut state, sword, _) = attack_strength_test_state();
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(sword, 1).with_damage(1);
    state.inventory.slots[5] = ItemStack::new(42, 1);
    let mut survival = SurvivalState {
        exhaustion: 0.1,
        ..SurvivalState::FULL
    };
    let mut xp = XpState::default();

    let applied = apply_player_damage_publication(
        Some(&mut state),
        &mut survival,
        &mut xp,
        PlayerDamagePublication {
            expected_health: SurvivalState::MAX_HEALTH,
            health: 16.0,
            inventory: vec![PlayerInventorySlotDelta {
                slot: 5,
                expected: ItemStack::new(42, 1),
                updated: ItemStack::new(42, 1).with_damage(2),
            }],
            carried_item: None,
            xp: None,
            died: false,
            fresh_hurt: true,
            shield_blocked: false,
            shield_cooldown: None,
            knockback: None,
        },
    );

    assert!(applied.survival_changed);
    assert_eq!(survival.health, 16.0);
    assert_eq!(survival.exhaustion, 0.1);
    assert_eq!(state.inventory.held(0).unwrap().damage, Some(1));
    assert_eq!(state.inventory.slots[5].damage, Some(2));
}

#[test]
fn stale_damage_publication_does_not_apply_health_side_effects() {
    let mut survival = SurvivalState {
        health: 18.0,
        ..SurvivalState::FULL
    };
    let mut xp = XpState::default();
    let knockback = melee_knockback(0.0, 2.0, true, Vec3::new(0.0, 64.0, 0.0))
        .expect("distinct horizontal positions produce knockback");

    let applied = apply_player_damage_publication(
        None,
        &mut survival,
        &mut xp,
        PlayerDamagePublication {
            expected_health: SurvivalState::MAX_HEALTH,
            health: 0.0,
            inventory: Vec::new(),
            carried_item: None,
            xp: None,
            died: true,
            fresh_hurt: true,
            shield_blocked: false,
            shield_cooldown: None,
            knockback: Some(knockback),
        },
    );

    assert_eq!(survival.health, 18.0);
    assert!(!applied.survival_changed);
    assert!(!applied.died);
    assert!(!applied.fresh_hurt);
    assert_eq!(applied.knockback, None);
}

include!("tests/contact_damage.rs");
include!("tests/gamerule_keep_inventory.rs");
include!("tests/inventory_and_survival.rs");
include!("tests/spawning_and_world.rs");
