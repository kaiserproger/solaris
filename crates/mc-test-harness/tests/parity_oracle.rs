use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use mc_protocol::packets::Packet;
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundKnownPacks, FinishConfiguration, RegistryData,
    ServerboundKnownPacks, UpdateTags,
};
use mc_protocol::packets::play::{
    AddEntity, ClientboundChangeDifficulty, ClientboundCommands, ClientboundContainerSetContent,
    ClientboundContainerSetSlot, ClientboundInitializeBorder, ClientboundKeepAlive,
    ClientboundPlayerAbilities, ClientboundSetHealth, ClientboundSetHeldSlot, ClientboundSetTime,
    ConfirmTeleportation, EntityEvent, GameEvent, LoginPlay, MovePlayerFlags, RemoveEntities,
    ServerboundChatCommand, ServerboundKeepAlive, ServerboundMovePlayerPos,
    ServerboundMovePlayerStatusOnly, ServerboundPlayerLoaded, ServerboundSetCarriedItem,
    SetCenterChunk, SetDefaultSpawnPosition, SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;
use mc_test_harness::parity::{
    CoreActionGenerator, CoreActionSequenceScenario, ObservationFact, ObservationSet,
    OracleAvailability, ParityScenario, ScenarioContext, ScenarioFuture, ServerKind,
    VanillaServerProcess, diff_observations, read_packet_id_skipping_startup_noise,
    read_typed_skipping_startup_noise, vanilla_oracle_availability,
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

struct ConfigurationPhaseScenario;

impl ParityScenario for ConfigurationPhaseScenario {
    fn name(&self) -> &'static str {
        "configuration-phase"
    }

    fn run<'a>(&'a self, ctx: ScenarioContext) -> ScenarioFuture<'a> {
        Box::pin(async move { observe_configuration_phase(ctx).await })
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
    let _: LoginPlay = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen { id: LoginPlay::ID });
    let _: ClientboundChangeDifficulty = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundChangeDifficulty::ID,
    });
    let _: ClientboundPlayerAbilities = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundPlayerAbilities::ID,
    });
    let held: ClientboundSetHeldSlot = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundSetHeldSlot::ID,
    });
    observations.push(ObservationFact::HeldSlotChanged { slot: held.slot });
    let _: EntityEvent = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: EntityEvent::ID,
    });
    read_packet_id_skipping_startup_noise(&mut client, ClientboundCommands::ID).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundCommands::ID,
    });
    let sync = read_spawn_position(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: SynchronizePlayerPosition::ID,
    });
    observations.push(ObservationFact::Note {
        key: "spawn_position_received".into(),
        value: "true".into(),
    });
    let _: ClientboundInitializeBorder = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundInitializeBorder::ID,
    });
    let _: ClientboundSetTime = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundSetTime::ID,
    });
    let _: SetDefaultSpawnPosition = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: SetDefaultSpawnPosition::ID,
    });
    let event: GameEvent = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen { id: GameEvent::ID });
    observations.push(ObservationFact::Note {
        key: "start_waiting_for_chunks".into(),
        value: (event.event == GameEvent::EVENT_START_WAITING_FOR_CHUNKS).to_string(),
    });
    let _: SetCenterChunk = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: SetCenterChunk::ID,
    });
    observations.push(ObservationFact::Note {
        key: "center_chunk_received".into(),
        value: "true".into(),
    });
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await?;
    Ok(observations.normalized())
}

