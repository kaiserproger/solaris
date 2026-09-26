use super::super::*;
use mc_data::Identifier;
use std::collections::BTreeMap;

fn terrain_registry_missing_grass_block() -> Arc<mc_world::BlockRegistry> {
    use mc_data::blocks::{BlockReport, BlockStateReport};
    let names = [
        "minecraft:air",
        "minecraft:bedrock",
        "minecraft:stone",
        "minecraft:dirt",
    ];
    let report = names
        .into_iter()
        .enumerate()
        .map(|(id, name)| BlockReport {
            id: Identifier::parse(name).unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: u32::try_from(id).unwrap(),
                default: true,
                properties: BTreeMap::new(),
            }],
        })
        .collect::<Vec<_>>();
    Arc::new(mc_world::BlockRegistry::from_report(&report).unwrap())
}

#[test]
fn ensure_world_region_root_creates_legacy_layout_for_missing_world() {
    let tmp = tempfile::tempdir().unwrap();
    let world = tmp.path().join("new-world");

    ensure_world_region_root(&world).unwrap();

    assert!(world.join("region").is_dir());
}

#[test]
fn ensure_world_region_root_keeps_existing_modern_layout() {
    let tmp = tempfile::tempdir().unwrap();
    let modern = tmp
        .path()
        .join("dimensions")
        .join("minecraft")
        .join("overworld")
        .join("region");
    std::fs::create_dir_all(&modern).unwrap();

    ensure_world_region_root(tmp.path()).unwrap();

    assert!(modern.is_dir());
    assert!(!tmp.path().join("region").exists());
}

#[test]
fn ensure_world_region_root_reports_blocked_legacy_region_file() {
    let tmp = tempfile::tempdir().unwrap();
    let legacy = tmp.path().join("region");
    std::fs::write(&legacy, b"not a directory").unwrap();

    let error = ensure_world_region_root(tmp.path()).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("creating empty world region directory"),
        "{error:#}"
    );
    assert!(legacy.is_file());
}

#[test]
fn chunk_cache_size_covers_view_plus_light_border() {
    assert_eq!(chunk_cache_size_for_view_distance(0), 9);
    assert_eq!(chunk_cache_size_for_view_distance(4), 169);
    assert_eq!(chunk_cache_size_for_view_distance(10), 625);
    assert_eq!(chunk_cache_size_for_view_distance(-1), 9);
}

#[test]
fn playable_startup_keeps_its_fixed_vd4_contract() {
    let config: mc_server::ServerConfig =
        toml::from_str(include_str!("../../../../playable.toml")).expect("parse playable config");

    assert_eq!(startup_spawn_view_distance(&config), 4);
    assert_eq!(runtime_cache_view_distance(&config), 4);
}

#[test]
fn disabled_autoscale_prepares_the_configured_view_distance() {
    let mut config: mc_server::ServerConfig =
        toml::from_str(include_str!("../../../../example.toml")).expect("parse example config");
    config.server.view_distance = 24;
    config.autoscale.enabled = false;

    assert_eq!(startup_spawn_view_distance(&config), 24);
    assert_eq!(runtime_cache_view_distance(&config), 24);
}

#[test]
fn autoscale_overrides_define_startup_and_cache_windows() {
    let mut config: mc_server::ServerConfig =
        toml::from_str(include_str!("../../../../example.toml")).expect("parse example config");
    config.server.view_distance = 20;
    config.autoscale.enabled = true;
    config.autoscale.profile = mc_server::AutoscaleProfile::HighEnd;
    config.autoscale.min_view_distance = Some(12);
    config.autoscale.max_view_distance = Some(28);

    assert_eq!(startup_spawn_view_distance(&config), 12);
    assert_eq!(runtime_cache_view_distance(&config), 28);
}

