use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundCommands, ConfirmTeleportation, GameEvent, LoginPlay, SetCenterChunk,
    SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;
use mc_test_harness::parity::{
    CoreActionGenerator, CoreActionSequenceScenario, ObservationFact, ObservationSet,
    OracleAvailability, ParityScenario, ScenarioContext, ScenarioFuture, ServerKind,
    VanillaServerProcess, diff_observations, vanilla_oracle_availability,
};

struct SpawnSmokeScenario;

impl ParityScenario for SpawnSmokeScenario {
    fn name(&self) -> &'static str {
        "spawn-smoke"
    }

    fn run<'a>(&'a self, ctx: ScenarioContext) -> ScenarioFuture<'a> {
        Box::pin(async move { observe_spawn_smoke(ctx).await })
    }
}

async fn observe_spawn_smoke(ctx: ScenarioContext) -> Result<ObservationSet> {
    let subject = match ctx.kind {
        ServerKind::Solaris => "solaris",
        ServerKind::Vanilla => "vanilla",
    };
    let mut client = Client::connect(ctx.addr).await?;
    let _login = client.drive_login(ctx.addr, subject).await?;
    client.drive_configuration().await?;

    let mut observations = ObservationSet::new(subject, "spawn-smoke");
    let _: LoginPlay = client.read_typed().await?;
    observations.push(ObservationFact::PacketSeen { id: LoginPlay::ID });
    let _: ClientboundCommands = client.read_typed().await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundCommands::ID,
    });
    let sync: SynchronizePlayerPosition = client.read_typed().await?;
    observations.push(ObservationFact::PacketSeen {
        id: SynchronizePlayerPosition::ID,
    });
    observations.push(ObservationFact::SpawnPosition {
        x: sync.x.floor() as i64,
        y: sync.y.floor() as i64,
        z: sync.z.floor() as i64,
    });
    let event: GameEvent = client.read_typed().await?;
    observations.push(ObservationFact::PacketSeen { id: GameEvent::ID });
    observations.push(ObservationFact::Note {
        key: "start_waiting_for_chunks".into(),
        value: (event.event == GameEvent::EVENT_START_WAITING_FOR_CHUNKS).to_string(),
    });
    let center: SetCenterChunk = client.read_typed().await?;
    observations.push(ObservationFact::PacketSeen {
        id: SetCenterChunk::ID,
    });
    observations.push(ObservationFact::Note {
        key: "center_chunk".into(),
        value: format!("{},{}", center.chunk_x, center.chunk_z),
    });
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await?;
    Ok(observations.normalized())
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

async fn spawn_solaris() -> Result<(mc_net::BoundServer, std::net::SocketAddr)> {
    let data = Arc::new(mc_data::solaris_required_data());
    let blocks_report = mc_data::blocks::solaris_required_blocks_report();
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&blocks_report)?);
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(Arc::clone(&blocks), 128)
        .with_generator(generator);
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let items = Arc::new(mc_data::items::solaris_required_items());
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse()?,
        motd: "M51 parity oracle".into(),
        max_players: 8,
        view_distance: 2,
        data,
        blocks,
        world,
        tags: Arc::new(mc_data::tags::solaris_required_item_tags(&items)),
        recipes: Arc::new(mc_data::recipes::solaris_required_recipes()),
        loot: Arc::new(mc_data::loot::builtin().clone()),
        block_light: Some(Arc::new(
            mc_data::block_light::BlockLightTable::conservative_from_blocks_report(&blocks_report),
        )),
        items,
        item_facts: Arc::new(mc_data::item_components::solaris_required_item_facts()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &blocks_report,
        )),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::solaris_required_biome_spawn_rules()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy {
            chunk_send_rate: 8,
            chunk_load_rate: 8,
            chunk_generate_rate: 8,
            chunk_prepare_budget_ms: 5,
            chunk_prepare_batch_size: 8,
            chunk_io_threads: 1,
            chunk_worker_threads: 2,
            entity_worker_threads: 1,
            chunk_result_queue_size: 64,
            region_cache_size: 4,
            compression_threshold: 256,
            compression_level: None,
        },
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await?;
    let addr = bound.local_addr()?;
    Ok((bound, addr))
}

#[test]
fn missing_vanilla_oracle_is_an_explicit_skip_not_successful_comparison() {
    let temp = tempfile::tempdir().expect("tempdir");
    let availability = vanilla_oracle_availability(temp.path());
    assert!(matches!(availability, OracleAvailability::Missing { .. }));
    assert!(
        availability
            .skip_message()
            .expect("skip message")
            .contains("server.jar")
    );
}

#[tokio::test]
async fn solaris_spawn_smoke_scenario_produces_normalized_observations() {
    let (bound, addr) = spawn_solaris().await.expect("spawn Solaris");
    let task = tokio::spawn(async move { bound.serve().await });

    let scenario = SpawnSmokeScenario;
    let observations = scenario
        .run(ScenarioContext {
            kind: ServerKind::Solaris,
            addr,
        })
        .await
        .expect("scenario runs");

    assert_eq!(scenario.name(), "spawn-smoke");
    assert!(observations.facts().contains(&ObservationFact::PacketSeen {
        id: SynchronizePlayerPosition::ID,
    }));
    assert!(observations.facts().iter().any(|fact| matches!(
        fact,
        ObservationFact::Note { key, value }
            if key == "start_waiting_for_chunks" && value == "true"
    )));

    task.abort();
}