async fn observe_configuration_phase(ctx: ScenarioContext) -> Result<ObservationSet> {
    let subject = match ctx.kind {
        ServerKind::Solaris => "solaris",
        ServerKind::Vanilla => "vanilla",
    };
    let mut client = Client::connect(ctx.addr).await?;
    let _login = client.drive_login(ctx.addr, subject).await?;

    let mut observations = ObservationSet::new(subject, "configuration-phase");
    let known = loop {
        let frame = client.read_frame().await?;
        if frame.id == ClientboundKnownPacks::ID {
            break ClientboundKnownPacks::decode(&mut frame.body.clone())?;
        }
    };
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundKnownPacks::ID,
    });
    observations.push(ObservationFact::Note {
        key: "known_packs.count".into(),
        value: known.packs.len().to_string(),
    });
    for (index, pack) in known.packs.iter().enumerate() {
        observations.push(ObservationFact::Note {
            key: format!("known_packs.{index}"),
            value: format!("{}:{}:{}", pack.namespace, pack.id, pack.version),
        });
    }
    client
        .write_packet(&ServerboundKnownPacks {
            packs: known.packs.clone(),
        })
        .await?;

    let mut registry_packets = 0usize;
    loop {
        let frame = client.read_frame().await?;
        if frame.id == RegistryData::ID {
            let registry = RegistryData::decode(&mut frame.body.clone())?;
            registry_packets += 1;
            observations.push(ObservationFact::PacketSeen {
                id: RegistryData::ID,
            });
            observations.push(ObservationFact::Note {
                key: format!("registry_data.{}.entries", registry.registry_id),
                value: registry.entries.len().to_string(),
            });
            continue;
        }
        if frame.id == UpdateTags::ID {
            let tags = UpdateTags::decode(&mut frame.body.clone())?;
            observations.push(ObservationFact::PacketSeen { id: UpdateTags::ID });
            observations.push(ObservationFact::Note {
                key: "update_tags.registries.count".into(),
                value: tags.registries.len().to_string(),
            });
            for registry in tags.registries {
                observations.push(ObservationFact::Note {
                    key: format!("update_tags.{}.tags", registry.registry),
                    value: registry.tags.len().to_string(),
                });
            }
            continue;
        }
        if frame.id == FinishConfiguration::ID {
            let _finish = FinishConfiguration::decode(&mut frame.body.clone())?;
            observations.push(ObservationFact::PacketSeen {
                id: FinishConfiguration::ID,
            });
            observations.push(ObservationFact::Note {
                key: "registry_data.packet_count".into(),
                value: registry_packets.to_string(),
            });
            client.write_packet(&AcknowledgeFinishConfiguration).await?;
            return Ok(observations.normalized());
        }
        anyhow::bail!(
            "unexpected configuration frame id=0x{:02X} body_len={}",
            frame.id,
            frame.body.len()
        );
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn local_vanilla_dir() -> PathBuf {
    repo_root().join("data/vanilla")
}

async fn read_spawn_position(client: &mut Client) -> Result<SynchronizePlayerPosition> {
    read_typed_skipping_startup_noise(client).await
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
            runtime_control: None,
        },
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await?;
    let addr = bound.local_addr()?;
    Ok((bound, addr))
}

async fn spawn_solaris_with_local_vanilla_data()
-> Result<(mc_net::BoundServer, std::net::SocketAddr)> {
    let vanilla_dir = local_vanilla_dir();
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    let registries_json = vanilla_dir.join("reports/registries.json");
    let data = Arc::new(mc_data::load(&vanilla_dir)?);
    let blocks_report = mc_data::blocks::load_blocks_report(&blocks_json)?;
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&blocks_report)?);
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(Arc::clone(&blocks), 128)
        .with_generator(generator);
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data)?);
    let items_report = mc_data::items::load_items_report(&registries_json)?;
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let entity_report = mc_data::entity_types::load_entity_types_report(&registries_json)?;
    let entity_types = Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(
        &entity_report,
    ));
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse()?,
        motd: "M79 configuration parity".into(),
        max_players: 8,
        view_distance: 2,
        data,
        blocks: Arc::clone(&blocks),
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::builtin().clone()),
        block_light: mc_data::block_light::load(vanilla_dir.join("reports/block_light.json"))
            .ok()
            .map(Arc::new),
        items,
        item_facts: Arc::new(mc_data::item_components::load_item_facts(
            vanilla_dir.join("reports/item_components"),
        )?),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &blocks_report,
        )),
        entity_types,
        biome_spawns: mc_data::biomes::load_biome_spawn_rules(
            vanilla_dir.join("data/minecraft/worldgen/biome"),
        )
        .map(Arc::new)
        .unwrap_or_default(),
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
            runtime_control: None,
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
async fn solaris_configuration_phase_scenario_produces_normalized_observations() {
    let (bound, addr) = spawn_solaris().await.expect("spawn Solaris");
    let task = tokio::spawn(async move { bound.serve().await });

    let scenario = ConfigurationPhaseScenario;
    let observations = scenario
        .run(ScenarioContext {
            kind: ServerKind::Solaris,
            addr,
        })
        .await
        .expect("configuration scenario runs");

    assert_eq!(scenario.name(), "configuration-phase");
    assert!(observations.facts().contains(&ObservationFact::PacketSeen {
        id: ClientboundKnownPacks::ID,
    }));
    assert!(
        observations
            .facts()
            .contains(&ObservationFact::PacketSeen { id: UpdateTags::ID })
    );
    assert!(observations.facts().iter().any(|fact| matches!(
        fact,
        ObservationFact::Note { key, value }
            if key == "registry_data.packet_count" && value != "0"
    )));

    task.abort();
}