#[test]
fn runtime_config_rejects_resource_limit_overflows_before_startup() {
    let world = tempfile::tempdir().unwrap();
    let mut config: ServerConfig =
        toml::from_str(include_str!("../../../../example.toml")).expect("parse example config");
    config.data.world_dir = Some(world.path().to_path_buf());

    config.server.max_players = 4_097;
    let error = validate_runtime_config(&config).unwrap_err();
    assert!(error.to_string().contains("server.max_players=4097"));
    config.server.max_players = 20;

    config.chunk_pipeline.chunk_result_queue_size = 262_145;
    let error = validate_runtime_config(&config).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("chunk_pipeline.chunk_result_queue_size=262145")
    );
    config.chunk_pipeline.chunk_result_queue_size = 256;

    config.chunk_pipeline.chunk_generate_rate = 4_097;
    let error = validate_runtime_config(&config).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("chunk_pipeline.chunk_generate_rate=4097")
    );
    config.chunk_pipeline.chunk_generate_rate = 8;

    config.simulation.random_tick_speed = 4_097;
    let error = validate_runtime_config(&config).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("simulation.random_tick_speed=4097")
    );
    config.simulation.random_tick_speed = 3;

    config.simulation.friendly_spawn_chunk_budget = 0;
    let error = validate_runtime_config(&config).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("simulation.friendly_spawn_chunk_budget=0")
    );
    config.simulation.friendly_spawn_chunk_budget = 48;

    config.chunk_pipeline.compression_level = Some(10);
    let error = validate_runtime_config(&config).unwrap_err();
    assert!(error.to_string().contains("compression_level"));
}

#[test]
fn runtime_config_accepts_documented_resource_limit_boundaries() {
    let world = tempfile::tempdir().unwrap();
    let mut config: ServerConfig =
        toml::from_str(include_str!("../../../../example.toml")).expect("parse example config");
    config.data.world_dir = Some(world.path().to_path_buf());
    config.server.max_players = 4_096;
    config.chunk_pipeline.chunk_send_rate = 4_096;
    config.chunk_pipeline.chunk_load_rate = 4_096;
    config.chunk_pipeline.chunk_generate_rate = 4_096;
    config.chunk_pipeline.chunk_prepare_budget_ms = 1_000;
    config.chunk_pipeline.chunk_prepare_batch_size = 4_096;
    config.chunk_pipeline.chunk_result_queue_size = 262_144;
    config.chunk_pipeline.region_cache_size = 65_536;
    config.chunk_pipeline.compression_level = Some(9);
    config.simulation.random_tick_speed = 4_096;
    config.simulation.save_interval_ticks = 1_728_000;
    config.simulation.friendly_spawn_interval_ticks = 1_728_000;
    config.simulation.hostile_spawn_interval_ticks = 1_728_000;
    config.simulation.friendly_spawn_chunk_budget = mc_net::MAX_NATURAL_SPAWN_CHUNK_BUDGET;
    config.simulation.hostile_spawn_chunk_budget = mc_net::MAX_NATURAL_SPAWN_CHUNK_BUDGET;

    validate_runtime_config(&config).unwrap();

    config.chunk_pipeline.chunk_send_rate = u32::MAX;
    config.chunk_pipeline.chunk_load_rate = u32::MAX;
    config.chunk_pipeline.chunk_generate_rate = u32::MAX;
    validate_runtime_config(&config).unwrap();
}

#[test]
fn runtime_config_rejects_invalid_chunk_geometry() {
    let world = tempfile::tempdir().unwrap();
    let toml_src = format!(
        r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 25565

            [data]
            world_dir = "{}"
            min_y = 1
            height = 255
        "#,
        world.path().display()
    );
    let config: ServerConfig = toml::from_str(&toml_src).unwrap();

    let error = validate_runtime_config(&config).unwrap_err();

    assert!(error.to_string().contains("data.min_y (1)"), "{error:#}");
    assert!(error.to_string().contains("data.height (255)"), "{error:#}");
}

#[test]
fn world_contract_rejects_mismatched_geometry_before_world_open() {
    let world = tempfile::tempdir().unwrap();
    let original = mc_world::ChunkGeometry::new(0, 256).unwrap();
    let changed = mc_world::ChunkGeometry::new(-64, 384).unwrap();

    assert_eq!(
        ensure_world_contract(
            world.path(),
            original,
            7,
            "vanilla_like",
            "vanilla",
            "vanilla",
        )
        .unwrap(),
        WorldSource::SolarisGenerated,
    );
    let bytes = std::fs::read(world_contract_path(world.path())).unwrap();
    let persisted: PersistedWorldContract = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(persisted.schema, WORLD_CONTRACT_SCHEMA);
    assert_eq!(persisted.worldgen_revision, mc_worldgen::WORLDGEN_REVISION);
    assert_eq!(persisted.seed, 7);
    assert_eq!(persisted.mode, "vanilla_like");
    assert_eq!(persisted.ore_profile, "vanilla");
    assert_eq!(persisted.settlement_profile, "vanilla");
    assert_eq!(persisted.min_y, 0);
    assert_eq!(persisted.height, 256);
    assert_eq!(persisted.spawn_block_x, 0);
    assert_eq!(persisted.spawn_block_z, 0);
    assert!(world_requires_solaris_spawn(world.path()).unwrap());

    let error = ensure_world_contract(
        world.path(),
        changed,
        7,
        "vanilla_like",
        "vanilla",
        "vanilla",
    )
    .unwrap_err();

    let message = error.to_string();
    assert!(message.contains("world contract geometry"), "{message}");
    assert!(message.contains("0..256"), "{message}");
    assert!(message.contains("-64..320"), "{message}");
}

