use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use mc_data::Identifier;
use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_entity::Vec3;
use mc_physics::BlockMaterialIds;
use mc_world::light::ChunkLight;
use mc_world::{BlockRegistry, BlockStateId, Chunk, ChunkPos, WorldReadView, WorldStorage};
use tokio::sync::mpsc;

use super::*;
use super::{
    entity_lifecycle::track_entity_chunk_locked, herd_spawn_authority::NaturalSpawnReport,
};
use crate::login::LoggedInProfile;

const TEST_CHUNK: (i32, i32) = (2, 0);
const TEST_Y: i32 = 65;

#[derive(Clone, Copy)]
enum SpawnTerrain {
    Ground,
    Water,
    Unsupported,
}

fn simple_block(id: u32, name: &str) -> BlockReport {
    BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: BTreeMap::new(),
        states: vec![BlockStateReport {
            id,
            default: true,
            properties: BTreeMap::new(),
        }],
    }
}

fn spawn_world(terrain: SpawnTerrain, block_light: u8) -> (WorldReadView, BlockMaterialIds) {
    spawn_world_with_light(terrain, 15, block_light)
}

fn spawn_world_with_light(
    terrain: SpawnTerrain,
    sky_light: u8,
    block_light: u8,
) -> (WorldReadView, BlockMaterialIds) {
    spawn_world_chunks_with_light([TEST_CHUNK], terrain, sky_light, block_light)
}

fn spawn_world_chunks(
    chunks: impl IntoIterator<Item = (i32, i32)>,
    terrain: SpawnTerrain,
    block_light: u8,
) -> (WorldReadView, BlockMaterialIds) {
    spawn_world_chunks_with_light(chunks, terrain, 15, block_light)
}

fn spawn_world_chunks_with_light(
    chunks: impl IntoIterator<Item = (i32, i32)>,
    terrain: SpawnTerrain,
    sky_light: u8,
    block_light: u8,
) -> (WorldReadView, BlockMaterialIds) {
    let blocks = Arc::new(
        BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:stone"),
            simple_block(2, "minecraft:water"),
        ])
        .unwrap(),
    );
    let mut world = WorldStorage::in_memory(blocks);
    let plains = Identifier::parse("minecraft:plains").unwrap();
    for (x, z) in chunks {
        let position = ChunkPos { x, z };
        let mut chunk = Chunk::empty(position, BlockStateId(0), plains.clone());
        match terrain {
            SpawnTerrain::Ground => {
                for x in 0..16 {
                    for z in 0..16 {
                        chunk.set_block(x, TEST_Y - 1, z, BlockStateId(1));
                    }
                }
            }
            SpawnTerrain::Water => {
                for x in 0..16 {
                    for z in 0..16 {
                        for y in TEST_Y - 2..=TEST_Y + 2 {
                            chunk.set_block(x, y, z, BlockStateId(2));
                        }
                    }
                }
            }
            SpawnTerrain::Unsupported => {}
        }
        let mut light = ChunkLight::filled(sky_light, block_light);
        if sky_light == 0 && block_light == 0 {
            let local_y = usize::try_from(TEST_Y - mc_world::MIN_Y).unwrap();
            light.set_sky_local(0, local_y, 0, 1);
        }
        chunk.set_baked_light(&light);
        world.commit_chunk_snapshot(position, chunk).unwrap();
    }
    (world.read_view(), BlockMaterialIds::new(0, Some(2), None))
}

fn register_player(
    registry: &SessionRegistry,
    name: &str,
    chunks: HashSet<(i32, i32)>,
    simulation_distance: i32,
) -> (SessionId, mpsc::Receiver<OutboundCommand>) {
    register_player_at(
        registry,
        name,
        chunks,
        simulation_distance,
        (0, 0),
        PlayerPose::new(0.5, f64::from(TEST_Y), 0.5),
    )
}

fn register_player_at(
    registry: &SessionRegistry,
    name: &str,
    chunks: HashSet<(i32, i32)>,
    simulation_distance: i32,
    center: (i32, i32),
    pose: PlayerPose,
) -> (SessionId, mpsc::Receiver<OutboundCommand>) {
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid(name),
        name: name.to_owned(),
    };
    let (outbound, receiver) = mpsc::channel(64);
    let (session, _) = registry.register(
        &profile,
        center,
        simulation_distance,
        chunks.clone(),
        outbound,
        pose,
    );
    for chunk in chunks {
        assert!(registry.mark_loaded(session, chunk).is_empty());
    }
    (session, receiver)
}

