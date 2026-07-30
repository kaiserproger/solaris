use super::block_placement::plan_block_placement;
use super::command_execution::{DebugCommandContext, debug_water_corridor_edits};
use super::containers::furnace_fuel_ticks;
use super::use_item_on_adapter::cursor_y_relative_to_target;
use super::*;
use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::play::chunk_stream::{hostile_chunk_spawns, passive_chunk_spawns, prioritized_spiral};
use mc_data::blocks::{BlockReport, BlockStateReport, solaris_required_blocks_report};
use mc_data::items::ItemReport;
use mc_world::light::compute_chunk_light_in;
use tokio::io::{AsyncWrite, AsyncWriteExt};

use self::attack_pvp::attack_strength_test_state;

mod admin_commands;
mod arrow_launch;
mod attack_pvp;
mod bed_sleep;
mod block_drop_configured_loot;
mod block_drop_short_grass;
mod block_placement_nbt;
mod block_placement_planning;
mod block_resync;
mod bottom_slab_collision;
mod button_planning;
mod button_runtime_edges;
mod campfire_cooking;
mod chest;
mod client_view_distance;
mod collision_correction_entry;
mod collision_correction_escape;
mod container_inventory;
mod container_safety;
mod crafting_table_open;
mod death_xp;
mod debug_commands;
mod door_toggles;
mod enchanting_recipe_settlement;
mod entity_tick_cadence;
mod fake_farmland_identity_collision;
mod falling_blocks;
mod farmland_fallback_collision;
mod fence_deflation_boundary;
mod fluid_runtime;
mod furnace;
mod gamemode_commands;
mod inventory_settlement;
mod item_block_mapping;
mod leaf_distance_ticks;
mod movement_block_reads;
mod natural_random_ticks;
mod oracle_aabb_deflation_boundary;
mod oriented_stair_collision;
mod pending_teleport_confirm_behaviour;
mod pending_teleport_matching_confirm;
mod pending_teleport_movement_gate;
mod pending_teleport_movement_guard;
mod pending_teleport_resend;
mod pending_teleport_unexpected_confirm;
mod pickup;
mod plants;
mod play_custom_payload;
mod player_damage;
mod powder_snow_collision_correction;
mod powder_snow_dynamic_shape;
mod powder_snow_equipment_context;
mod powder_snow_long_fall;
mod real_door_sidecar;
mod redstone_pistons;
mod scheduled_buttons;
mod scheduled_hoppers;
mod shield;
mod stale_container_updates;
mod stone_full_cube_collision;
mod stonecutter;
mod synthetic_slab_identity_collision;
mod tall_narrow_fence_collision;
mod teleport_command_pending_confirmation;
mod toggle_planning;
mod top_slab_collision;
mod torch_campfire_collision;
mod unrelated_state_collision;
mod use_item_on_preflight;
mod wrong_property_slab_collision;

use stonecutter::{stonecutter_test_items, stonecutter_test_recipe};
use use_item_on_preflight::test_use_item_on;

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

include!("tests/contact_damage.rs");
include!("tests/gamerule_keep_inventory.rs");
include!("tests/inventory_and_survival.rs");
include!("tests/spawning_and_world.rs");
