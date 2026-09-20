use super::super::*;
use super::support::{stub_blocks, stub_tags};

#[test]
fn translates_to_network_config() {
    let toml_src = r#"
        [server]
        name = "S"
        motd = "Howdy"
        max_players = 50
        view_distance = 7
        simulation_distance = 5

        [network]
        bind_address = "127.0.0.1"
        port = 25000

        [data]
        world_dir = "/tmp/world"
    "#;
    let cfg: ServerConfig = toml::from_str(toml_src).unwrap();
    let data = Arc::new(mc_data::testing::stub());
    let net = cfg
        .to_network(
            data,
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
    assert_eq!(net.motd, "Howdy");
    assert_eq!(net.max_players, 50);
    assert_eq!(net.view_distance, 7);
    assert_eq!(net.random_tick.simulation_distance, 5);
    assert_eq!(net.bind_address.port(), 25000);
    assert!(net.world.is_none());
    assert_eq!(net.chunk_pipeline.region_cache_size, 4);
    let runtime = net
        .chunk_pipeline
        .runtime_control
        .expect("default config wires runtime autoscale");
    assert_eq!(runtime.initial_limits.view_distance, 7);
    assert_eq!(cfg.data.world_dir, Some(PathBuf::from("/tmp/world")));
}

#[test]
fn invalid_bind_address_is_rejected() {
    let toml_src = r#"
        [server]
        name = "S"
        motd = ""

        [network]
        bind_address = "not-an-ip"
        port = 25565
    "#;
    let cfg: ServerConfig = toml::from_str(toml_src).unwrap();
    let data = Arc::new(mc_data::testing::stub());
    assert!(
        cfg.to_network(
            data,
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
            Arc::new(BiomeSpawnRules::default())
        )
        .is_err()
    );
}
