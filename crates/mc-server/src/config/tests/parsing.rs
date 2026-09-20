use super::super::*;
use super::support::{stub_blocks, stub_tags};

#[test]
fn settlement_profile_defaults_to_vanilla_and_parses_the_builtin() {
    let default: ServerConfig = toml::from_str(
        r#"
        [server]
        name = "Settlement"
        motd = "Settlement"
        [network]
        bind_address = "127.0.0.1"
        port = 0
        "#,
    )
    .unwrap();
    assert_eq!(default.data.settlement_profile, SettlementProfile::Vanilla);
    assert_eq!(default.data.settlement_profile.name(), "vanilla");

    let configured: ServerConfig = toml::from_str(
        r#"
        [server]
        name = "Settlement"
        motd = "Settlement"
        [network]
        bind_address = "127.0.0.1"
        port = 0
        [data]
        settlement_profile = "plains_village_prototype"
        "#,
    )
    .unwrap();
    assert_eq!(
        configured.data.settlement_profile,
        SettlementProfile::PlainsVillagePrototype
    );
    assert_eq!(
        configured.data.settlement_profile.name(),
        "plains_village_prototype"
    );
}

#[test]
fn dashboard_section_is_default_off_loopback_and_optional() {
    let config: ServerConfig = toml::from_str(
        r#"
        [server]
        name = "s"
        motd = "m"
        [network]
        bind_address = "127.0.0.1"
        port = 25565
        "#,
    )
    .expect("minimal config parses without a dashboard section");
    assert!(!config.dashboard.enabled);
    assert_eq!(config.dashboard.bind_address, "127.0.0.1");
    assert_eq!(config.dashboard.port, 8080);
    assert!(!config.dashboard.allow_remote);
    let socket = config
        .dashboard
        .validate()
        .expect("loopback default validates");
    assert!(socket.ip().is_loopback());
}

#[test]
fn dashboard_section_rejects_remote_bind_without_explicit_allow() {
    let config: ServerConfig = toml::from_str(
        r#"
        [server]
        name = "s"
        motd = "m"
        [network]
        bind_address = "127.0.0.1"
        port = 25565
        [dashboard]
        enabled = true
        bind_address = "0.0.0.0"
        port = 8080
        "#,
    )
    .expect("remote dashboard config parses");
    let error = config
        .dashboard
        .validate()
        .expect_err("remote bind refused");
    assert!(error.contains("allow_remote"));

    let allowed: ServerConfig = toml::from_str(
        r#"
        [server]
        name = "s"
        motd = "m"
        [network]
        bind_address = "127.0.0.1"
        port = 25565
        [dashboard]
        enabled = true
        bind_address = "0.0.0.0"
        port = 8080
        allow_remote = true
        "#,
    )
    .expect("explicit remote dashboard parses");
    assert!(allowed.dashboard.validate().is_ok());
}

#[test]
fn dashboard_section_rejects_invalid_bind_address() {
    let config: ServerConfig = toml::from_str(
        r#"
        [server]
        name = "s"
        motd = "m"
        [network]
        bind_address = "127.0.0.1"
        port = 25565
        [dashboard]
        bind_address = "not-an-ip"
        "#,
    )
    .expect("dashboard config parses before validation");
    assert!(config.dashboard.validate().is_err());
}