#[tokio::test]
#[ignore = "requires local .analysis/server.jar vanilla oracle and Java"]
async fn vanilla_and_solaris_configuration_phase_can_be_diffed() {
    let availability = vanilla_oracle_availability(repo_root());
    let OracleAvailability::Available { jar } = availability else {
        eprintln!("{}", availability.skip_message().expect("skip message"));
        return;
    };

    let vanilla_dir = tempfile::tempdir().expect("vanilla tempdir");
    let vanilla = VanillaServerProcess::launch(&jar, vanilla_dir.path(), Duration::from_secs(90))
        .expect("vanilla starts");
    let (solaris, solaris_addr) = spawn_solaris_with_local_vanilla_data()
        .await
        .expect("spawn Solaris with local vanilla sidecar");
    let solaris_task = tokio::spawn(async move { solaris.serve().await });

    let scenario = ConfigurationPhaseScenario;
    let vanilla_observations = scenario
        .run(ScenarioContext {
            kind: ServerKind::Vanilla,
            addr: vanilla.addr(),
        })
        .await
        .expect("vanilla configuration scenario runs");
    let solaris_observations = scenario
        .run(ScenarioContext {
            kind: ServerKind::Solaris,
            addr: solaris_addr,
        })
        .await
        .expect("Solaris configuration scenario runs");

    let diff = diff_observations(&vanilla_observations, &solaris_observations);
    assert!(diff.is_empty(), "{diff}");
    println!("M79_ORACLE_COMPARISON_OK configuration-phase");

    vanilla.stop().expect("vanilla stops");
    solaris_task.abort();
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
    println!("M79_ORACLE_COMPARISON_OK spawn-smoke");

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

// ---------------------------------------------------------------------------
// M53.b scenario 1: container held-slot lifecycle
// ---------------------------------------------------------------------------

struct ContainerHeldSlotScenario;

impl ParityScenario for ContainerHeldSlotScenario {
    fn name(&self) -> &'static str {
        "container-held-slot"
    }

    fn run<'a>(&'a self, ctx: ScenarioContext) -> ScenarioFuture<'a> {
        Box::pin(async move { observe_container_held_slot(ctx).await })
    }
}