fn template(
    slot: u8,
    type_id: i32,
    type_name: &str,
    local_x: i32,
    local_z: i32,
    hostile: bool,
) -> HerdSpawn {
    template_in_chunk(
        TEST_CHUNK, TEST_Y, slot, type_id, type_name, local_x, local_z, hostile,
    )
}

#[allow(clippy::too_many_arguments)]
fn template_in_chunk(
    chunk: (i32, i32),
    y: i32,
    slot: u8,
    type_id: i32,
    type_name: &str,
    local_x: i32,
    local_z: i32,
    hostile: bool,
) -> HerdSpawn {
    HerdSpawn {
        chunk,
        slot,
        entity_type_id: type_id,
        entity_type_name: type_name.to_owned(),
        position: Vec3::new(
            f64::from(chunk.0 * 16 + local_x) + 0.5,
            f64::from(y),
            f64::from(chunk.1 * 16 + local_z) + 0.5,
        ),
        hostile,
        sheep_color: None,
    }
}

fn tick_input<'a>(
    tick: u64,
    friendly_interval: u64,
    hostile_interval: u64,
    simulation_distance: i32,
    world_read: Option<&'a WorldReadView>,
    materials: Option<&'a BlockMaterialIds>,
) -> NaturalSpawnTickInput<'a> {
    NaturalSpawnTickInput {
        tick,
        policy: RandomTickPolicy {
            simulation_distance,
            friendly_spawn_interval_ticks: friendly_interval,
            hostile_spawn_interval_ticks: hostile_interval,
            ..RandomTickPolicy::default()
        },
        world_read,
        materials,
    }
}

#[test]
fn periodic_scheduler_is_bounded_rotating_and_intervals_are_independent() {
    let registry = SessionRegistry::new();
    let chunks = (2..=6).map(|x| (x, 0)).collect::<HashSet<_>>();
    let (_player, _receiver) = register_player(&registry, "SpawnRotation", chunks.clone(), 6);
    for chunk in 2..=5 {
        assert!(registry.register_natural_spawn_templates((chunk, 0), Vec::new()));
    }
    let far_template = HerdSpawn {
        chunk: (6, 0),
        slot: 0,
        entity_type_id: 11,
        entity_type_name: "minecraft:cow".to_owned(),
        position: Vec3::new(104.5, f64::from(TEST_Y), 0.5),
        hostile: false,
        sheep_color: None,
    };
    assert!(registry.register_natural_spawn_templates((6, 0), vec![far_template]));
    let mut scheduler = NaturalSpawnScheduler::default();

    let mut first_input = tick_input(1, 1, 0, 6, None, None);
    first_input.policy.friendly_spawn_chunk_budget = 4;
    let (first, _) = registry.tick_periodic_natural_spawning(&mut scheduler, first_input);
    assert_eq!(first.friendly.attempts, 1);
    assert_eq!(first.hostile.attempts, 0);
    assert_eq!(first.friendly.chunks_sampled, 4);
    assert_eq!(first.friendly.templates_considered, 0);

    let mut second_input = tick_input(2, 1, 0, 6, None, None);
    second_input.policy.friendly_spawn_chunk_budget = 4;
    let (second, _) = registry.tick_periodic_natural_spawning(&mut scheduler, second_input);
    assert_eq!(second.friendly.chunks_sampled, 4);
    assert_eq!(second.friendly.templates_considered, 1);
    assert_eq!(second.friendly.rejected_unloaded, 1);

    let (not_due, _) =
        registry.tick_periodic_natural_spawning(&mut scheduler, tick_input(3, 2, 0, 6, None, None));
    assert_eq!(not_due, NaturalSpawnReport::default());
    let (disabled, _) =
        registry.tick_periodic_natural_spawning(&mut scheduler, tick_input(4, 0, 0, 6, None, None));
    assert_eq!(disabled, NaturalSpawnReport::default());
}