#[test]
fn parses_playable_profile_as_loopback_survival_spike() {
    let cfg: ServerConfig =
        toml::from_str(include_str!("../../../../../playable.toml")).expect("parse playable.toml");

    assert_eq!(cfg.server.name, "solaris-playable");
    assert_eq!(cfg.server.motd, "Solaris playable spike");
    assert_eq!(cfg.server.view_distance, 4);
    assert_eq!(cfg.server.simulation_distance, 4);
    assert_eq!(cfg.network.bind_address, "127.0.0.1");
    assert_eq!(cfg.network.port, 25565);
    assert_eq!(
        cfg.data.world_dir,
        Some(std::path::PathBuf::from(".analysis/test-world-v11"))
    );
    assert_eq!(
        cfg.data.vanilla_data_dir,
        Some(std::path::PathBuf::from("data/vanilla"))
    );
    assert_eq!(cfg.data.seed, 0);
    assert_eq!(cfg.data.worldgen_mode, WorldgenMode::TellusLike);
    assert!(!cfg.auth.online_mode);
    assert!(!cfg.auth.prevent_proxy_connections);
    assert!(!cfg.auth.whitelist_enabled);
    assert!(cfg.auth.whitelist.is_empty());
    assert!(cfg.auth.banned_players.is_empty());
    assert!(cfg.admin.operators.is_empty());
    assert!(!cfg.admin.allow_local_dev_operators);
    assert_eq!(cfg.simulation.random_tick_speed, 5);
    assert_eq!(cfg.simulation.save_interval_ticks, 1200);
    assert_eq!(cfg.simulation.friendly_spawn_interval_ticks, 400);
    assert_eq!(cfg.simulation.hostile_spawn_interval_ticks, 20);
    assert_eq!(cfg.simulation.friendly_spawn_chunk_budget, 48);
    assert_eq!(cfg.simulation.hostile_spawn_chunk_budget, 4);
    assert_eq!(cfg.chunk_pipeline.chunk_send_rate, 8);
    assert_eq!(cfg.chunk_pipeline.chunk_load_rate, 16);
    assert_eq!(cfg.chunk_pipeline.chunk_generate_rate, 16);
    assert_eq!(cfg.chunk_pipeline.chunk_result_queue_size, 64);
    assert_eq!(cfg.chunk_pipeline.region_cache_size, 9);
    assert!(cfg.autoscale.enabled);
    assert_eq!(cfg.autoscale.min_view_distance, Some(4));
    assert_eq!(cfg.autoscale.max_view_distance, Some(4));
    let autoscale_policy = cfg.autoscale.to_policy(&cfg.server, &cfg.chunk_pipeline);
    assert_eq!(autoscale_policy.min_view_distance, 4);
    assert_eq!(autoscale_policy.max_view_distance, 4);
}

#[test]
fn parses_example_config_shape() {
    let toml_src = r#"
        [server]
        name = "S"
        motd = "M"

        [network]
        bind_address = "0.0.0.0"
        port = 25565
    "#;
    let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
    assert_eq!(cfg.server.name, "S");
    assert_eq!(cfg.server.max_players, 20);
    assert_eq!(cfg.server.view_distance, 10);
    assert_eq!(cfg.server.simulation_distance, 10);
    assert_eq!(cfg.network.port, 25565);
    assert_eq!(cfg.chunk_pipeline.chunk_prepare_batch_size, 8);
    assert_eq!(cfg.simulation.random_tick_speed, 3);
    assert_eq!(cfg.simulation.save_interval_ticks, 20);
    assert_eq!(cfg.simulation.friendly_spawn_interval_ticks, 400);
    assert_eq!(cfg.simulation.hostile_spawn_interval_ticks, 20);
    assert!(cfg.data.vanilla_data_dir.is_none());
    assert_eq!(cfg.data.worldgen_mode, WorldgenMode::TellusLike);
    assert_eq!(
        cfg.data.chunk_geometry().unwrap(),
        mc_world::OVERWORLD_GEOMETRY
    );
    assert!(!cfg.admin.allow_local_dev_operators);
    assert!(!cfg.auth.online_mode);
    assert!(!cfg.auth.prevent_proxy_connections);
    assert!(!cfg.auth.whitelist_enabled);
    assert!(cfg.autoscale.enabled);
    assert_eq!(cfg.autoscale.profile, AutoscaleProfile::Balanced);
}

#[test]
fn parses_ip_bound_online_authentication() {
    let cfg: ServerConfig = toml::from_str(
        r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 25565

            [auth]
            online_mode = true
            prevent_proxy_connections = true
        "#,
    )
    .expect("parse IP-bound online authentication");

    assert!(cfg.auth.online_mode);
    assert!(cfg.auth.prevent_proxy_connections);
}

#[test]
fn parses_explicit_chunk_geometry_and_rejects_invalid_ranges() {
    let toml_src = r#"
        [server]
        name = "S"
        motd = "M"

        [network]
        bind_address = "127.0.0.1"
        port = 25565

        [data]
        min_y = 0
        height = 256
    "#;
    let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
    let geometry = cfg.data.chunk_geometry().expect("valid geometry");
    assert_eq!(geometry.min_y(), 0);
    assert_eq!(geometry.height(), 256);

    let invalid = DataSection {
        min_y: 1,
        height: 255,
        ..DataSection::default()
    };
    assert!(invalid.chunk_geometry().is_err());
}