#[test]
fn world_contract_schema_two_is_rejected_cleanly_before_spawn_validation() {
    let world = tempfile::tempdir().unwrap();
    let contract_dir = world.path().join("solaris");
    std::fs::create_dir_all(&contract_dir).unwrap();
    let legacy = serde_json::json!({
        "schema": 2,
        "worldgen_revision": mc_worldgen::WORLDGEN_REVISION,
        "seed": 712816,
        "mode": "tellus_like",
        "ore_profile": "vanilla",
        "settlement_profile": "vanilla",
        "min_y": -64,
        "height": 384
    });
    std::fs::write(
        world_contract_path(world.path()),
        serde_json::to_vec_pretty(&legacy).unwrap(),
    )
    .unwrap();

    let error = ensure_world_contract_with_spawn(
        world.path(),
        mc_world::OVERWORLD_GEOMETRY,
        712816,
        "tellus_like",
        "vanilla",
        "vanilla",
        mc_world::WorldSpawn::new(320, -192),
        None,
        None,
    )
    .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("unsupported persisted world contract schema 2"));
    assert!(!message.contains("missing field"));
}

#[test]
fn world_contract_persists_and_rejects_changed_spawn() {
    let world = tempfile::tempdir().unwrap();
    let geometry = mc_world::ChunkGeometry::new(-64, 384).unwrap();
    let spawn = mc_world::WorldSpawn::new(320, -192);

    assert_eq!(
        ensure_world_contract_with_spawn(
            world.path(),
            geometry,
            712_816,
            "tellus_like",
            "vanilla",
            "vanilla",
            spawn,
            None,
            None,
        )
        .unwrap(),
        WorldSource::SolarisGenerated,
    );
    let bytes = std::fs::read(world_contract_path(world.path())).unwrap();
    let persisted: PersistedWorldContract = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(persisted.spawn_block_x, spawn.block_x);
    assert_eq!(persisted.spawn_block_z, spawn.block_z);

    let error = ensure_world_contract_with_spawn(
        world.path(),
        geometry,
        712_816,
        "tellus_like",
        "vanilla",
        "vanilla",
        mc_world::WorldSpawn::new(384, -192),
        None,
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("spawn=(320, -192)"));
    assert!(error.to_string().contains("spawn=(384, -192)"));
}

#[test]
fn world_contract_rejects_mismatched_worldgen_revision_before_world_open() {
    let world = tempfile::tempdir().unwrap();
    let geometry = mc_world::ChunkGeometry::new(-64, 384).unwrap();
    ensure_world_contract(
        world.path(),
        geometry,
        7,
        "vanilla_like",
        "vanilla",
        "vanilla",
    )
    .unwrap();

    let path = world_contract_path(world.path());
    let bytes = std::fs::read(&path).unwrap();
    let mut persisted: PersistedWorldContract = serde_json::from_slice(&bytes).unwrap();
    persisted.worldgen_revision = persisted.worldgen_revision.saturating_sub(1);
    std::fs::write(&path, serde_json::to_vec_pretty(&persisted).unwrap()).unwrap();

    let error = ensure_world_contract(
        world.path(),
        geometry,
        7,
        "vanilla_like",
        "vanilla",
        "vanilla",
    )
    .unwrap_err();
    assert!(error.to_string().contains("persisted worldgen revision="));
}

#[test]
fn unversioned_anvil_world_opens_without_solaris_generation() {
    let world = tempfile::tempdir().unwrap();
    let region = world.path().join("region");
    std::fs::create_dir_all(&region).unwrap();
    std::fs::write(
        region.join("r.12.-7.mca"),
        b"not read during metadata preflight",
    )
    .unwrap();

    let geometry = mc_world::ChunkGeometry::new(0, 256).unwrap();
    assert!(!world_requires_solaris_spawn(world.path()).unwrap());
    assert_eq!(
        ensure_world_contract(
            world.path(),
            geometry,
            0,
            "vanilla_like",
            "vanilla",
            "vanilla",
        )
        .unwrap(),
        WorldSource::ExistingVanilla,
    );
    assert!(!world_contract_path(world.path()).exists());
}