#[test]
fn default_friendly_policy_reaches_and_refills_a_bounded_visible_population() {
    const ALPHA2_OBSERVED_FRIENDLY_POPULATION: usize = 10;

    let chunks = (2..18).map(|x| (x, 0)).collect::<HashSet<_>>();
    let (world_read, materials) =
        spawn_world_chunks(chunks.iter().copied(), SpawnTerrain::Ground, 0);
    let registry = SessionRegistry::new();
    let (_player, _receiver) = register_player(&registry, "DefaultPopulation", chunks.clone(), 20);
    for &chunk in &chunks {
        let templates = [4, 8, 12]
            .into_iter()
            .enumerate()
            .map(|(slot, local_x)| {
                template_in_chunk(
                    chunk,
                    TEST_Y,
                    slot as u8,
                    11,
                    "minecraft:cow",
                    local_x,
                    8,
                    false,
                )
            })
            .collect();
        assert!(registry.register_natural_spawn_templates(chunk, templates));
    }

    let mut scheduler = NaturalSpawnScheduler::default();
    let (initial, _) = registry.tick_periodic_natural_spawning(
        &mut scheduler,
        tick_input(400, 400, 0, 20, Some(&world_read), Some(&materials)),
    );
    assert_eq!(initial.friendly.chunks_sampled, 16);
    assert_eq!(initial.friendly.committed, 32);
    assert_eq!(initial.friendly.rejected_cap, 16);
    assert!(
        registry.persisted_entity_records().len() > ALPHA2_OBSERVED_FRIENDLY_POPULATION,
        "the default policy should materially exceed alpha2's observed sparse population"
    );

    let (at_cap, _) = registry.tick_periodic_natural_spawning(
        &mut scheduler,
        tick_input(800, 400, 0, 20, Some(&world_read), Some(&materials)),
    );
    assert_eq!(at_cap.friendly.committed, 0);
    assert_eq!(registry.persisted_entity_records().len(), 32);

    let removed_id = registry.persisted_entity_records()[0].snapshot.id;
    {
        let mut guards = registry.lock_session_entities("remove one natural spawn for refill");
        assert!(remove_server_entity_locked(&mut guards, removed_id).is_some());
    }
    let (refill, _) = registry.tick_periodic_natural_spawning(
        &mut scheduler,
        tick_input(1_200, 400, 0, 20, Some(&world_read), Some(&materials)),
    );
    assert_eq!(refill.friendly.committed, 1);
    assert_eq!(registry.persisted_entity_records().len(), 32);
}

#[test]
fn natural_spawn_cap_is_global_across_separated_players() {
    let chunks = HashSet::from([(2, 0), (18, 0)]);
    let (world_read, materials) =
        spawn_world_chunks(chunks.iter().copied(), SpawnTerrain::Ground, 0);
    let registry = SessionRegistry::new();
    let (_first, _first_receiver) = register_player_at(
        &registry,
        "GlobalCapA",
        HashSet::from([(2, 0)]),
        4,
        (0, 0),
        PlayerPose::new(0.5, f64::from(TEST_Y), 0.5),
    );
    let (_second, _second_receiver) = register_player_at(
        &registry,
        "GlobalCapB",
        HashSet::from([(18, 0)]),
        4,
        (22, 0),
        PlayerPose::new(352.5, f64::from(TEST_Y), 0.5),
    );
    for chunk in chunks {
        let templates = [2, 4, 6, 8, 10, 12]
            .into_iter()
            .enumerate()
            .map(|(slot, local_x)| {
                template_in_chunk(
                    chunk,
                    TEST_Y,
                    slot as u8,
                    11,
                    "minecraft:cow",
                    local_x,
                    8,
                    false,
                )
            })
            .collect();
        assert!(registry.register_natural_spawn_templates(chunk, templates));
    }

    let mut input = tick_input(1, 1, 0, 4, Some(&world_read), Some(&materials));
    input.policy.friendly_spawn_cap = 5;
    input.policy.friendly_spawn_chunk_budget = 48;
    let (report, _) =
        registry.tick_periodic_natural_spawning(&mut NaturalSpawnScheduler::default(), input);

    assert_eq!(report.friendly.chunks_sampled, 2);
    assert_eq!(report.friendly.committed, 5);
    assert!(report.friendly.rejected_cap > 0);
    assert_eq!(registry.persisted_entity_records().len(), 5);
}

#[test]
fn aquatic_spawn_cap_is_global_across_separated_players() {
    let chunks = HashSet::from([(2, 0), (18, 0)]);
    let (world_read, materials) =
        spawn_world_chunks(chunks.iter().copied(), SpawnTerrain::Water, 0);
    let registry = SessionRegistry::new();
    let (_first, _first_receiver) = register_player_at(
        &registry,
        "AquaticCapA",
        HashSet::from([(2, 0)]),
        4,
        (0, 0),
        PlayerPose::new(0.5, f64::from(TEST_Y), 0.5),
    );
    let (_second, _second_receiver) = register_player_at(
        &registry,
        "AquaticCapB",
        HashSet::from([(18, 0)]),
        4,
        (22, 0),
        PlayerPose::new(352.5, f64::from(TEST_Y), 0.5),
    );
    for chunk in chunks {
        assert!(registry.register_natural_spawn_templates(
            chunk,
            vec![template_in_chunk(
                chunk,
                TEST_Y,
                0,
                18,
                "minecraft:cod",
                8,
                8,
                false,
            )],
        ));
    }

    let mut input = tick_input(1, 1, 0, 4, Some(&world_read), Some(&materials));
    input.policy.aquatic_spawn_cap = 1;
    input.policy.friendly_spawn_chunk_budget = 48;
    let (report, _) =
        registry.tick_periodic_natural_spawning(&mut NaturalSpawnScheduler::default(), input);

    assert_eq!(report.friendly.committed, 1);
    assert_eq!(report.friendly.rejected_cap, 1);
    assert_eq!(registry.persisted_entity_records().len(), 1);
}

