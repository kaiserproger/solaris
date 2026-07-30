use super::block_placement::plan_block_placement;
use super::command_execution::{DebugCommandContext, debug_water_corridor_edits};
use super::containers::furnace_fuel_ticks;
use super::falling_blocks::{FallingBlockStart, LandedFallingBlock, plan_falling_block_starts};
use super::use_item_on_adapter::cursor_y_relative_to_target;
use super::*;
use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use crate::play::chunk_stream::{hostile_chunk_spawns, passive_chunk_spawns, prioritized_spiral};
use mc_data::blocks::{BlockReport, BlockStateReport, solaris_required_blocks_report};
use mc_data::items::ItemReport;
use mc_world::light::compute_chunk_light_in;
use tokio::io::{AsyncWrite, AsyncWriteExt};

use self::attack_pvp::attack_strength_test_state;

mod attack_pvp;
mod bed_sleep;
mod block_placement_nbt;
mod block_resync;
mod button_planning;
mod button_runtime_edges;
mod campfire_cooking;
mod chest;
mod container_safety;
mod door_toggles;
mod enchanting_recipe_settlement;
mod furnace;
mod inventory_settlement;
mod leaf_distance_ticks;
mod natural_random_ticks;
mod pickup;
mod plants;
mod redstone_pistons;
mod scheduled_buttons;
mod scheduled_hoppers;
mod stonecutter;

use stonecutter::{stonecutter_test_items, stonecutter_test_recipe};

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

    sessions.set_daylight_cycle_enabled(false);
    sessions.advance_world_time(5);
    let frozen = clientbound_session_world_time(&sessions);
    assert_eq!(frozen.game_time, 12);
    assert_eq!(
        frozen.overworld_clock,
        Some(mc_protocol::packets::play::WorldClockUpdate {
            total_ticks: 12_352,
            partial_tick: 0.0,
            rate: 0.0,
        })
    );

    sessions.set_daylight_cycle_enabled(true);
    sessions.advance_world_time(3);
    assert_eq!(sessions.world_time(), 12_355);
    assert_eq!(sessions.simulation_tick(), 15);

    assert_eq!(clientbound_world_time(u64::MAX, 1, 1.0).game_time, i64::MAX);
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
        parse_admin_command("/gamerule do_daylight_cycle", op),
        Ok(AdminCommand::DaylightCycle(None))
    );
    assert_eq!(
        parse_admin_command("/gamerule do_daylight_cycle false", op),
        Ok(AdminCommand::DaylightCycle(Some(false)))
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
            "Usage: /gamerule <do_daylight_cycle|keep_inventory|players_sleeping_percentage> [value]"
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
    assert_eq!(tree.nodes[20].children, vec![21, 23, 25]);
    assert_eq!(
        tree.nodes[23],
        mc_protocol::packets::play::CommandNode::literal("do_daylight_cycle", vec![24], true,)
            .restricted(true)
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
    assert_eq!(
        command_suggestions("/gamerule d", op).suggestions,
        vec!["do_daylight_cycle".to_string()]
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