#[test]
fn unversioned_anvil_world_rejects_a_plugin_worldgen_profile() {
    let world = tempfile::tempdir().unwrap();
    let region = world.path().join("region");
    std::fs::create_dir_all(&region).unwrap();
    std::fs::write(region.join("r.0.0.mca"), b"not opened during preflight").unwrap();

    let error = ensure_world_contract(
        world.path(),
        mc_world::OVERWORLD_GEOMETRY,
        0,
        "vanilla_like",
        "realistic_deposits",
        "vanilla",
    )
    .unwrap_err();

    assert!(error.to_string().contains("unversioned Anvil import"));
    assert!(!world_contract_path(world.path()).exists());

    let settlement_error = ensure_world_contract(
        world.path(),
        mc_world::OVERWORLD_GEOMETRY,
        0,
        "vanilla_like",
        "vanilla",
        "plains_village_prototype",
    )
    .unwrap_err();
    assert!(
        settlement_error
            .to_string()
            .contains("unversioned Anvil import")
    );
    assert!(!world_contract_path(world.path()).exists());
}

#[test]
fn world_contract_rejects_seed_and_mode_changes() {
    let world = tempfile::tempdir().unwrap();
    let geometry = mc_world::OVERWORLD_GEOMETRY;

    assert_eq!(
        ensure_world_contract(
            world.path(),
            geometry,
            11,
            "vanilla_like",
            "vanilla",
            "vanilla",
        )
        .unwrap(),
        WorldSource::SolarisGenerated,
    );
    assert_eq!(
        ensure_world_contract(
            world.path(),
            geometry,
            11,
            "vanilla_like",
            "vanilla",
            "vanilla",
        )
        .unwrap(),
        WorldSource::SolarisGenerated,
    );

    let seed_error = ensure_world_contract(
        world.path(),
        geometry,
        12,
        "vanilla_like",
        "vanilla",
        "vanilla",
    )
    .unwrap_err();
    assert!(seed_error.to_string().contains("seed=11"));
    assert!(seed_error.to_string().contains("seed=12"));

    let mode_error = ensure_world_contract(
        world.path(),
        geometry,
        11,
        "tellus_like",
        "vanilla",
        "vanilla",
    )
    .unwrap_err();
    assert!(mode_error.to_string().contains("mode=vanilla_like"));
    assert!(mode_error.to_string().contains("mode=tellus_like"));

    let profile_error = ensure_world_contract(
        world.path(),
        geometry,
        11,
        "vanilla_like",
        "realistic_deposits",
        "vanilla",
    )
    .unwrap_err();
    assert!(profile_error.to_string().contains("ore_profile=vanilla"));
    assert!(
        profile_error
            .to_string()
            .contains("ore_profile=realistic_deposits")
    );

    let settlement_error = ensure_world_contract(
        world.path(),
        geometry,
        11,
        "vanilla_like",
        "vanilla",
        "plains_village_prototype",
    )
    .unwrap_err();
    assert!(
        settlement_error
            .to_string()
            .contains("settlement_profile=vanilla")
    );
    assert!(
        settlement_error
            .to_string()
            .contains("settlement_profile=plains_village_prototype")
    );

    assert!(world_contract_path(world.path()).is_file());
}

/// The built-in settlement profile is a first-class world identity: startup
/// writes it, accepts a world that already carries it, and refuses a world
/// whose persisted profile differs instead of generating villages into it.
#[test]
fn world_contract_accepts_and_persists_the_builtin_settlement_profile() {
    let world = tempfile::tempdir().unwrap();
    let geometry = mc_world::OVERWORLD_GEOMETRY;
    let profile = mc_server::SettlementProfile::PlainsVillagePrototype.name();

    assert_eq!(
        ensure_world_contract(
            world.path(),
            geometry,
            712_816,
            "tellus_like",
            "vanilla",
            profile,
        )
        .unwrap(),
        WorldSource::SolarisGenerated,
    );
    let bytes = std::fs::read(world_contract_path(world.path())).unwrap();
    let persisted: PersistedWorldContract = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(persisted.settlement_profile, profile);

    assert_eq!(
        ensure_world_contract(
            world.path(),
            geometry,
            712_816,
            "tellus_like",
            "vanilla",
            profile,
        )
        .unwrap(),
        WorldSource::SolarisGenerated,
    );

    let error = ensure_world_contract(
        world.path(),
        geometry,
        712_816,
        "tellus_like",
        "vanilla",
        "vanilla",
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("settlement_profile=plains_village_prototype"),
        "{error}"
    );
}