#[tokio::test]
async fn solaris_core_action_sequence_samples_inventory_after_actions() {
    let (bound, addr) = spawn_solaris().await.expect("spawn Solaris");
    let task = tokio::spawn(async move { bound.serve().await });

    let actions = CoreActionGenerator::generate(0x54, 3);
    assert_eq!(
        actions
            .iter()
            .map(|action| action.summary())
            .collect::<Vec<_>>(),
        vec!["move:167,76", "look:-62,-89", "wait:4"]
    );
    let scenario = CoreActionSequenceScenario::new("seeded-core-actions", actions);
    let observations = scenario
        .run(ScenarioContext {
            kind: ServerKind::Solaris,
            addr,
        })
        .await
        .expect("core action scenario runs");

    assert_eq!(scenario.name(), "seeded-core-actions");
    assert!(observations.facts().contains(&ObservationFact::PacketSeen {
        id: SynchronizePlayerPosition::ID,
    }));
    assert!(observations.facts().iter().any(|fact| matches!(
        fact,
        ObservationFact::Note { key, value }
            if key == "actions_executed" && value == "3"
    )));
    assert!(observations.facts().iter().any(|fact| matches!(
        fact,
        ObservationFact::Note { key, value }
            if key == "post_action_liveness" && value == "clientbound_frame"
    )));
    assert!(observations.facts().iter().any(|fact| matches!(
        fact,
        ObservationFact::InventoryContent {
            container_id: 0,
            slots: 46,
            ..
        }
    )));
    assert!(observations.facts().iter().any(|fact| matches!(
        fact,
        ObservationFact::Note { key, value }
            if key == "action.0" && value == "move:167,76"
    )));
    assert!(observations.facts().iter().any(|fact| matches!(
        fact,
        ObservationFact::Note { key, value }
            if key == "action.1" && value == "look:-62,-89"
    )));
    assert!(observations.facts().iter().any(|fact| matches!(
        fact,
        ObservationFact::Note { key, value }
            if key == "action.2" && value == "wait:4"
    )));

    task.abort();
}

#[tokio::test]
#[ignore = "requires local .analysis/server.jar vanilla oracle and Java"]
async fn vanilla_and_solaris_spawn_smoke_can_be_diffed() {
    let availability = vanilla_oracle_availability(repo_root());
    let OracleAvailability::Available { jar } = availability else {
        eprintln!("{}", availability.skip_message().expect("skip message"));
        return;
    };

    let vanilla_dir = tempfile::tempdir().expect("vanilla tempdir");
    let vanilla = VanillaServerProcess::launch(&jar, vanilla_dir.path(), Duration::from_secs(90))
        .expect("vanilla starts");
    let (solaris, solaris_addr) = spawn_solaris().await.expect("spawn Solaris");
    let solaris_task = tokio::spawn(async move { solaris.serve().await });

    let scenario = SpawnSmokeScenario;
    let vanilla_observations = scenario
        .run(ScenarioContext {
            kind: ServerKind::Vanilla,
            addr: vanilla.addr(),
        })
        .await
        .expect("vanilla scenario runs");
    let solaris_observations = scenario
        .run(ScenarioContext {
            kind: ServerKind::Solaris,
            addr: solaris_addr,
        })
        .await
        .expect("Solaris scenario runs");

    let diff = diff_observations(&vanilla_observations, &solaris_observations);
    assert!(diff.is_empty(), "{diff}");

    vanilla.stop().expect("vanilla stops");
    solaris_task.abort();
}

#[tokio::test]
#[ignore = "requires local .analysis/server.jar vanilla oracle and Java"]
async fn vanilla_and_solaris_seeded_core_actions_can_be_diffed() {
    let availability = vanilla_oracle_availability(repo_root());
    let OracleAvailability::Available { jar } = availability else {
        eprintln!("{}", availability.skip_message().expect("skip message"));
        return;
    };

    let vanilla_dir = tempfile::tempdir().expect("vanilla tempdir");
    let vanilla = VanillaServerProcess::launch(&jar, vanilla_dir.path(), Duration::from_secs(90))
        .expect("vanilla starts");
    let (solaris, solaris_addr) = spawn_solaris().await.expect("spawn Solaris");
    let solaris_task = tokio::spawn(async move { solaris.serve().await });

    let scenario = CoreActionSequenceScenario::new(
        "seeded-core-actions",
        CoreActionGenerator::generate(0x54, 3),
    );
    let vanilla_observations = scenario
        .run(ScenarioContext {
            kind: ServerKind::Vanilla,
            addr: vanilla.addr(),
        })
        .await
        .expect("vanilla core action scenario runs");
    let solaris_observations = scenario
        .run(ScenarioContext {
            kind: ServerKind::Solaris,
            addr: solaris_addr,
        })
        .await
        .expect("Solaris core action scenario runs");

    assert!(vanilla_observations.facts().iter().any(|fact| matches!(
        fact,
        ObservationFact::InventoryContent {
            container_id: 0,
            slots: 46,
            ..
        }
    )));
    assert!(solaris_observations.facts().iter().any(|fact| matches!(
        fact,
        ObservationFact::InventoryContent {
            container_id: 0,
            slots: 46,
            ..
        }
    )));

    let diff = diff_observations(&vanilla_observations, &solaris_observations);
    assert!(diff.is_empty(), "{diff}");

    vanilla.stop().expect("vanilla stops");
    solaris_task.abort();
}