#[test]
fn hostile_spawn_cap_is_global_across_separated_players() {
    let chunks = HashSet::from([(2, 0), (18, 0)]);
    let (world_read, materials) =
        spawn_world_chunks_with_light(chunks.iter().copied(), SpawnTerrain::Ground, 0, 0);
    let registry = SessionRegistry::new();
    registry.set_world_time(18_000);
    let (_first, _first_receiver) = register_player_at(
        &registry,
        "HostileCapA",
        HashSet::from([(2, 0)]),
        4,
        (0, 0),
        PlayerPose::new(0.5, f64::from(TEST_Y), 0.5),
    );
    let (_second, _second_receiver) = register_player_at(
        &registry,
        "HostileCapB",
        HashSet::from([(18, 0)]),
        4,
        (22, 0),
        PlayerPose::new(352.5, f64::from(TEST_Y), 0.5),
    );
    for chunk in chunks {
        assert!(registry.register_natural_spawn_templates(
            chunk,
            vec![template_in_chunk(
                chunk,
                TEST_Y,
                0,
                54,
                "minecraft:zombie",
                8,
                8,
                true,
            )],
        ));
    }

    let mut input = tick_input(1, 0, 1, 4, Some(&world_read), Some(&materials));
    input.policy.hostile_spawn_cap = 1;
    input.policy.hostile_spawn_chunk_budget = 48;
    let (report, _) =
        registry.tick_periodic_natural_spawning(&mut NaturalSpawnScheduler::default(), input);

    assert_eq!(report.hostile.committed, 1);
    assert_eq!(report.hostile.rejected_cap, 1);
    assert_eq!(registry.persisted_entity_records().len(), 1);
}

#[test]
fn overlapping_players_do_not_duplicate_active_spawn_chunks() {
    let (world_read, materials) = spawn_world(SpawnTerrain::Ground, 0);
    let registry = SessionRegistry::new();
    let chunks = HashSet::from([TEST_CHUNK]);
    let (_first, _first_receiver) = register_player(&registry, "OverlapA", chunks.clone(), 4);
    let (_second, _second_receiver) = register_player(&registry, "OverlapB", chunks, 4);
    let templates = [2, 4, 6, 8, 10, 12]
        .into_iter()
        .enumerate()
        .map(|(slot, local_x)| template(slot as u8, 11, "minecraft:cow", local_x, 8, false))
        .collect();
    assert!(registry.register_natural_spawn_templates(TEST_CHUNK, templates));

    let mut input = tick_input(1, 1, 0, 4, Some(&world_read), Some(&materials));
    input.policy.friendly_spawn_cap = 32;
    input.policy.friendly_spawn_chunk_budget = 48;
    let (report, _) =
        registry.tick_periodic_natural_spawning(&mut NaturalSpawnScheduler::default(), input);

    assert_eq!(report.friendly.chunks_sampled, 1);
    assert_eq!(report.friendly.templates_considered, 6);
    assert_eq!(report.friendly.committed, 6);
    assert_eq!(registry.persisted_entity_records().len(), 6);
}

#[test]
fn simulation_distance_filters_active_spawn_chunks_before_planning() {
    let far_chunk = (5, 0);
    let (world_read, materials) = spawn_world_chunks([far_chunk], SpawnTerrain::Ground, 0);
    let registry = SessionRegistry::new();
    let (_player, _receiver) = register_player(
        &registry,
        "SimulationDistance",
        HashSet::from([far_chunk]),
        1,
    );
    assert!(registry.register_natural_spawn_templates(
        far_chunk,
        vec![template_in_chunk(
            far_chunk,
            TEST_Y,
            0,
            11,
            "minecraft:cow",
            8,
            8,
            false
        )],
    ));

    let mut input = tick_input(1, 1, 0, 1, Some(&world_read), Some(&materials));
    input.policy.friendly_spawn_chunk_budget = 64;
    let (report, _) =
        registry.tick_periodic_natural_spawning(&mut NaturalSpawnScheduler::default(), input);

    assert_eq!(report.friendly.attempts, 1);
    assert_eq!(report.friendly.chunks_sampled, 0);
    assert_eq!(report.friendly.templates_considered, 0);
    assert_eq!(report.friendly.committed, 0);
    assert!(registry.persisted_entity_records().is_empty());
}