#[test]
fn startup_chunk_workers_cover_configured_and_available_parallelism() {
    let available = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);

    assert_eq!(startup_chunk_worker_threads(0), available.max(1));
    assert_eq!(startup_chunk_worker_threads(available + 3), available + 3);
    // A configured bound is the operator's number even when it is below the
    // process CPU count: the bake must not raise it back to every core.
    assert_eq!(startup_chunk_worker_threads(2), 2);
    assert_eq!(startup_light_bake_worker_threads(0), 1);
    assert_eq!(startup_light_bake_worker_threads(2), 4);
    assert_eq!(
        startup_light_bake_worker_threads(available + 3),
        ((available + 3) * 2).min(STARTUP_LIGHT_BAKE_WORKER_CAP)
    );
    assert_eq!(
        startup_light_bake_worker_threads(100),
        STARTUP_LIGHT_BAKE_WORKER_CAP
    );
}

#[test]
fn chest_loot_catalog_for_startup_falls_back_without_data_dir() {
    assert!(
        chest_loot_catalog_for_startup(std::path::Path::new("/nonexistent-data-dir")).is_none()
    );
}

#[test]
fn build_terrain_generator_rejects_missing_required_block() {
    let blocks = terrain_registry_missing_grass_block();
    let err = match build_terrain_generator(
        42,
        mc_worldgen::WorldgenMode::VanillaLike,
        mc_world::OVERWORLD_GEOMETRY,
        blocks,
        mc_worldgen::StructureRules::none(),
        None,
        None,
        None,
    ) {
        Ok(_) => panic!("missing required terrain block must fail"),
        Err(err) => err,
    };

    assert!(
        err.to_string().contains("building terrain generator"),
        "{err:#}"
    );
    assert!(
        format!("{err:#}")
            .contains("block registry missing required terrain block minecraft:grass_block"),
        "{err:#}"
    );
}

#[test]
fn build_terrain_generator_propagates_chunk_geometry() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .unwrap(),
    );
    let geometry = mc_world::ChunkGeometry::new(0, 256).unwrap();
    let generator = build_terrain_generator(
        42,
        mc_worldgen::WorldgenMode::VanillaLike,
        geometry,
        blocks,
        mc_worldgen::StructureRules::none(),
        None,
        None,
        None,
    )
    .unwrap();

    let chunk =
        mc_world::ChunkGenerator::generate(generator.as_ref(), mc_world::ChunkPos { x: 0, z: 0 });

    assert_eq!(chunk.geometry(), geometry);
    assert_eq!(chunk.sections.len(), 16);
}

/// The analogue notice is owed exactly when a build places vanilla
/// villages, and it says what actually changed: the terrain adaptation is
/// the column-height analogue, not vanilla's density arithmetic.
#[test]
fn village_terrain_analogue_notice_fires_only_for_vanilla_villages() {
    assert!(village_terrain_analogue_notice(false).is_none());
    let notice = village_terrain_analogue_notice(true).expect("a village build owes the notice");
    assert_eq!(
        notice.code,
        "village_terrain_adaptation_beard_thin_is_a_column_height_analogue"
    );
    assert!(notice.message.contains("beard_thin"), "{}", notice.message);
    assert!(
        notice.message.contains("column-height analogue"),
        "{}",
        notice.message
    );
    assert!(
        notice
            .message
            .contains("not as vanilla's density arithmetic"),
        "{}",
        notice.message,
    );
}

/// No plan source, no villages: `build_terrain_generator` receives the
/// source from its caller, and this call passes `None` (the shape a world
/// whose profile is not `vanilla`, or where a deployed component plan owns
/// settlement, reaches it with). The generator then places nothing and the
/// terrain-adaptation analogue notice has nothing to report.
#[test]
fn build_terrain_generator_places_no_villages_without_a_plan_source() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .unwrap(),
    );
    let generator = build_terrain_generator(
        42,
        mc_worldgen::WorldgenMode::VanillaLike,
        mc_world::OVERWORLD_GEOMETRY,
        blocks,
        mc_worldgen::StructureRules::none(),
        None,
        None,
        None,
    )
    .unwrap();

    assert!(generator.village_plan_source().is_none());
}