async fn observe_container_held_slot(ctx: ScenarioContext) -> Result<ObservationSet> {
    let subject = match ctx.kind {
        ServerKind::Solaris => "solaris",
        ServerKind::Vanilla => "vanilla",
    };
    let mut client = Client::connect(ctx.addr).await?;
    let _login = client.drive_login(ctx.addr, subject).await?;
    client.drive_configuration().await?;

    let mut observations = ObservationSet::new(subject, "container-held-slot");
    let _: LoginPlay = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen { id: LoginPlay::ID });
    let _: ClientboundChangeDifficulty = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundChangeDifficulty::ID,
    });
    let _: ClientboundPlayerAbilities = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundPlayerAbilities::ID,
    });
    let held: ClientboundSetHeldSlot = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundSetHeldSlot::ID,
    });
    observations.push(ObservationFact::HeldSlotChanged { slot: held.slot });
    let _: EntityEvent = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: EntityEvent::ID,
    });
    read_packet_id_skipping_startup_noise(&mut client, ClientboundCommands::ID).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundCommands::ID,
    });
    let sync = read_spawn_position(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: SynchronizePlayerPosition::ID,
    });
    observations.push(ObservationFact::Note {
        key: "spawn_position_received".into(),
        value: "true".into(),
    });
    let _: ClientboundInitializeBorder = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundInitializeBorder::ID,
    });
    let _: ClientboundSetTime = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundSetTime::ID,
    });
    let _: SetDefaultSpawnPosition = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: SetDefaultSpawnPosition::ID,
    });
    let _: GameEvent = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen { id: GameEvent::ID });
    let _: SetCenterChunk = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: SetCenterChunk::ID,
    });
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await?;
    client.write_packet(&ServerboundPlayerLoaded).await?;

    // Drain initial frames to capture held slot and inventory snapshot.
    let mut saw_held_slot = true;
    let mut saw_inventory = false;
    for index in 0..64 {
        let timeout = if index == 0 {
            Duration::from_secs(5)
        } else {
            Duration::from_millis(250)
        };
        let frame = match client.read_frame_with_timeout(timeout).await {
            Ok(frame) => frame,
            Err(_err) if saw_inventory => break,
            Err(_err) if saw_held_slot => {
                continue;
            }
            Err(err) => return Err(err).context("wait for post-login frames"),
        };

        match frame.id {
            id if id == ClientboundSetHeldSlot::ID => {
                let mut body = frame.body.clone();
                let held = ClientboundSetHeldSlot::decode(&mut body)?;
                observations.push(ObservationFact::HeldSlotChanged { slot: held.slot });
                saw_held_slot = true;
            }
            id if id == ClientboundContainerSetContent::ID => {
                let mut body = frame.body.clone();
                let inventory = ClientboundContainerSetContent::decode(&mut body)?;
                let slots = u16::try_from(inventory.items.len())
                    .context("inventory slot count exceeds observation range")?;
                let non_empty_slots = u16::try_from(
                    inventory
                        .items
                        .iter()
                        .filter(|item| !item.is_empty())
                        .count(),
                )
                .context("non-empty inventory slot count exceeds observation range")?;
                observations.push(ObservationFact::InventoryContent {
                    container_id: inventory.container_id,
                    state_id: inventory.state_id,
                    slots,
                    non_empty_slots,
                    carried_count: inventory.carried_item.count,
                });
                saw_inventory = true;
            }
            id if id == ClientboundSetHealth::ID => {
                let mut body = frame.body.clone();
                let health = ClientboundSetHealth::decode(&mut body)?;
                observations.push(ObservationFact::Health {
                    half_hearts_milli: (health.health * 1000.0).round() as i32,
                    food: health.food,
                });
            }
            id if id == ClientboundKeepAlive::ID => {
                let mut body = frame.body.clone();
                let keepalive = ClientboundKeepAlive::decode(&mut body)?;
                client
                    .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                    .await?;
            }
            _ => {}
        }
        if saw_inventory {
            break;
        }
    }

    observations.push(ObservationFact::Note {
        key: "initial_held_slot_observed".into(),
        value: saw_held_slot.to_string(),
    });
    observations.push(ObservationFact::Note {
        key: "initial_inventory_observed".into(),
        value: saw_inventory.to_string(),
    });

    // Send a held-slot change to slot 1.
    client
        .write_packet(&ServerboundSetCarriedItem { slot: 1 })
        .await?;
    observations.push(ObservationFact::Note {
        key: "set_carried_item.1".into(),
        value: "sent".into(),
    });

    // Observe the server's echoed held-slot change.
    let mut saw_echo = false;
    for _ in 0..32 {
        let frame = match client
            .read_frame_with_timeout(Duration::from_millis(250))
            .await
        {
            Ok(frame) => frame,
            Err(_) => break,
        };
        if frame.id == ClientboundSetHeldSlot::ID {
            let mut body = frame.body.clone();
            let held = ClientboundSetHeldSlot::decode(&mut body)?;
            observations.push(ObservationFact::HeldSlotChanged { slot: held.slot });
            saw_echo = true;
            break;
        }
        if frame.id == ClientboundKeepAlive::ID {
            let mut body = frame.body.clone();
            let keepalive = ClientboundKeepAlive::decode(&mut body)?;
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await?;
        }
    }

    observations.push(ObservationFact::Note {
        key: "held_slot_echo_observed".into(),
        value: saw_echo.to_string(),
    });

    // Send one more held-slot change back to slot 0.
    client
        .write_packet(&ServerboundSetCarriedItem { slot: 0 })
        .await?;
    observations.push(ObservationFact::Note {
        key: "set_carried_item.0".into(),
        value: "sent".into(),
    });

    // Observe the second echo.
    let mut saw_second_echo = false;
    for _ in 0..32 {
        let frame = match client
            .read_frame_with_timeout(Duration::from_millis(250))
            .await
        {
            Ok(frame) => frame,
            Err(_) => break,
        };
        if frame.id == ClientboundSetHeldSlot::ID {
            let mut body = frame.body.clone();
            let held = ClientboundSetHeldSlot::decode(&mut body)?;
            observations.push(ObservationFact::HeldSlotChanged { slot: held.slot });
            saw_second_echo = true;
            break;
        }
        if frame.id == ClientboundKeepAlive::ID {
            let mut body = frame.body.clone();
            let keepalive = ClientboundKeepAlive::decode(&mut body)?;
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await?;
        }
    }

    observations.push(ObservationFact::Note {
        key: "second_held_slot_echo_observed".into(),
        value: saw_second_echo.to_string(),
    });

    Ok(observations.normalized())
}