#[test]
fn high_chunk_budget_keeps_entity_collision_fence_active() {
    let (world_read, materials) = spawn_world(SpawnTerrain::Ground, 0);
    let registry = SessionRegistry::new();
    let (_player, _receiver) =
        register_player(&registry, "CollisionFence", HashSet::from([TEST_CHUNK]), 4);
    assert!(registry.register_natural_spawn_templates(
        TEST_CHUNK,
        vec![
            template(0, 11, "minecraft:cow", 8, 8, false),
            template(1, 11, "minecraft:cow", 8, 8, false),
        ],
    ));

    let mut input = tick_input(1, 1, 0, 4, Some(&world_read), Some(&materials));
    input.policy.friendly_spawn_cap = 64;
    input.policy.friendly_spawn_chunk_budget = 64;
    let (report, _) =
        registry.tick_periodic_natural_spawning(&mut NaturalSpawnScheduler::default(), input);

    assert_eq!(report.friendly.templates_considered, 2);
    assert_eq!(report.friendly.committed, 1);
    assert_eq!(report.friendly.rejected_collision, 1);
    assert_eq!(registry.persisted_entity_records().len(), 1);
}

#[test]
fn zero_natural_spawn_cap_admits_no_entities() {
    let (world_read, materials) = spawn_world(SpawnTerrain::Ground, 0);
    let registry = SessionRegistry::new();
    let (_player, _receiver) =
        register_player(&registry, "ZeroCap", HashSet::from([TEST_CHUNK]), 4);
    assert!(registry.register_natural_spawn_templates(
        TEST_CHUNK,
        vec![template(0, 11, "minecraft:cow", 8, 8, false)],
    ));

    let mut input = tick_input(1, 1, 0, 4, Some(&world_read), Some(&materials));
    input.policy.friendly_spawn_cap = 0;
    let (report, _) =
        registry.tick_periodic_natural_spawning(&mut NaturalSpawnScheduler::default(), input);

    assert_eq!(report.friendly.committed, 0);
    assert_eq!(report.friendly.rejected_cap, 1);
    assert!(registry.persisted_entity_records().is_empty());
}

#[test]
fn periodic_ground_spawns_obey_per_chunk_cap_and_support() {
    let (world_read, materials) = spawn_world(SpawnTerrain::Ground, 0);
    let registry = SessionRegistry::new();
    let (_player, _receiver) =
        register_player(&registry, "GroundSpawn", HashSet::from([TEST_CHUNK]), 4);
    let templates = (0..7)
        .map(|slot| template(slot, 11, "minecraft:cow", 2 + i32::from(slot) * 2, 8, false))
        .collect();
    assert!(registry.register_natural_spawn_templates(TEST_CHUNK, templates));
    let mut scheduler = NaturalSpawnScheduler::default();

    let (report, _) = registry.tick_periodic_natural_spawning(
        &mut scheduler,
        tick_input(1, 1, 0, 4, Some(&world_read), Some(&materials)),
    );
    assert_eq!(report.friendly.committed, 6);
    assert_eq!(report.friendly.rejected_cap, 1);
    assert_eq!(registry.persisted_entity_records().len(), 6);

    let (unsupported_world, unsupported_materials) = spawn_world(SpawnTerrain::Unsupported, 0);
    let unsupported_registry = SessionRegistry::new();
    let (_player, _receiver) = register_player(
        &unsupported_registry,
        "UnsupportedSpawn",
        HashSet::from([TEST_CHUNK]),
        4,
    );
    assert!(unsupported_registry.register_natural_spawn_templates(
        TEST_CHUNK,
        vec![template(0, 11, "minecraft:cow", 8, 8, false)],
    ));
    let (unsupported, _) = unsupported_registry.tick_periodic_natural_spawning(
        &mut NaturalSpawnScheduler::default(),
        tick_input(
            1,
            1,
            0,
            4,
            Some(&unsupported_world),
            Some(&unsupported_materials),
        ),
    );
    assert_eq!(unsupported.friendly.rejected_block_or_fluid, 1);
    assert_eq!(unsupported.friendly.committed, 0);
}