#[test]
fn parses_component_plugin_deployment_configuration() {
    let toml_src = r#"
        [server]
        name = "S"
        motd = "M"

        [network]
        bind_address = "127.0.0.1"
        port = 25565

        [plugins]
        directory = "plugins"
        strict = true
        expected = ["basic-economy", "online-roster"]
    "#;
    let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");

    assert_eq!(cfg.plugins.directory, Some(PathBuf::from("plugins")));
    assert!(cfg.plugins.strict);
    assert_eq!(
        cfg.plugins.expected,
        ["basic-economy".to_owned(), "online-roster".to_owned()]
    );
    let error = toml::from_str::<ServerConfig>(
        r#"
            [plugins]
            runtime = "wasm"
        "#,
    )
    .expect_err("the removed plugin runtime selector must be rejected");
    assert!(error.to_string().contains("unknown field `runtime`"));
}

#[test]
fn loader_live_gate_config_is_isolated_and_parseable() {
    let cfg: ServerConfig = toml::from_str(include_str!(
        "../../../../../examples/loader-live-gate/playable.toml"
    ))
    .expect("parse Loader live-gate config");

    // The gate's isolation, which is what this case is for: its own port, an
    // analysis workspace it may be deleted from, offline auth and no
    // autoscaling. Where the gate's plugin directory is packaged is the
    // packaging script's business, so only the fact that it deploys one is
    // asserted here.
    assert_eq!(cfg.network.port, 25567);
    assert!(
        cfg.plugins.directory.is_some(),
        "the gate deploys a plugin directory"
    );
    assert_eq!(
        cfg.data.world_dir,
        Some(PathBuf::from(".analysis/loader-live-gate/world"))
    );
    assert!(!cfg.auth.online_mode);
    assert!(!cfg.autoscale.enabled);
}

#[test]
fn parses_explicit_auth_and_admin_policy() {
    let toml_src = r#"
        [server]
        name = "S"
        motd = "M"

        [network]
        bind_address = "127.0.0.1"
        port = 25565

        [admin]
        operators = ["Notch"]
        allow_local_dev_operators = true

        [auth]
        online_mode = false
        whitelist_enabled = true
        whitelist = ["Notch"]
        banned_players = ["BadActor"]
    "#;
    let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
    assert_eq!(cfg.admin.operators, ["Notch"]);
    assert!(cfg.admin.allow_local_dev_operators);
    assert!(cfg.auth.whitelist_enabled);
    assert_eq!(cfg.auth.whitelist, ["Notch"]);
    assert_eq!(cfg.auth.banned_players, ["BadActor"]);
}

#[test]
fn parses_optional_vanilla_data_dir() {
    let toml_src = r#"
        [server]
        name = "S"
        motd = "M"

        [network]
        bind_address = "0.0.0.0"
        port = 25565

        [data]
        vanilla_data_dir = "data/vanilla"
    "#;
    let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
    assert_eq!(
        cfg.data.vanilla_data_dir,
        Some(PathBuf::from("data/vanilla"))
    );
}

#[test]
fn parses_explicit_tellus_like_worldgen_config() {
    let toml_src = r#"
        [server]
        name = "S"
        motd = "M"

        [network]
        bind_address = "0.0.0.0"
        port = 25565

        [data]
        worldgen_mode = "tellus_like"
    "#;
    let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
    assert_eq!(cfg.data.worldgen_mode, WorldgenMode::TellusLike);
}

#[test]
fn data_section_rejects_removed_vanilla_dir() {
    let toml_src = r#"
        [server]
        name = "S"
        motd = "M"

        [network]
        bind_address = "0.0.0.0"
        port = 25565

        [data]
        vanilla_dir = "data/vanilla"
    "#;

    let err = toml::from_str::<ServerConfig>(toml_src).unwrap_err();
    assert!(err.to_string().contains("unknown field `vanilla_dir`"));
}