#[tokio::test]
async fn solaris_container_held_slot_produces_observations() {
    let (bound, addr) = spawn_solaris().await.expect("spawn Solaris");
    let task = tokio::spawn(async move { bound.serve().await });

    let scenario = ContainerHeldSlotScenario;
    let observations = scenario
        .run(ScenarioContext {
            kind: ServerKind::Solaris,
            addr,
        })
        .await
        .expect("scenario runs");

    assert_eq!(scenario.name(), "container-held-slot");
    assert!(observations.facts().contains(&ObservationFact::PacketSeen {
        id: SynchronizePlayerPosition::ID,
    }));
    assert!(observations.facts().iter().any(|fact| matches!(
        fact,
        ObservationFact::Note { key, value } if key == "initial_held_slot_observed"
    )));
    assert!(observations.facts().iter().any(|fact| matches!(
        fact,
        ObservationFact::Note { key, value } if key == "initial_inventory_observed"
    )));
    assert!(
        observations
            .facts()
            .iter()
            .any(|fact| matches!(fact, ObservationFact::HeldSlotChanged { .. }))
    );

    task.abort();
}

#[tokio::test]
#[ignore = "requires local .analysis/server.jar vanilla oracle and Java"]
async fn vanilla_and_solaris_container_held_slot_can_be_diffed() {
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

    let scenario = ContainerHeldSlotScenario;
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

// ---------------------------------------------------------------------------
// M53.b scenario 2: entity lifecycle (spawn observation)
// ---------------------------------------------------------------------------

struct EntityLifecycleScenario;

impl ParityScenario for EntityLifecycleScenario {
    fn name(&self) -> &'static str {
        "entity-lifecycle"
    }

    fn run<'a>(&'a self, ctx: ScenarioContext) -> ScenarioFuture<'a> {
        Box::pin(async move { observe_entity_lifecycle(ctx).await })
    }
}