#[test]
fn periodic_aquatic_and_hostile_admission_use_fluid_time_and_darkness() {
    let (water_world, materials) = spawn_world(SpawnTerrain::Water, 0);
    let aquatic = SessionRegistry::new();
    let (_player, _receiver) =
        register_player(&aquatic, "AquaticSpawn", HashSet::from([TEST_CHUNK]), 4);
    assert!(aquatic.register_natural_spawn_templates(
        TEST_CHUNK,
        vec![template(0, 18, "minecraft:cod", 8, 8, false)],
    ));
    let (aquatic_report, _) = aquatic.tick_periodic_natural_spawning(
        &mut NaturalSpawnScheduler::default(),
        tick_input(1, 1, 0, 4, Some(&water_world), Some(&materials)),
    );
    assert_eq!(aquatic_report.friendly.committed, 1);

    let land_in_water = SessionRegistry::new();
    let (_player, _receiver) = register_player(
        &land_in_water,
        "LandInWater",
        HashSet::from([TEST_CHUNK]),
        4,
    );
    assert!(land_in_water.register_natural_spawn_templates(
        TEST_CHUNK,
        vec![template(0, 11, "minecraft:cow", 8, 8, false)],
    ));
    let (land_report, _) = land_in_water.tick_periodic_natural_spawning(
        &mut NaturalSpawnScheduler::default(),
        tick_input(1, 1, 0, 4, Some(&water_world), Some(&materials)),
    );
    assert_eq!(land_report.friendly.rejected_block_or_fluid, 1);

    let (bright_world, bright_materials) = spawn_world(SpawnTerrain::Ground, 15);
    let bright = SessionRegistry::new();
    let (_player, _receiver) =
        register_player(&bright, "BrightHostile", HashSet::from([TEST_CHUNK]), 4);
    assert!(bright.register_natural_spawn_templates(
        TEST_CHUNK,
        vec![template(0, 54, "minecraft:zombie", 8, 8, true)],
    ));
    let mut bright_scheduler = NaturalSpawnScheduler::default();
    let (day, _) = bright.tick_periodic_natural_spawning(
        &mut bright_scheduler,
        tick_input(1, 0, 1, 4, Some(&bright_world), Some(&bright_materials)),
    );
    assert_eq!(day.hostile.committed, 0);
    assert_eq!(day.hostile.rejected_time + day.hostile.rejected_darkness, 1);

    let (day_cave_world, day_cave_materials) = spawn_world_with_light(SpawnTerrain::Ground, 0, 0);
    let day_cave = SessionRegistry::new();
    let (_player, _receiver) =
        register_player(&day_cave, "DayCaveHostile", HashSet::from([TEST_CHUNK]), 4);
    assert!(day_cave.register_natural_spawn_templates(
        TEST_CHUNK,
        vec![template(0, 54, "minecraft:zombie", 8, 8, true)],
    ));
    let (day_cave_report, _) = day_cave.tick_periodic_natural_spawning(
        &mut NaturalSpawnScheduler::default(),
        tick_input(1, 0, 1, 4, Some(&day_cave_world), Some(&day_cave_materials)),
    );
    assert_eq!(
        day_cave_report.hostile.committed, 1,
        "unexpected day-cave report: {day_cave_report:?}"
    );
    assert_eq!(day_cave_report.hostile.rejected_time, 0);
    assert_eq!(day_cave_report.hostile.rejected_darkness, 0);

    let (lit_cave_world, lit_cave_materials) = spawn_world_with_light(SpawnTerrain::Ground, 0, 15);
    let lit_cave = SessionRegistry::new();
    let (_player, _receiver) = register_player(
        &lit_cave,
        "LitDayCaveHostile",
        HashSet::from([TEST_CHUNK]),
        4,
    );
    assert!(lit_cave.register_natural_spawn_templates(
        TEST_CHUNK,
        vec![template(0, 54, "minecraft:zombie", 8, 8, true)],
    ));
    let (lit_cave_report, _) = lit_cave.tick_periodic_natural_spawning(
        &mut NaturalSpawnScheduler::default(),
        tick_input(1, 0, 1, 4, Some(&lit_cave_world), Some(&lit_cave_materials)),
    );
    assert_eq!(lit_cave_report.hostile.committed, 0);
    assert_eq!(lit_cave_report.hostile.rejected_time, 0);
    assert_eq!(lit_cave_report.hostile.rejected_darkness, 1);

    bright.set_world_time(NIGHT_START_TICK);
    let (lit_night, _) = bright.tick_periodic_natural_spawning(
        &mut bright_scheduler,
        tick_input(2, 0, 1, 4, Some(&bright_world), Some(&bright_materials)),
    );
    assert_eq!(lit_night.hostile.rejected_darkness, 1);

    let (dark_world, dark_materials) = spawn_world(SpawnTerrain::Ground, 0);

    let thunder = SessionRegistry::new();
    let (_player, _receiver) =
        register_player(&thunder, "ThunderHostile", HashSet::from([TEST_CHUNK]), 4);
    thunder.set_world_time(6_000);
    thunder.set_weather(WeatherKind::Thunder);
    thunder.tick_weather(100);
    assert!(thunder.register_natural_spawn_templates(
        TEST_CHUNK,
        vec![template(0, 54, "minecraft:zombie", 8, 8, true)],
    ));
    let (thunder_report, _) = thunder.tick_periodic_natural_spawning(
        &mut NaturalSpawnScheduler::default(),
        tick_input(8, 0, 1, 4, Some(&dark_world), Some(&dark_materials)),
    );
    assert_eq!(
        thunder_report.hostile.committed, 1,
        "unexpected daytime-thunder report: {thunder_report:?}"
    );

    let dark = SessionRegistry::new();
    let (_player, _receiver) =
        register_player(&dark, "DarkHostile", HashSet::from([TEST_CHUNK]), 4);
    assert!(dark.register_natural_spawn_templates(
        TEST_CHUNK,
        vec![template(0, 54, "minecraft:zombie", 8, 8, true)],
    ));
    dark.set_world_time(NIGHT_START_TICK);
    let (twilight_report, _) = dark.tick_periodic_natural_spawning(
        &mut NaturalSpawnScheduler::default(),
        tick_input(2, 0, 1, 4, Some(&dark_world), Some(&dark_materials)),
    );
    assert_eq!(twilight_report.hostile.committed, 0);
    assert_eq!(twilight_report.hostile.rejected_darkness, 1);

    dark.set_world_time(18_000);
    let (dark_report, _) = dark.tick_periodic_natural_spawning(
        &mut NaturalSpawnScheduler::default(),
        tick_input(8, 0, 1, 4, Some(&dark_world), Some(&dark_materials)),
    );
    assert_eq!(
        dark_report.hostile.committed, 1,
        "unexpected dark-night report: {dark_report:?}"
    );
}