#[test]
fn server_section_rejects_unknown_fields() {
    let toml_src = r#"
        [server]
        name = "S"
        motd = "M"
        online_mode = false

        [network]
        bind_address = "127.0.0.1"
        port = 25565
    "#;

    let err = toml::from_str::<ServerConfig>(toml_src).unwrap_err();
    assert!(err.to_string().contains("unknown field `online_mode`"));
}

#[test]
fn network_section_rejects_unknown_fields() {
    let toml_src = r#"
        [server]
        name = "S"
        motd = "M"

        [network]
        bind_address = "127.0.0.1"
        port = 25565
        online_mode = false
    "#;

    let err = toml::from_str::<ServerConfig>(toml_src).unwrap_err();
    assert!(err.to_string().contains("unknown field `online_mode`"));
}

#[test]
fn chunk_pipeline_rejects_removed_worker_percentages() {
    let toml_src = r#"
        [server]
        name = "S"
        motd = "M"

        [network]
        bind_address = "127.0.0.1"
        port = 25565

        [chunk_pipeline]
        chunk_worker_threads_percent = 75
    "#;

    let err = toml::from_str::<ServerConfig>(toml_src).unwrap_err();
    assert!(
        err.to_string()
            .contains("unknown field `chunk_worker_threads_percent`")
    );
}

#[test]
fn admin_section_rejects_unknown_fields() {
    let toml_src = r#"
        [server]
        name = "S"
        motd = "M"

        [network]
        bind_address = "127.0.0.1"
        port = 25565

        [admin]
        operators = []
        op_everyone = true
    "#;

    let err = toml::from_str::<ServerConfig>(toml_src).unwrap_err();
    assert!(err.to_string().contains("unknown field `op_everyone`"));
}

#[test]
fn auth_section_rejects_unknown_fields() {
    let toml_src = r#"
        [server]
        name = "S"
        motd = "M"

        [network]
        bind_address = "127.0.0.1"
        port = 25565

        [auth]
        online_mode = false
        ops = ["Notch"]
    "#;

    let err = toml::from_str::<ServerConfig>(toml_src).unwrap_err();
    assert!(err.to_string().contains("unknown field `ops`"));
}

#[test]
fn parses_chunk_pipeline_overrides() {
    let toml_src = r#"
        [server]
        name = "S"
        motd = "M"

        [network]
        bind_address = "0.0.0.0"
        port = 25565

        [chunk_pipeline]
        chunk_send_rate = 12
        chunk_load_rate = 8
        chunk_generate_rate = 4
        chunk_prepare_budget_ms = 3
        chunk_prepare_batch_size = 2
        chunk_result_queue_size = 9
        region_cache_size = 7
        compression_threshold = 128
        compression_level = 6
    "#;
    let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
    assert_eq!(cfg.chunk_pipeline.chunk_send_rate, 12);
    assert_eq!(cfg.chunk_pipeline.chunk_load_rate, 8);
    assert_eq!(cfg.chunk_pipeline.chunk_generate_rate, 4);
    assert_eq!(cfg.chunk_pipeline.chunk_prepare_budget_ms, 3);
    assert_eq!(cfg.chunk_pipeline.chunk_prepare_batch_size, 2);
    assert_eq!(cfg.chunk_pipeline.chunk_result_queue_size, 9);
    assert_eq!(cfg.chunk_pipeline.region_cache_size, 7);
    assert_eq!(cfg.chunk_pipeline.compression_threshold, 128);
    assert_eq!(cfg.chunk_pipeline.compression_level, Some(6));
}