async fn observe_entity_lifecycle(ctx: ScenarioContext) -> Result<ObservationSet> {
    let subject = match ctx.kind {
        ServerKind::Solaris => "solaris",
        ServerKind::Vanilla => "vanilla",
    };
    let mut client = Client::connect(ctx.addr).await?;
    let _login = client.drive_login(ctx.addr, subject).await?;
    client.drive_configuration().await?;

    let mut observations = ObservationSet::new(subject, "entity-lifecycle");
    let _: LoginPlay = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen { id: LoginPlay::ID });
    let _: ClientboundChangeDifficulty = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundChangeDifficulty::ID,
    });
    let _: ClientboundPlayerAbilities = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundPlayerAbilities::ID,
    });
    let _: ClientboundSetHeldSlot = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundSetHeldSlot::ID,
    });
    let _: EntityEvent = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: EntityEvent::ID,
    });
    read_packet_id_skipping_startup_noise(&mut client, ClientboundCommands::ID).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundCommands::ID,
    });
    let sync = read_spawn_position(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: SynchronizePlayerPosition::ID,
    });
    observations.push(ObservationFact::Note {
        key: "spawn_position_received".into(),
        value: "true".into(),
    });
    let _: ClientboundInitializeBorder = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundInitializeBorder::ID,
    });
    let _: ClientboundSetTime = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundSetTime::ID,
    });
    let _: SetDefaultSpawnPosition = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: SetDefaultSpawnPosition::ID,
    });
    let _: GameEvent = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen { id: GameEvent::ID });
    let _: SetCenterChunk = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: SetCenterChunk::ID,
    });
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await?;
    client.write_packet(&ServerboundPlayerLoaded).await?;

    // Ignore natural spawn noise from vanilla's generated world, then exercise a
    // deterministic command-spawned entity on both servers.
    drain_entity_lifecycle_frames(
        &mut client,
        &mut observations,
        sync.x,
        sync.y,
        sync.z,
        false,
        None,
    )
    .await?;
    let summon_x = sync.x.floor() as i32 + 2;
    let summon_y = sync.y.floor() as i32;
    let summon_z = sync.z.floor() as i32;
    client
        .write_packet(&ServerboundChatCommand {
            command: format!("summon minecraft:zombie {summon_x} {summon_y} {summon_z}"),
        })
        .await?;
    let zombie_type_id = mc_data::entity_types::solaris_required_entity_types()
        .id_of(&mc_data::Identifier::parse("minecraft:zombie")?)
        .and_then(|id| i32::try_from(id).ok())
        .context("zombie entity type id")?;
    let entity_count = drain_entity_lifecycle_frames(
        &mut client,
        &mut observations,
        sync.x,
        sync.y,
        sync.z,
        true,
        Some((
            zombie_type_id,
            f64::from(summon_x),
            f64::from(summon_y),
            f64::from(summon_z),
        )),
    )
    .await?;
    if entity_count == 0 {
        anyhow::bail!("expected command-spawned zombie AddEntity was not observed");
    }

    observations.push(ObservationFact::Note {
        key: "explicit_entity_spawn_observed".into(),
        value: "true".into(),
    });
    observations.push(ObservationFact::Note {
        key: "post_action_liveness".into(),
        value: "clientbound_frame".into(),
    });

    Ok(observations.normalized())
}

async fn drain_entity_lifecycle_frames(
    client: &mut Client,
    observations: &mut ObservationSet,
    x: f64,
    y: f64,
    z: f64,
    record_entities: bool,
    expected_entity: Option<(i32, f64, f64, f64)>,
) -> Result<u32> {
    let flags = MovePlayerFlags::new(false, false);
    let mut entity_count = 0u32;
    let cycles = if record_entities { 40 } else { 8 };
    for _cycle in 0..cycles {
        client
            .write_packet(&ServerboundMovePlayerStatusOnly { flags })
            .await?;

        let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
        while tokio::time::Instant::now() < deadline {
            let remaining = deadline - tokio::time::Instant::now();
            let frame = match client.read_frame_with_timeout(remaining).await {
                Ok(frame) => frame,
                Err(_) => break,
            };
            match frame.id {
                id if id == ClientboundKeepAlive::ID => {
                    let mut body = frame.body.clone();
                    let keepalive = ClientboundKeepAlive::decode(&mut body)?;
                    client
                        .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                        .await?;
                }
                id if id == AddEntity::ID => {
                    let mut body = frame.body.clone();
                    let add = AddEntity::decode(&mut body)?;
                    if record_entities
                        && expected_entity.is_none_or(|(entity_type_id, ex, ey, ez)| {
                            add.entity_type_id == entity_type_id
                                && (add.x - ex).abs() <= 1.5
                                && (add.y - ey).abs() <= 2.0
                                && (add.z - ez).abs() <= 1.5
                        })
                    {
                        observations.push(ObservationFact::Note {
                            key: "entity_spawn_packet_seen".into(),
                            value: "true".into(),
                        });
                        entity_count += 1;
                        return Ok(entity_count);
                    }
                }
                id if id == RemoveEntities::ID => {
                    let mut body = frame.body.clone();
                    let _removed = RemoveEntities::decode(&mut body)?;
                }
                id if id == EntityEvent::ID => {
                    let mut body = frame.body.clone();
                    let _event = EntityEvent::decode(&mut body)?;
                }
                id if id == ClientboundSetHealth::ID => {
                    let mut body = frame.body.clone();
                    let _health = ClientboundSetHealth::decode(&mut body)?;
                }
                id if id == ClientboundContainerSetContent::ID => {
                    let mut body = frame.body.clone();
                    let inventory = ClientboundContainerSetContent::decode(&mut body)?;
                    let slots = u16::try_from(inventory.items.len())
                        .context("inventory slot count exceeds observation range")?;
                    let non_empty_slots = u16::try_from(
                        inventory
                            .items
                            .iter()
                            .filter(|item| !item.is_empty())
                            .count(),
                    )
                    .context("non-empty inventory slot count exceeds observation range")?;
                    observations.push(ObservationFact::InventoryContent {
                        container_id: inventory.container_id,
                        state_id: inventory.state_id,
                        slots,
                        non_empty_slots,
                        carried_count: inventory.carried_item.count,
                    });
                }
                id if id == ClientboundContainerSetSlot::ID => {
                    let mut body = frame.body.clone();
                    let slot = ClientboundContainerSetSlot::decode(&mut body)?;
                    observations.push(ObservationFact::ContainerSlotContent {
                        container_id: slot.container_id,
                        state_id: slot.state_id,
                        slot: slot.slot,
                        item_id: slot.item_stack.item_id,
                        count: slot.item_stack.count,
                    });
                }
                _ => {}
            }
        }

        client
            .write_packet(&ServerboundMovePlayerPos { x, y, z, flags })
            .await?;
    }
    Ok(entity_count)
}