#[test]
fn periodic_population_refills_after_movement_and_despawn() {
    let (world_read, materials) = spawn_world(SpawnTerrain::Ground, 0);
    let registry = SessionRegistry::new();
    let (_player, _receiver) =
        register_player(&registry, "SpawnRefill", HashSet::from([TEST_CHUNK]), 4);
    assert!(registry.register_natural_spawn_templates(
        TEST_CHUNK,
        vec![template(0, 11, "minecraft:cow", 8, 8, false)],
    ));
    let mut scheduler = NaturalSpawnScheduler::default();
    let (first, _) = registry.tick_periodic_natural_spawning(
        &mut scheduler,
        tick_input(1, 1, 0, 4, Some(&world_read), Some(&materials)),
    );
    assert_eq!(first.friendly.committed, 1);
    let first_id = registry.persisted_entity_records()[0].snapshot.id;

    {
        let mut guards = registry.lock_session_entities("move natural spawn out of active chunks");
        let expected = guards.entities.snapshot(first_id).unwrap();
        let mut moved = expected.clone();
        moved.position = Vec3::new(320.5, f64::from(TEST_Y), 0.5);
        assert!(
            guards
                .entities
                .replace_snapshot_if_current(expected, moved.clone())
        );
        track_entity_chunk_locked(&mut guards, first_id, moved.position);
    }
    let (after_move, _) = registry.tick_periodic_natural_spawning(
        &mut scheduler,
        tick_input(2, 1, 0, 4, Some(&world_read), Some(&materials)),
    );
    assert_eq!(after_move.friendly.committed, 1);
    let records = registry.persisted_entity_records();
    assert_eq!(records.len(), 2);
    let local_id = records
        .iter()
        .find(|record| record.snapshot.id != first_id)
        .unwrap()
        .snapshot
        .id;

    {
        let mut guards = registry.lock_session_entities("despawn local natural spawn");
        assert!(remove_server_entity_locked(&mut guards, local_id).is_some());
    }
    let (after_despawn, _) = registry.tick_periodic_natural_spawning(
        &mut scheduler,
        tick_input(3, 1, 0, 4, Some(&world_read), Some(&materials)),
    );
    assert_eq!(after_despawn.friendly.committed, 1);
    let records = registry.persisted_entity_records();
    assert_eq!(records.len(), 2);
    assert!(records.iter().any(|record| record.snapshot.id == first_id));
    assert!(records.iter().any(|record| record.snapshot.id != first_id));
}