#[test]
fn build_terrain_generator_applies_the_prepared_plugin_ore_profile() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .unwrap(),
    );
    let generator = build_terrain_generator(
        42,
        mc_worldgen::WorldgenMode::VanillaLike,
        mc_world::OVERWORLD_GEOMETRY,
        blocks,
        mc_worldgen::StructureRules::none(),
        Some(mc_script::PluginWorldgenOreProfile::RealisticDeposits),
        None,
        None,
    )
    .unwrap();

    assert_eq!(generator.ore_generation_profile(), "realistic_deposits");
}

#[test]
fn playable_ruin_rules_require_seed_zero_vanilla_like_profile() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .unwrap(),
    );
    let items = mc_data::items::solaris_required_items();

    let playable = structure_rules_for_startup(
        0,
        mc_server::WorldgenMode::VanillaLike,
        std::path::Path::new("resolved-content-cache"),
        &blocks,
        &items,
        None,
        mc_server::SettlementProfile::Vanilla,
    )
    .unwrap();
    let unrelated_seed = structure_rules_for_startup(
        7,
        mc_server::WorldgenMode::VanillaLike,
        std::path::Path::new("resolved-content-cache"),
        &blocks,
        &items,
        None,
        mc_server::SettlementProfile::Vanilla,
    )
    .unwrap();
    let unrelated_mode = structure_rules_for_startup(
        0,
        mc_server::WorldgenMode::TellusLike,
        std::path::Path::new("resolved-content-cache"),
        &blocks,
        &items,
        None,
        mc_server::SettlementProfile::Vanilla,
    )
    .unwrap();

    assert!(!playable.is_empty());
    assert!(unrelated_seed.is_empty());
    assert!(unrelated_mode.is_empty());
}

#[test]
fn settlement_profile_loads_the_extracted_prototype_when_present() {
    let vanilla_data_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let fountain = vanilla_data_dir
        .join("data/minecraft/structure/village/plains/town_centers/plains_fountain_01.nbt");
    if !fountain.is_file() {
        // CI has no Mojang sidecar; the local field run covers this path.
        return;
    }
    let blocks =
        mc_world::BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .unwrap();
    let items = mc_data::items::solaris_required_items();

    let rules = structure_rules_for_startup(
        0,
        mc_server::WorldgenMode::TellusLike,
        &vanilla_data_dir,
        &blocks,
        &items,
        Some(&mc_script::PluginSettlementPlan::plains_village_prototype(
            "test-settlement",
        )),
        mc_server::SettlementProfile::Vanilla,
    )
    .unwrap();

    assert_eq!(rules.templates().len(), 1);
    assert!(rules.templates()[0].blocks().len() > 200);
}