#[tokio::test]
async fn solaris_entity_lifecycle_produces_observations() {
    let (bound, addr) = spawn_solaris().await.expect("spawn Solaris");
    let task = tokio::spawn(async move { bound.serve().await });

    let scenario = EntityLifecycleScenario;
    let observations = scenario
        .run(ScenarioContext {
            kind: ServerKind::Solaris,
            addr,
        })
        .await
        .expect("scenario runs");

    assert_eq!(scenario.name(), "entity-lifecycle");
    assert!(observations.facts().contains(&ObservationFact::PacketSeen {
        id: SynchronizePlayerPosition::ID,
    }));
    assert!(observations.facts().iter().any(|fact| matches!(
        fact,
        ObservationFact::Note { key, value } if key == "explicit_entity_spawn_observed" && value == "true"
    )));
    assert!(observations.facts().iter().any(|fact| matches!(
        fact,
        ObservationFact::Note { key, value } if key == "post_action_liveness" && value == "clientbound_frame"
    )));

    task.abort();
}

#[tokio::test]
#[ignore = "requires local .analysis/server.jar vanilla oracle and Java"]
async fn vanilla_and_solaris_entity_lifecycle_can_be_diffed() {
    let availability = vanilla_oracle_availability(repo_root());
    let OracleAvailability::Available { jar } = availability else {
        eprintln!("{}", availability.skip_message().expect("skip message"));
        return;
    };

    let vanilla_dir = tempfile::tempdir().expect("vanilla tempdir");
    let mut vanilla =
        VanillaServerProcess::launch(&jar, vanilla_dir.path(), Duration::from_secs(90))
            .expect("vanilla starts");
    vanilla
        .send_command("op vanilla")
        .expect("op vanilla oracle client");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (solaris, solaris_addr) = spawn_solaris().await.expect("spawn Solaris");
    let solaris_task = tokio::spawn(async move { solaris.serve().await });

    let scenario = EntityLifecycleScenario;
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

// ---------------------------------------------------------------------------
// M53.b scenario 3: timed action liveness
// ---------------------------------------------------------------------------

struct TimedActionScenario;

impl ParityScenario for TimedActionScenario {
    fn name(&self) -> &'static str {
        "timed-action"
    }

    fn run<'a>(&'a self, ctx: ScenarioContext) -> ScenarioFuture<'a> {
        Box::pin(async move { observe_timed_action(ctx).await })
    }
}