#[test]
fn periodic_natural_spawn_restart_preserves_entities_and_rejects_replayed_identities() {
    const SPAWN_TICK: u64 = 40;

    let (world_read, materials) = spawn_world_with_light(SpawnTerrain::Ground, 0, 0);
    let source = SessionRegistry::new();
    source.set_world_time(NIGHT_START_TICK);
    let (_player, _receiver) =
        register_player(&source, "SpawnRestart", HashSet::from([TEST_CHUNK]), 4);
    let templates = vec![
        template(0, 11, "minecraft:cow", 4, 8, false),
        template(1, 54, "minecraft:zombie", 12, 8, true),
    ];
    assert!(source.register_natural_spawn_templates(TEST_CHUNK, templates.clone()));

    let (spawned, _) = source.tick_periodic_natural_spawning(
        &mut NaturalSpawnScheduler::default(),
        tick_input(
            SPAWN_TICK,
            SPAWN_TICK,
            SPAWN_TICK,
            4,
            Some(&world_read),
            Some(&materials),
        ),
    );
    assert_eq!(spawned.friendly.committed, 1);
    assert_eq!(spawned.hostile.committed, 1);
    source.synchronize_entity_lifecycle_epoch(SPAWN_TICK);

    let (checkpoint, _) = source.persisted_entity_save_snapshot();
    assert_eq!(checkpoint.lifecycle_clock, SPAWN_TICK);
    assert_eq!(checkpoint.records.len(), 2);
    let mut expected = checkpoint
        .records
        .iter()
        .map(|record| record.snapshot.clone())
        .collect::<Vec<_>>();
    expected.sort_unstable_by_key(|snapshot| snapshot.id);

    let restored = SessionRegistry::new();
    assert_eq!(restored.restore_persisted_entities(checkpoint), 2);
    assert_eq!(restored.simulation_tick(), SPAWN_TICK);
    let mut actual = restored
        .persisted_entity_records()
        .into_iter()
        .map(|record| record.snapshot)
        .collect::<Vec<_>>();
    actual.sort_unstable_by_key(|snapshot| snapshot.id);
    assert_eq!(
        actual, expected,
        "restart must preserve retained mob snapshots exactly"
    );

    // Move the restored mobs away from their deterministic template positions so a
    // replay reaches unique-owner identity validation instead of being rejected by
    // the collision planner first.
    {
        let mut guards = restored.lock_session_entities("move restored natural spawns for replay");
        for (index, snapshot) in actual.iter().enumerate() {
            let current = guards
                .entities
                .snapshot(snapshot.id)
                .expect("restored natural spawn snapshot");
            let mut moved = current.clone();
            moved.position = Vec3::new(320.5 + index as f64, f64::from(TEST_Y), 0.5);
            assert!(
                guards
                    .entities
                    .replace_snapshot_if_current(current, moved.clone())
            );
            track_entity_chunk_locked(&mut guards, snapshot.id, moved.position);
        }
    }

    restored.set_world_time(NIGHT_START_TICK);
    let (_player, _receiver) =
        register_player(&restored, "SpawnRestart", HashSet::from([TEST_CHUNK]), 4);
    assert!(restored.register_natural_spawn_templates(TEST_CHUNK, templates));
    let (replayed, _) = restored.tick_periodic_natural_spawning(
        &mut NaturalSpawnScheduler::default(),
        tick_input(
            SPAWN_TICK,
            SPAWN_TICK,
            SPAWN_TICK,
            4,
            Some(&world_read),
            Some(&materials),
        ),
    );
    assert_eq!(replayed.friendly.committed, 0);
    assert_eq!(replayed.hostile.committed, 0);
    assert_eq!(replayed.friendly.rejected_duplicate_or_stale, 1);
    assert_eq!(replayed.hostile.rejected_duplicate_or_stale, 1);

    let records = restored.persisted_entity_records();
    assert_eq!(
        records.len(),
        2,
        "restart replay must not duplicate deterministic identities"
    );
    let restored_uuids = records
        .iter()
        .map(|record| record.snapshot.uuid)
        .collect::<HashSet<_>>();
    let expected_uuids = expected
        .iter()
        .map(|snapshot| snapshot.uuid)
        .collect::<HashSet<_>>();
    assert_eq!(restored_uuids, expected_uuids);
}