#[test]
fn chunk_pipeline_normalizes_zero_values_for_runtime() {
    let section = ChunkPipelineSection {
        chunk_send_rate: 0,
        chunk_load_rate: 0,
        chunk_generate_rate: 0,
        chunk_prepare_budget_ms: 0,
        chunk_prepare_batch_size: 0,
        chunk_result_queue_size: 0,
        region_cache_size: 0,
        compression_threshold: -1,
        compression_level: Some(99),
        worker_threads: 0,
    };
    let policy = section.to_network();
    assert_eq!(policy.chunk_send_rate, 1);
    assert_eq!(policy.chunk_load_rate, 1);
    assert_eq!(policy.chunk_generate_rate, 1);
    assert_eq!(policy.chunk_prepare_batch_size, 1);
    let defaults = mc_net::ChunkPipelinePolicy::default();
    assert_eq!(policy.chunk_io_threads, defaults.chunk_io_threads);
    assert_eq!(policy.chunk_worker_threads, defaults.chunk_worker_threads);
    assert_eq!(policy.chunk_result_queue_size, 1);
    assert_eq!(policy.region_cache_size, 1);
    assert_eq!(policy.compression_threshold, 0);
    assert_eq!(policy.compression_level, Some(9));
}

#[test]
fn chunk_worker_threads_bound_replaces_the_derived_split() {
    let defaults = mc_net::ChunkPipelinePolicy::default();
    let derived = ChunkPipelineSection::default().to_network();
    assert_eq!(derived.chunk_io_threads, defaults.chunk_io_threads);
    assert_eq!(derived.chunk_worker_threads, defaults.chunk_worker_threads);

    let bounded = ChunkPipelineSection {
        worker_threads: 2,
        ..ChunkPipelineSection::default()
    }
    .to_network();
    assert_eq!(bounded.chunk_worker_threads, 2);
    assert_eq!(bounded.chunk_io_threads, 1);
    assert_eq!(bounded.chunk_send_rate, derived.chunk_send_rate);

    // A zero bound is the derived split, not a one-thread pipeline.
    let zero = ChunkPipelineSection {
        worker_threads: 0,
        ..ChunkPipelineSection::default()
    }
    .to_network();
    assert_eq!(zero.chunk_worker_threads, defaults.chunk_worker_threads);
}

#[test]
fn parses_simulation_overrides() {
    let toml_src = r#"
        [server]
        name = "S"
        motd = "M"

        [network]
        bind_address = "0.0.0.0"
        port = 25565

        [simulation]
        random_tick_speed = 7
        save_interval_ticks = 40
        friendly_spawn_interval_ticks = 800
        hostile_spawn_interval_ticks = 0
        friendly_spawn_chunk_budget = 20
        hostile_spawn_chunk_budget = 6
    "#;
    let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");

    assert_eq!(cfg.simulation.random_tick_speed, 7);
    assert_eq!(cfg.simulation.save_interval_ticks, 40);
    assert_eq!(cfg.simulation.friendly_spawn_interval_ticks, 800);
    assert_eq!(cfg.simulation.hostile_spawn_interval_ticks, 0);
    assert_eq!(cfg.simulation.friendly_spawn_chunk_budget, 20);
    assert_eq!(cfg.simulation.hostile_spawn_chunk_budget, 6);
}

