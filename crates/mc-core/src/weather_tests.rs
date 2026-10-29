use std::sync::Arc;

use super::*;
use crate::boundary::{ConnectionId, PlayerInput};
use crate::journal::{
    CheckpointManifest, JournalEntry, JournalWriter, Replay, read_complete_batches_after,
    replay_batches,
};
use crate::native_checkpoint::tests::{read_manifest, write_manifest};
use crate::registry::CoreRegistry;
use crate::tick::run_tick;
use crate::world::ChunkWorld;

fn fixture() -> TickState {
    let registry = Arc::new(CoreRegistry::load(None).expect("registry"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let world = ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 80, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    TickState::new(registry, world, 1, 512)
}

fn weather_tick(state: &mut TickState) {
    let previous = view(state);
    state.tick += 1;
    advance_cycle(state);
    fade_and_publish(state, previous);
}

#[test]
fn weather_levels_follow_old_f32_steps_and_thunder_fades_independently() {
    let mut state = fixture();
    set_command(&mut state, Weather::Rain, Some(5_000));
    assert_eq!((state.rain_level, state.thunder_level), (0.0, 0.0));
    let mut expected = 0.0_f32;
    for _ in 0..100 {
        weather_tick(&mut state);
        expected = (expected + 0.01).min(1.0);
        assert_eq!(state.rain_level.to_bits(), expected.to_bits());
        assert_eq!(state.thunder_level, 0.0);
    }
    assert!(state.rain_level >= 0.99);
    // The old oracle records 101 updates: f32 addition reaches the clamp
    // one tick after the nominal 100-tick ramp. Do not round away its bits.
    weather_tick(&mut state);
    assert_eq!(state.rain_level, 1.0);

    set_command(&mut state, Weather::Thunder, Some(5_000));
    assert_eq!(state.thunder_level, 0.0);
    weather_tick(&mut state);
    assert_eq!(state.thunder_level, 0.01);
    assert_eq!(state.rain_level, 1.0);
    for _ in 0..100 {
        weather_tick(&mut state);
    }
    assert_eq!(state.thunder_level, 1.0);

    set_command(&mut state, Weather::Rain, Some(5_000));
    assert_eq!(state.thunder_level, 1.0);
    weather_tick(&mut state);
    assert_eq!(state.rain_level, 1.0);
    assert_eq!(state.thunder_level, 1.0 - 0.01);
    for _ in 0..100 {
        weather_tick(&mut state);
    }
    assert_eq!(state.thunder_level, 0.0);
    assert_eq!(state.rain_level, 1.0);

    set_command(&mut state, Weather::Clear, Some(5_000));
    assert_eq!(state.rain_level, 1.0);
    for _ in 0..101 {
        weather_tick(&mut state);
    }
    assert_eq!((state.rain_level, state.thunder_level), (0.0, 0.0));
}

#[test]
fn frozen_cycle_retains_command_duration_but_both_levels_still_fade() {
    let mut state = fixture();
    state
        .gamerules
        .insert("advance_weather".into(), "false".into());
    state.rain_level = 0.37;
    state.thunder_level = 0.12;
    set_command(&mut state, Weather::Thunder, Some(3));
    assert_eq!((state.rain_level, state.thunder_level), (0.37, 0.12));
    for _ in 0..101 {
        weather_tick(&mut state);
        assert_eq!(state.weather, (Weather::Thunder, 3));
    }
    assert_eq!((state.rain_level, state.thunder_level), (1.0, 1.0));
    state
        .gamerules
        .insert("advance_weather".into(), "true".into());
    weather_tick(&mut state);
    assert_eq!(state.weather, (Weather::Thunder, 2));
    weather_tick(&mut state);
    assert_eq!(state.weather, (Weather::Thunder, 1));
    weather_tick(&mut state);
    assert_eq!(state.weather.0, Weather::Clear);
    assert!((12_000..=180_000).contains(&state.weather.1));
    assert_eq!((state.rain_level, state.thunder_level), (0.99, 0.99));
}

#[test]
fn natural_cycle_returns_wet_weather_to_clear() {
    let mut state = fixture();
    for (initial, next, range) in [
        (Weather::Rain, Weather::Clear, 12_000..=180_000),
        (Weather::Thunder, Weather::Clear, 12_000..=180_000),
    ] {
        state.weather = (initial, 1);
        weather_tick(&mut state);
        assert_eq!(state.weather.0, next);
        assert!(range.contains(&state.weather.1));
    }
}

#[test]
fn tick_snapshots_and_reliable_changes_use_real_levels_with_a_frozen_day_clock() {
    let mut state = fixture();
    state
        .gamerules
        .insert("advance_time".into(), "false".into());
    state
        .gamerules
        .insert("advance_weather".into(), "false".into());
    state.day_time = 87_654;
    let connection = ConnectionId(31);
    let _ = run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection,
            uuid: uuid::Uuid::new_v4(),
            name: "weather".into(),
            permission_level: 0,
        }],
    );
    let (output, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::Command {
            connection,
            command: "weather thunder 5s".into(),
            permission_level: 4,
        }],
    );
    assert_eq!(state.weather, (Weather::Thunder, 100));
    assert_eq!(output.snapshot.day_time, 87_654);
    assert_eq!(output.snapshot.day_time_rate, 0.0);
    assert!(output.snapshot.raining);
    assert_eq!(output.snapshot.rain_level, 0.01);
    assert_eq!(output.snapshot.thunder_level, 0.01);
    let weather: Vec<_> = drains
        .iter()
        .filter(|(recipient, _)| *recipient == connection)
        .flat_map(|(_, changes)| changes)
        .filter(|change| matches!(change, PlayerChange::WeatherChanged { .. }))
        .collect();
    assert_eq!(weather.len(), 1, "command and fade share one publication");
    assert!(matches!(weather[0], PlayerChange::WeatherChanged {
        raining: true, rain_level, thunder_level,
    } if *rain_level == 0.01 && *thunder_level == 0.01));
    let (output, _, _) = run_tick(&mut state, &[]);
    assert_eq!(output.snapshot.rain_level, 0.02);
    assert_eq!(output.snapshot.thunder_level, 0.02);
    assert_eq!(output.snapshot.day_time, 87_654);
    assert_eq!(state.weather, (Weather::Thunder, 100));
}