#[test]
fn extracted_village_prototype_generates_deterministically_when_present() {
    let vanilla_data_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let fountain = vanilla_data_dir
        .join("data/minecraft/structure/village/plains/town_centers/plains_fountain_01.nbt");
    if !fountain.is_file() {
        // CI has no Mojang sidecar; the local field run covers this path.
        return;
    }
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .unwrap(),
    );
    let items = mc_data::items::solaris_required_items();
    let rules = structure_rules_for_startup(
        0,
        mc_server::WorldgenMode::TellusLike,
        &vanilla_data_dir,
        &blocks,
        &items,
        Some(&mc_script::PluginSettlementPlan::plains_village_prototype(
            "test-settlement",
        )),
        mc_server::SettlementProfile::Vanilla,
    )
    .unwrap();
    // The built-in profile places the same prototype with no plugin at all.
    let builtin_rules = structure_rules_for_startup(
        0,
        mc_server::WorldgenMode::TellusLike,
        &vanilla_data_dir,
        &blocks,
        &items,
        None,
        mc_server::SettlementProfile::PlainsVillagePrototype,
    )
    .unwrap();
    assert!(
        !builtin_rules.is_empty(),
        "the built-in settlement profile must place village sections"
    );
    let builtin = build_terrain_generator(
        0,
        mc_server::WorldgenMode::TellusLike.to_worldgen(),
        mc_world::OVERWORLD_GEOMETRY,
        Arc::clone(&blocks),
        builtin_rules,
        None,
        None,
        None,
    )
    .unwrap();
    let first = build_terrain_generator(
        0,
        mc_server::WorldgenMode::TellusLike.to_worldgen(),
        mc_world::OVERWORLD_GEOMETRY,
        Arc::clone(&blocks),
        rules.clone(),
        None,
        None,
        None,
    )
    .unwrap();
    let second = build_terrain_generator(
        0,
        mc_server::WorldgenMode::TellusLike.to_worldgen(),
        mc_world::OVERWORLD_GEOMETRY,
        Arc::clone(&blocks),
        rules,
        None,
        None,
        None,
    )
    .unwrap();
    let baseline = build_terrain_generator(
        0,
        mc_server::WorldgenMode::TellusLike.to_worldgen(),
        mc_world::OVERWORLD_GEOMETRY,
        blocks,
        mc_worldgen::StructureRules::none(),
        None,
        None,
        None,
    )
    .unwrap();
    let mut changed = 0;
    for chunk_x in 3..=5 {
        for chunk_z in -1..=1 {
            let pos = mc_world::ChunkPos {
                x: chunk_x,
                z: chunk_z,
            };
            let first_chunk = mc_world::ChunkGenerator::generate(first.as_ref(), pos);
            let second_chunk = mc_world::ChunkGenerator::generate(second.as_ref(), pos);
            let baseline_chunk = mc_world::ChunkGenerator::generate(baseline.as_ref(), pos);
            for y in mc_world::OVERWORLD_GEOMETRY.min_y()..mc_world::OVERWORLD_GEOMETRY.max_y() {
                for local_z in 0..16 {
                    for local_x in 0..16 {
                        let generated = first_chunk.get_block(local_x, y, local_z);
                        assert_eq!(
                            generated,
                            second_chunk.get_block(local_x, y, local_z),
                            "same profile and seed must reproduce every village block"
                        );
                        changed +=
                            usize::from(generated != baseline_chunk.get_block(local_x, y, local_z));
                    }
                }
            }
        }
    }
    assert!(changed > 200, "prototype changed only {changed} blocks");

    let mut builtin_changed = 0;
    for chunk_x in 3..=5 {
        for chunk_z in -1..=1 {
            let pos = mc_world::ChunkPos {
                x: chunk_x,
                z: chunk_z,
            };
            let builtin_chunk = mc_world::ChunkGenerator::generate(builtin.as_ref(), pos);
            let baseline_chunk = mc_world::ChunkGenerator::generate(baseline.as_ref(), pos);
            for y in mc_world::OVERWORLD_GEOMETRY.min_y()..mc_world::OVERWORLD_GEOMETRY.max_y() {
                for local_z in 0..16 {
                    for local_x in 0..16 {
                        builtin_changed += usize::from(
                            builtin_chunk.get_block(local_x, y, local_z)
                                != baseline_chunk.get_block(local_x, y, local_z),
                        );
                    }
                }
            }
        }
    }
    assert!(
        builtin_changed > 200,
        "built-in village changed only {builtin_changed} blocks"
    );
}

#[tokio::test]
async fn production_bound_server_performs_final_save_after_internal_shutdown() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&[]).unwrap());
    let items = Arc::new(mc_data::items::ItemRegistry::default());
    let world = Arc::new(tokio::sync::Mutex::new(
        mc_world::WorldStorage::open(tmp.path(), Arc::clone(&blocks))
            .unwrap()
            .with_item_registry(Arc::clone(&items)),
    ));
    let shutdown = mc_net::ShutdownHandle::default();
    let config = mc_net::ServerConfig {
        tab_list: mc_net::TabListConfig::default(),
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "shutdown phase test".into(),
        max_players: 0,
        view_distance: 0,
        data: Arc::new(mc_data::testing::stub()),
        blocks,
        world: Some(world),
        tags: Arc::new(mc_data::tags::TagsData::default()),
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), false),
        loader_manifest: None,
        shutdown: shutdown.clone(),
    };
    let bound = mc_net::bind(config).await.expect("bind");
    let metadata = tmp.path().join("solaris").join("world.dat");
    shutdown.request();
    let config_path = tmp.path().join("unused-config.toml");
    run_bound_server(bound, shutdown, &config_path, None, false, None)
        .await
        .expect("production entrypoint drains and performs its sole final save");
    assert!(metadata.exists());
}