#[test]
fn parses_autoscale_overrides_and_builds_bounded_policy() {
    let toml_src = r#"
        [server]
        name = "S"
        motd = "M"
        view_distance = 8

        [network]
        bind_address = "0.0.0.0"
        port = 25565

        [chunk_pipeline]
        chunk_send_rate = 8
        chunk_load_rate = 16
        chunk_generate_rate = 16

        [autoscale]
        enabled = true
        profile = "low_end"
        min_view_distance = 3
        max_view_distance = 6
        target_tick_ms = 45
        target_first_chunk_ms = 1200
        scale_down_after_seconds = 90
        scale_up_after_seconds = 120
    "#;
    let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
    let policy = cfg.autoscale.to_policy(&cfg.server, &cfg.chunk_pipeline);
    let limits = cfg
        .autoscale
        .initial_limits(&cfg.server, &cfg.chunk_pipeline);

    assert!(cfg.autoscale.enabled);
    assert_eq!(cfg.autoscale.profile, AutoscaleProfile::LowEnd);
    assert_eq!(policy.min_view_distance, 3);
    assert_eq!(policy.max_view_distance, 6);
    assert_eq!(policy.target_tick_ms, 45);
    assert_eq!(policy.target_first_chunk_ms, 1200);
    assert_eq!(policy.scale_down_after_seconds, 90);
    assert_eq!(policy.scale_up_after_seconds, 120);
    assert_eq!(limits.view_distance, 6);
    assert_eq!(limits.chunk_send_rate, 8);
    assert_eq!(limits.chunk_load_rate, 16);
    assert_eq!(limits.chunk_generate_rate, 16);

    let net = cfg
        .to_network(
            Arc::new(mc_data::testing::stub()),
            stub_blocks(),
            None,
            stub_tags(),
            Arc::new(Vec::new()),
            Arc::new(LootTables::default()),
            None,
            Arc::new(ItemRegistry::default()),
            Arc::new(ItemFactsTable::default()),
            Arc::new(BlockFactsTable::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Arc::new(BiomeSpawnRules::default()),
        )
        .unwrap();
    let runtime = net
        .chunk_pipeline
        .runtime_control
        .expect("enabled autoscale wires runtime control");
    assert_eq!(runtime.policy, policy);
    assert_eq!(runtime.initial_limits, limits);
}

#[test]
fn autoscale_default_bounds_preserve_configured_view_sixteen() {
    let mut config: ServerConfig = toml::from_str(
        r#"
        [server]
        name = "S"
        motd = "M"
        view_distance = 16
        simulation_distance = 16
        [network]
        bind_address = "127.0.0.1"
        port = 25565
    "#,
    )
    .expect("server configuration parses");
    let policy = config
        .autoscale
        .to_policy(&config.server, &config.chunk_pipeline);
    let initial = config
        .autoscale
        .initial_limits(&config.server, &config.chunk_pipeline);
    let controller = mc_net::RuntimeControlPlane::new(policy, initial);
    assert_eq!(controller.snapshot().limits.view_distance, 16);
    assert_eq!(policy.max_view_distance, 16);
    assert!(policy.min_view_distance < 16);

    config.autoscale.max_view_distance = Some(4);
    let bounded = config
        .autoscale
        .initial_limits(&config.server, &config.chunk_pipeline);
    assert_eq!(bounded.view_distance, 4);

    config.autoscale.min_view_distance = Some(20);
    config.autoscale.max_view_distance = Some(24);
    let bounded = config
        .autoscale
        .initial_limits(&config.server, &config.chunk_pipeline);
    assert_eq!(bounded.view_distance, 20);
}

#[test]
fn autoscale_section_rejects_unknown_fields() {
    let toml_src = r#"
        [server]
        name = "S"
        motd = "M"

        [network]
        bind_address = "0.0.0.0"
        port = 25565

        [autoscale]
        enabled = false
        unexpected = true
    "#;

    let err = toml::from_str::<ServerConfig>(toml_src).unwrap_err();
    assert!(err.to_string().contains("unknown field `unexpected`"));
}

#[test]
fn simulation_normalizes_runtime_budget() {
    let section = SimulationSection {
        random_tick_speed: 0,
        save_interval_ticks: 0,
        friendly_spawn_interval_ticks: 0,
        hostile_spawn_interval_ticks: 0,
        friendly_spawn_chunk_budget: 0,
        hostile_spawn_chunk_budget: usize::MAX,
    };
    let policy = section.to_network(42, 5);

    assert_eq!(policy.simulation_distance, 5);
    assert_eq!(policy.random_tick_speed, 0);
    assert_eq!(policy.chunk_budget, 64);
    assert_eq!(policy.fluid_tick_budget, 256);
    assert_eq!(policy.save_interval_ticks, 1);
    assert_eq!(policy.friendly_spawn_interval_ticks, 0);
    assert_eq!(policy.hostile_spawn_interval_ticks, 0);
    assert_eq!(policy.friendly_spawn_chunk_budget, 1);
    assert_eq!(
        policy.hostile_spawn_chunk_budget,
        mc_net::MAX_NATURAL_SPAWN_CHUNK_BUDGET
    );
    assert_eq!(policy.seed, 42);
}

#[test]
fn simulation_rejects_removed_manual_work_budgets() {
    let error = toml::from_str::<ServerConfig>(
        r#"
            [simulation]
            random_tick_chunk_budget = 11
            scheduled_fluid_tick_budget = 13
        "#,
    )
    .expect_err("runtime work budgets must belong to autoscale");

    let message = error.to_string();
    assert!(
        message.contains("random_tick_chunk_budget")
            || message.contains("scheduled_fluid_tick_budget")
    );
}