#[test]
fn checkpoint_and_tail_fold_restore_exact_clock_duration_and_float_bits() {
    let dir = std::env::temp_dir().join(format!("mc-core-weather-fold-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("dir");
    let config = crate::CoreConfig::new(dir.clone(), 42);
    let mut state = fixture();
    state.day_time = 93_211;
    state.weather = (Weather::Thunder, 432);
    state.thundering = true;
    state.thunder_remaining_ticks = 432;
    state.rain_level = 0.37123457;
    state.thunder_level = 0.12876543;
    state
        .gamerules
        .insert("advance_time".into(), "false".into());
    state
        .gamerules
        .insert("advance_weather".into(), "false".into());
    let base_image = ClimateImage::capture(&state);
    let mut writer = JournalWriter::open(&dir, 1).expect("writer");
    writer
        .append(1, 1, &[JournalEntry::Climate { value: base_image }])
        .expect("base WAL");
    writer.gate(0, 1).expect("base gate");
    write_manifest(
        &config,
        &CheckpointManifest {
            tick: 1,
            players: Default::default(),
            world_spawn: Some([0, 80, 0]),
            world_spawn_rotation: [0.0; 2],
            generated_entity_claims: Default::default(),
            block_ticks: Default::default(),
            climate: Some(base_image),
            ..Default::default()
        },
    )
    .expect("checkpoint");
    let checkpoint = read_manifest(&config).unwrap();
    let mut restored = fixture();
    checkpoint
        .climate
        .unwrap()
        .restore(&mut restored)
        .expect("restore base");
    assert_eq!(ClimateImage::capture(&restored), base_image);

    state.day_time = 111_777;
    state.weather = (Weather::Clear, 77);
    state.rain_level = 0.2456789;
    state.thunder_level = 0.91234565;
    let tail = ClimateImage::capture(&state);
    writer
        .append(2, 2, &[JournalEntry::Climate { value: tail }])
        .expect("tail WAL");
    writer.flush().expect("flush");
    let batches = read_complete_batches_after(&dir.join("journal"), checkpoint.tick).unwrap();
    let replay = replay_batches(
        Replay {
            climate: checkpoint.climate,
            ..Default::default()
        },
        &batches,
    );
    assert_eq!(replay.tick, 2);
    replay
        .climate
        .unwrap()
        .restore(&mut restored)
        .expect("restore tail");
    assert_eq!(ClimateImage::capture(&restored), tail);
    assert_eq!(restored.journaled_climate, Some(tail));
    let (output, _, _) = run_tick(&mut restored, &[]);
    assert_eq!(output.snapshot.day_time, tail.day_time);
    assert_eq!(restored.weather, (Weather::Clear, 77));
    assert_eq!(
        output.snapshot.rain_level.to_bits(),
        (state.rain_level - 0.01).to_bits()
    );
    assert_eq!(
        output.snapshot.thunder_level.to_bits(),
        (state.thunder_level - 0.01).to_bits()
    );
    drop(writer);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn real_core_reopen_uses_checkpoint_and_tail_climate_not_the_tick_number() {
    for with_tail in [false, true] {
        let dir =
            std::env::temp_dir().join(format!("mc-core-weather-open-{}", uuid::Uuid::new_v4()));
        let mut config = crate::config::CoreConfig::new(dir.clone(), 42);
        config.simulation_distance = 1;
        config.checkpoint_every_ticks = u64::MAX;
        crate::core::Core::open(config.clone())
            .expect("initial core")
            .shutdown();
        let mut manifest = read_manifest(&config).unwrap();
        let image = ClimateImage {
            day_time: 123_987,
            weather_kind: 2,
            remaining_ticks: 1_234,
            thunder_remaining_ticks: 4321,
            clear_remaining_ticks: 0,
            thundering: true,
            rain_level_bits: 0.37123457_f32.to_bits(),
            thunder_level_bits: 0.12876543_f32.to_bits(),
            advance_time: false,
            advance_weather: false,
        };
        let recovered_tick = if with_tail {
            let tick = manifest.tick + 1;
            let mut writer = JournalWriter::open(&dir, 1).expect("writer");
            writer
                .append(tick, tick, &[JournalEntry::Climate { value: image }])
                .expect("tail");
            writer.flush().expect("tail flush");
            tick
        } else {
            manifest.climate = Some(image);
            write_manifest(&config, &manifest).expect("climate checkpoint");
            manifest.tick
        };
        let core = crate::core::Core::open(config.clone()).expect("reopened core");
        let mut rx = core.subscribe();
        let output = rx.blocking_recv().expect("recovered tick");
        assert_eq!(output.snapshot.day_time, image.day_time);
        assert_eq!(output.snapshot.day_time_rate, 0.0);
        assert!(output.snapshot.raining);
        let mut rain = f32::from_bits(image.rain_level_bits);
        let mut thunder = f32::from_bits(image.thunder_level_bits);
        for _ in recovered_tick..output.tick {
            rain = step_level(rain, true);
            thunder = step_level(thunder, true);
        }
        assert_eq!(output.snapshot.rain_level.to_bits(), rain.to_bits());
        assert_eq!(output.snapshot.thunder_level.to_bits(), thunder.to_bits());
        core.shutdown();
        let final_manifest = read_manifest(&config).unwrap();
        let final_image = final_manifest.climate.expect("durable climate");
        assert_eq!(final_image.day_time, image.day_time);
        assert_eq!(final_image.remaining_ticks, image.remaining_ticks);
        assert!(!final_image.advance_time && !final_image.advance_weather);
        let mut rain = f32::from_bits(image.rain_level_bits);
        let mut thunder = f32::from_bits(image.thunder_level_bits);
        for _ in recovered_tick..final_manifest.tick {
            rain = step_level(rain, true);
            thunder = step_level(thunder, true);
        }
        assert_eq!(final_image.rain_level_bits, rain.to_bits());
        assert_eq!(final_image.thunder_level_bits, thunder.to_bits());
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}

#[test]
fn invalid_persisted_weather_fails_without_partially_restoring_the_clock() {
    let mut state = fixture();
    state.day_time = 77;
    let valid = ClimateImage::capture(&state);
    for invalid in [
        ClimateImage {
            weather_kind: 3,
            ..valid
        },
        ClimateImage {
            remaining_ticks: -1,
            ..valid
        },
        ClimateImage {
            rain_level_bits: f32::NAN.to_bits(),
            ..valid
        },
        ClimateImage {
            thunder_level_bits: 1.1_f32.to_bits(),
            ..valid
        },
    ] {
        assert!(invalid.restore(&mut state).is_err());
        assert_eq!(ClimateImage::capture(&state), valid);
    }
}

#[test]
fn frozen_saturated_climate_does_not_rejournal_unchanged_images() {
    let mut state = fixture();
    state.weather = (Weather::Rain, 200);
    state.rain_level = 1.0;
    state
        .gamerules
        .insert("advance_time".into(), "false".into());
    state
        .gamerules
        .insert("advance_weather".into(), "false".into());
    let (output, _, _) = run_tick(&mut state, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry, JournalEntry::Climate { .. }))
            .count(),
        1
    );
    let (output, _, _) = run_tick(&mut state, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(
        !entries
            .iter()
            .any(|entry| matches!(entry, JournalEntry::Climate { .. }))
    );
}

#[test]
fn weather_gap_natural_rain_and_thunder_clocks_are_independent() {
    let mut state = fixture();
    state.weather = (Weather::Rain, 100);
    state.thunder_remaining_ticks = 1;
    advance_cycle(&mut state);
    assert_eq!(state.weather, (Weather::Thunder, 99));
    assert!((3_600..=15_600).contains(&state.thunder_remaining_ticks));
    state.thunder_remaining_ticks = 1;
    advance_cycle(&mut state);
    assert_eq!(state.weather, (Weather::Rain, 98));
    assert!((12_000..=180_000).contains(&state.thunder_remaining_ticks));
    state.weather = (Weather::Clear, 100);
    state.thunder_remaining_ticks = 1;
    advance_cycle(&mut state);
    assert!(state.thundering);
    assert_eq!(state.weather, (Weather::Clear, 99));
    state.weather.1 = 1;
    advance_cycle(&mut state);
    assert_eq!(state.weather.0, Weather::Thunder);
    assert!((12_000..=24_000).contains(&state.weather.1));
}

#[test]
fn weather_gap_rain_extinguishes_burning_entities_but_not_under_roof() {
    let mut state = precipitation_fixture("minecraft:plains");
    let mob = state.spawn_mob(state.registry.cow_type_id, [8.5, 200.0, 8.5], 10.0);
    let slot = state.entities.slot_of(mob).unwrap();
    crate::contact_hazards::ignite(&mut state, mob, 100);
    let mut intents = Vec::new();
    crate::contact_hazards::run(&mut state, &mut intents);
    assert_eq!(state.entities.body(slot).burn_ticks, 0);
    state
        .world
        .set_block(8, 203, 8, state.registry.states.stone);
    crate::contact_hazards::ignite(&mut state, mob, 100);
    crate::contact_hazards::run(&mut state, &mut intents);
    assert!(state.entities.body(slot).burn_ticks > 0);
}

struct BiomeGenerator(&'static str, u32);

impl mc_world::ChunkGenerator for BiomeGenerator {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty(
            pos,
            mc_world::BlockStateId(self.1),
            mc_data::Identifier::parse(self.0).unwrap(),
        )
    }
}

fn precipitation_fixture(biome: &'static str) -> TickState {
    let registry = Arc::new(CoreRegistry::load(None).unwrap());
    let generator = Arc::new(BiomeGenerator(biome, registry.states.air));
    let world = ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [8, 65, 8],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 1, 512);
    let chunk = crate::boundary::ChunkPos { x: 0, z: 0 };
    state
        .world
        .ensure_blocking(&[(chunk, 0)].into_iter().collect());
    state
        .world
        .set_active_chunks([chunk].into_iter().collect(), 0);
    state.weather = (Weather::Rain, 1000);
    for x in 0..16 {
        for z in 0..16 {
            state.world.set_block(x, 64, z, state.registry.states.stone);
        }
    }
    state
}

#[test]
fn precipitation_samples_accumulate_snow_freeze_water_edges_and_fill_cauldron() {
    use crate::boundary::BlockPos;
    for (biome, filled) in [
        ("minecraft:plains", "minecraft:water_cauldron"),
        ("minecraft:snowy_plains", "minecraft:powder_snow_cauldron"),
    ] {
        let mut state = precipitation_fixture(biome);
        let mut writes = Vec::new();
        let pos = BlockPos { x: 8, y: 65, z: 8 };
        let cauldron = crate::portals::state_id(&state, "minecraft:cauldron", &[]);
        state.world.set_block(pos.x, pos.y, pos.z, cauldron);
        for level in 1..=3 {
            surface::precipitate(&mut state, &mut writes, pos, 0);
            assert_eq!(
                state.world.block_at(pos.x, pos.y, pos.z),
                Some(crate::portals::state_id(
                    &state,
                    filled,
                    &[("level", &level.to_string())]
                ))
            );
        }
        let full = state.world.block_at(pos.x, pos.y, pos.z);
        surface::precipitate(&mut state, &mut writes, pos, 0);
        assert_eq!(state.world.block_at(pos.x, pos.y, pos.z), full);
    }
    let mut state = precipitation_fixture("minecraft:snowy_plains");
    let mut writes = Vec::new();
    let floor = BlockPos { x: 4, y: 64, z: 4 };
    surface::precipitate(&mut state, &mut writes, floor, 0);
    assert_eq!(
        state.world.block_at(4, 65, 4),
        Some(crate::portals::state_id(
            &state,
            "minecraft:snow",
            &[("layers", "1")]
        ))
    );
    state
        .gamerules
        .insert("max_snow_accumulation_height".into(), "2".into());
    surface::precipitate(&mut state, &mut writes, BlockPos { y: 65, ..floor }, 0);
    assert_eq!(
        state.world.block_at(4, 65, 4),
        Some(crate::portals::state_id(
            &state,
            "minecraft:snow",
            &[("layers", "2")]
        ))
    );
    let water = BlockPos { x: 6, y: 65, z: 6 };
    state.world.set_block(6, 65, 6, state.registry.states.water);
    surface::precipitate(&mut state, &mut writes, water, 0);
    assert_eq!(
        state.world.block_at(6, 65, 6),
        Some(crate::portals::state_id(&state, "minecraft:ice", &[]))
    );
}

#[test]
fn precipitation_respects_dry_biomes_roofs_and_clear_weather() {
    let mut state = precipitation_fixture("minecraft:desert");
    let pos = crate::boundary::BlockPos { x: 8, y: 65, z: 8 };
    assert!(!crate::conduit::rain_exposed(&state, [8.5, 65.0, 8.5]));
    let cauldron = crate::portals::state_id(&state, "minecraft:cauldron", &[]);
    state.world.set_block(8, 65, 8, cauldron);
    surface::precipitate(&mut state, &mut Vec::new(), pos, 0);
    assert_eq!(state.world.block_at(8, 65, 8), Some(cauldron));
    let mut state = precipitation_fixture("minecraft:plains");
    assert!(crate::conduit::rain_exposed(&state, [8.5, 65.0, 8.5]));
    state.world.set_block(8, 67, 8, state.registry.states.stone);
    assert!(!crate::conduit::rain_exposed(&state, [8.5, 65.0, 8.5]));
    assert_eq!(surface::surface(&state, 8, 8).unwrap().y, 67);
    state.weather = (Weather::Clear, 1000);
    state.tick = 16;
    let mut writes = Vec::new();
    settle(&mut state, &mut writes);
    assert!(writes.is_empty());
}

#[test]
fn natural_storm_scheduler_launches_retained_projectile_only_in_weather_dimension() {
    let mut state = precipitation_fixture("minecraft:plains");
    state.weather = (Weather::Thunder, 1000);
    let tick = (1_u64..100_000)
        .map(|tick| tick * 16)
        .find(|tick| {
            let mut rng = crate::natural_spawn::Rng(42 ^ tick.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            rng.below(16);
            rng.below(16);
            rng.below(100_000) < 16
        })
        .unwrap();
    state.tick = tick;
    settle(&mut state, &mut Vec::new());
    assert_eq!(state.projectiles.spawned.len(), 1);
    let slot = state
        .entities
        .slot_of(state.projectiles.spawned[0])
        .unwrap();
    assert_eq!(
        state.entities.row(slot).projectile.as_ref().unwrap().kind,
        crate::projectiles::ProjectileKind::Lightning
    );
    state.dimension = crate::dimensions::Dimension::Nether;
    settle(&mut state, &mut Vec::new());
    assert_eq!(state.projectiles.spawned.len(), 1);
}

#[test]
fn lightning_ignites_supported_air_and_channels_through_exposed_rods() {
    let mut state = precipitation_fixture("minecraft:plains");
    state
        .gamerules
        .insert("fire_spread_radius_around_player".into(), "-1".into());
    state.difficulty = crate::survival::Difficulty::Normal;
    lightning::strike_blocks(&mut state, [8.5, 65.0, 8.5]);
    let fire = state.world.block_at(8, 65, 8).unwrap();
    assert_eq!(
        state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(fire))
            .unwrap()
            .block
            .id
            .path(),
        "fire"
    );
    assert!(!state.merge_block_writes.is_empty());
    let pos = crate::boundary::BlockPos { x: 4, y: 65, z: 4 };
    let rod = crate::portals::state_id(&state, "minecraft:lightning_rod", &[("powered", "false")]);
    state.world.set_block(4, 65, 4, rod);
    state.weather = (Weather::Thunder, 1000);
    let mut trident = crate::projectiles::ProjectileState::new(
        crate::projectiles::ProjectileKind::Trident,
        None,
        mc_data::ItemStack::EMPTY,
    );
    trident.stack.enchantments.push(mc_data::ItemEnchantment {
        id: mc_data::Identifier::parse("minecraft:channeling").unwrap(),
        level: 1,
    });
    lightning::channel_rod(&mut state, &trident, pos);
    assert_eq!(state.projectiles.spawned.len(), 1);
    lightning::strike_blocks(&mut state, [4.5, 66.0, 4.5]);
    assert_eq!(
        state.world.block_at(4, 65, 4),
        Some(crate::portals::state_id(
            &state,
            "minecraft:lightning_rod",
            &[("powered", "true")]
        ))
    );
    state.world.set_block(4, 67, 4, state.registry.states.stone);
    lightning::channel_rod(&mut state, &trident, pos);
    assert_eq!(state.projectiles.spawned.len(), 1);
    assert_eq!(
        crate::redstone::output(&state, pos, crate::boundary::BlockPos { x: 5, ..pos }),
        15
    );
    let mut writes = std::mem::take(&mut state.merge_block_writes);
    for tick in 0..8 {
        state.tick = tick;
        crate::redstone::run(&mut state, &mut writes, &mut Vec::new());
        assert_eq!(
            crate::redstone::output(&state, pos, crate::boundary::BlockPos { x: 5, ..pos }),
            15
        );
        writes.clear();
    }
    state.tick = 8;
    crate::redstone::run(&mut state, &mut writes, &mut Vec::new());
    assert_eq!(
        crate::redstone::output(&state, pos, crate::boundary::BlockPos { x: 5, ..pos }),
        0
    );
}

#[test]
fn weather_gap_rods_attract_within_128_and_overlay_removal_roofs_are_respected() {
    let mut state = precipitation_fixture("minecraft:plains");
    let rod = crate::portals::state_id(&state, "minecraft:lightning_rod", &[("powered", "false")]);
    let origin = crate::boundary::BlockPos { x: 8, y: 66, z: 8 };
    let pos = crate::boundary::BlockPos { x: 4, y: 65, z: 4 };
    state.world.set_block(pos.x, pos.y, pos.z, rod);
    assert_eq!(
        lightning::attract(&state, origin),
        crate::boundary::BlockPos { y: 66, ..pos }
    );
    state.world.fold_overlay();
    assert_eq!(
        lightning::attract(&state, origin),
        crate::boundary::BlockPos { y: 66, ..pos }
    );
    assert_eq!(
        lightning::attract(
            &state,
            crate::boundary::BlockPos {
                x: 132,
                y: 65,
                z: 4
            }
        ),
        crate::boundary::BlockPos { y: 66, ..pos }
    );
    let outside = crate::boundary::BlockPos {
        x: 133,
        y: 65,
        z: 4,
    };
    assert_eq!(lightning::attract(&state, outside), outside);
    state
        .world
        .set_block(pos.x, pos.y + 1, pos.z, state.registry.states.stone);
    assert_eq!(lightning::attract(&state, origin), origin);
    state
        .world
        .set_block(pos.x, pos.y + 1, pos.z, state.registry.states.air);
    state
        .world
        .set_block(pos.x, pos.y, pos.z, state.registry.states.air);
    assert_eq!(lightning::attract(&state, origin), origin);
}

#[test]
#[ignore = "manual approximate weather microbenchmark; ordinary memory-gated lane"]
fn weather_gap_microbench_precipitation_and_rod_search() {
    let mut state = precipitation_fixture("minecraft:plains");
    let mut writes = Vec::new();
    let origin = crate::boundary::BlockPos { x: 8, y: 66, z: 8 };
    let rod = crate::portals::state_id(&state, "minecraft:lightning_rod", &[("powered", "false")]);
    state.world.set_block(4, 65, 4, rod);
    state.world.fold_overlay();
    let mut precipitation = Vec::new();
    let mut attraction = Vec::new();
    for _ in 0..7 {
        let start = std::time::Instant::now();
        for tick in 1..=10_000 {
            state.tick = tick * 16;
            settle(&mut state, &mut writes);
            writes.clear();
            std::hint::black_box(&state.weather_chunks);
        }
        precipitation.push(start.elapsed().as_nanos() / 10_000);
        let start = std::time::Instant::now();
        for _ in 0..1_000 {
            std::hint::black_box(lightning::attract(&state, origin));
        }
        attraction.push(start.elapsed().as_nanos() / 1_000);
    }
    precipitation.sort_unstable();
    attraction.sort_unstable();
    eprintln!(
        "approximate, contended machine; one active resident chunk; 7 samples; precipitation median={}ns/sample (amortized /16 ticks); rod attraction median={}ns/strike; precipitation samples={precipitation:?}; attraction samples={attraction:?}",
        precipitation[3], attraction[3]
    );
}

#[test]
fn clear_weather_still_freezes_exposed_cold_water() {
    let mut state = precipitation_fixture("minecraft:snowy_plains");
    state.weather = (Weather::Clear, 1000);
    for x in 1..15 {
        for z in 1..15 {
            state.world.set_block(x, 65, z, state.registry.states.water);
        }
    }
    state.world.fold_overlay();
    let mut writes = Vec::new();
    for tick in 1..=1024 {
        state.tick = tick * 16;
        settle(&mut state, &mut writes);
    }
    let ice = crate::portals::state_id(&state, "minecraft:ice", &[]);
    assert!(writes.iter().any(|(_, id)| *id == ice));
    assert!(!writes.iter().any(|(_, id)| *id != ice));
}