async fn observe_timed_action(ctx: ScenarioContext) -> Result<ObservationSet> {
    let subject = match ctx.kind {
        ServerKind::Solaris => "solaris",
        ServerKind::Vanilla => "vanilla",
    };
    let mut client = Client::connect(ctx.addr).await?;
    let _login = client.drive_login(ctx.addr, subject).await?;
    client.drive_configuration().await?;

    let mut observations = ObservationSet::new(subject, "timed-action");
    let _: LoginPlay = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen { id: LoginPlay::ID });
    let _: ClientboundChangeDifficulty = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundChangeDifficulty::ID,
    });
    let _: ClientboundPlayerAbilities = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundPlayerAbilities::ID,
    });
    let _: ClientboundSetHeldSlot = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundSetHeldSlot::ID,
    });
    let _: EntityEvent = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: EntityEvent::ID,
    });
    read_packet_id_skipping_startup_noise(&mut client, ClientboundCommands::ID).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundCommands::ID,
    });
    let sync = read_spawn_position(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: SynchronizePlayerPosition::ID,
    });
    observations.push(ObservationFact::Note {
        key: "spawn_position_received".into(),
        value: "true".into(),
    });
    let _: ClientboundInitializeBorder = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundInitializeBorder::ID,
    });
    let _: ClientboundSetTime = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundSetTime::ID,
    });
    let _: SetDefaultSpawnPosition = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: SetDefaultSpawnPosition::ID,
    });
    let _: GameEvent = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen { id: GameEvent::ID });
    let _: SetCenterChunk = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: SetCenterChunk::ID,
    });
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await?;
    client.write_packet(&ServerboundPlayerLoaded).await?;

    let flags = MovePlayerFlags::new(false, false);
    let mut keepalive_count = 0u32;
    let cycles = 6u32;

    for _ in 0..cycles {
        client
            .write_packet(&ServerboundMovePlayerStatusOnly { flags })
            .await?;

        let end = tokio::time::Instant::now() + Duration::from_millis(500);
        while tokio::time::Instant::now() < end {
            let remaining = end - tokio::time::Instant::now();
            if remaining.is_zero() {
                break;
            }
            let frame = match client.read_frame_with_timeout(remaining).await {
                Ok(frame) => frame,
                Err(_) => break,
            };
            if frame.id == ClientboundKeepAlive::ID {
                let mut body = frame.body.clone();
                let keepalive = ClientboundKeepAlive::decode(&mut body)?;
                client
                    .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                    .await?;
                keepalive_count += 1;
            }
        }

        client
            .write_packet(&ServerboundMovePlayerPos {
                x: sync.x,
                y: sync.y,
                z: sync.z,
                flags,
            })
            .await?;
    }

    observations.push(ObservationFact::Note {
        key: "keepalive_round_trips".into(),
        value: keepalive_count.to_string(),
    });
    observations.push(ObservationFact::Note {
        key: "cycles_executed".into(),
        value: cycles.to_string(),
    });
    observations.push(ObservationFact::Note {
        key: "post_action_liveness".into(),
        value: "clientbound_frame".into(),
    });

    Ok(observations.normalized())
}

#[tokio::test]
async fn solaris_timed_action_produces_liveness_observations() {
    let (bound, addr) = spawn_solaris().await.expect("spawn Solaris");
    let task = tokio::spawn(async move { bound.serve().await });

    let scenario = TimedActionScenario;
    let observations = scenario
        .run(ScenarioContext {
            kind: ServerKind::Solaris,
            addr,
        })
        .await
        .expect("scenario runs");

    assert_eq!(scenario.name(), "timed-action");
    assert!(observations.facts().contains(&ObservationFact::PacketSeen {
        id: SynchronizePlayerPosition::ID,
    }));
    assert!(observations.facts().iter().any(|fact| matches!(
        fact,
        ObservationFact::Note { key, value } if key == "keepalive_round_trips"
    )));
    assert!(observations.facts().iter().any(|fact| matches!(
        fact,
        ObservationFact::Note { key, value } if key == "cycles_executed"
    )));

    task.abort();
}

#[tokio::test]
#[ignore = "requires local .analysis/server.jar vanilla oracle and Java"]
async fn vanilla_and_solaris_timed_action_can_be_diffed() {
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

    let scenario = TimedActionScenario;
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
