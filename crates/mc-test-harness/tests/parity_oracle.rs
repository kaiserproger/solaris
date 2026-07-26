use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use bytes::Bytes;
use mc_nbt::{ListTag, Tag};
use mc_protocol::packets::Packet;
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundKnownPacks, FinishConfiguration, RegistryData,
    ServerboundKnownPacks, UpdateTags,
};
use mc_protocol::packets::play::{
    AddEntity, BlockChangedAck, BlockUpdate, ClientboundChangeDifficulty, ClientboundCommands,
    ClientboundContainerClose, ClientboundContainerSetContent, ClientboundContainerSetSlot,
    ClientboundInitializeBorder, ClientboundKeepAlive, ClientboundOpenScreen,
    ClientboundPlayerAbilities, ClientboundSetHealth, ClientboundSetHeldSlot, ClientboundSetTime,
    ClientboundSystemChat, ConfirmTeleportation, ContainerInput, Direction, EntityEvent, GameEvent,
    HashedStack, InteractionHand, ItemStack, LevelChunkWithLight, LoginPlay, MovePlayerFlags,
    PlayerActionKind, RemoveEntities, SectionBlocksUpdate, ServerboundChatCommand,
    ServerboundContainerClick, ServerboundContainerClose, ServerboundKeepAlive,
    ServerboundMovePlayerPos, ServerboundMovePlayerStatusOnly, ServerboundPlayerAction,
    ServerboundPlayerLoaded, ServerboundSetCarriedItem, ServerboundUseItemOn, SetCenterChunk,
    SetDefaultSpawnPosition, SynchronizePlayerPosition, pack_block_pos, unpack_block_pos,
};
use mc_test_harness::client::Client;
use mc_test_harness::parity::{
    CoreActionGenerator, CoreActionSequenceScenario, ObservationFact, ObservationSet,
    OracleAvailability, ParityScenario, ScenarioContext, ScenarioFuture, ServerKind,
    VanillaServerProcess, diff_observations, read_packet_id_skipping_startup_noise,
    read_typed_skipping_startup_noise, vanilla_oracle_availability,
};
use mc_test_harness::replay::{
    BLOCK_TRANSACTION_ORACLE_SCHEMA, BlockTransactionOracleCase, BlockTransactionOracleEvent,
    BlockTransactionOracleManifest, BlockTransactionOraclePhase, BlockTransactionOraclePhaseTrace,
    BlockTransactionOracleTrace, CONTAINER_STATE_ORACLE_SCHEMA, ContainerStateOracleManifest,
    ContainerStateOracleMenu, ContainerStateOraclePhaseTrace, ContainerStateOracleSlot,
    ContainerStateOracleSnapshot, ContainerStateOracleStack, ContainerStateOracleTrace,
    REPLAY_RESULT_SCHEMA, ReplayCheckStatus, ReplayDriver, ReplayEvidenceKind, ReplayGateResult,
    ReplayHardwareProvenance, ReplayInvariantResult, ReplayOutcome, ReplayProvenance,
    ReplayRunResult, ReplayScenarioManifest, run_protocol_replay,
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
    Ok(observations.normalize_sequence())
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
            return Ok(observations.normalize_sequence());
        }
        anyhow::bail!(
            "unexpected configuration frame id=0x{:02X} body_len={}",
            frame.id,
            frame.body.len()
        );
    }
}

type RegistrySnapshot = BTreeMap<String, BTreeMap<String, Tag>>;

async fn collect_full_registry_snapshot(
    addr: std::net::SocketAddr,
    subject: &str,
) -> Result<RegistrySnapshot> {
    let mut client = Client::connect(addr).await?;
    let _login = client.drive_login(addr, subject).await?;

    loop {
        let frame = client.read_frame().await?;
        if frame.id == ClientboundKnownPacks::ID {
            let _ = ClientboundKnownPacks::decode(&mut frame.body.clone())?;
            break;
        }
    }
    client
        .write_packet(&ServerboundKnownPacks { packs: Vec::new() })
        .await?;

    let mut snapshot = RegistrySnapshot::new();
    loop {
        let frame = client.read_frame().await?;
        if frame.id == RegistryData::ID {
            let registry = RegistryData::decode(&mut frame.body.clone())?;
            let mut entries = BTreeMap::new();
            for entry in registry.entries {
                let payload = entry.nbt_payload.ok_or_else(|| {
                    anyhow::anyhow!(
                        "{subject} omitted fallback payload for {} in {}",
                        entry.name,
                        registry.registry_id
                    )
                })?;
                let mut payload = payload.as_ref();
                let tag = mc_nbt::read_network(&mut payload)?;
                anyhow::ensure!(payload.is_empty(), "registry payload has trailing bytes");
                let previous = entries.insert(entry.name.to_string(), canonical_registry_tag(tag));
                anyhow::ensure!(
                    previous.is_none(),
                    "duplicate registry entry {}",
                    entry.name
                );
            }
            let previous = snapshot.insert(registry.registry_id.to_string(), entries);
            anyhow::ensure!(
                previous.is_none(),
                "duplicate registry packet {}",
                registry.registry_id
            );
            continue;
        }
        if frame.id == UpdateTags::ID {
            let _ = UpdateTags::decode(&mut frame.body.clone())?;
            continue;
        }
        if frame.id == FinishConfiguration::ID {
            let _ = FinishConfiguration::decode(&mut frame.body.clone())?;
            client.write_packet(&AcknowledgeFinishConfiguration).await?;
            return Ok(snapshot);
        }
        anyhow::bail!(
            "unexpected configuration frame id=0x{:02X} body_len={}",
            frame.id,
            frame.body.len()
        );
    }
}

fn canonical_registry_tag(tag: Tag) -> Tag {
    match tag {
        Tag::List(list) => Tag::List(ListTag {
            element_type: list.element_type,
            elements: list
                .elements
                .into_iter()
                .map(canonical_registry_tag)
                .collect(),
        }),
        Tag::Compound(entries) => {
            let mut entries = entries
                .into_iter()
                .map(|(name, tag)| (name, canonical_registry_tag(tag)))
                .collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Tag::Compound(entries)
        }
        tag => tag,
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
            chunk_result_queue_size: 64,
            region_cache_size: 4,
            compression_threshold: 256,
            compression_level: None,
            runtime_control: None,
        },
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await?;
    let addr = bound.local_addr()?;
    Ok((bound, addr))
}

async fn spawn_solaris_with_local_vanilla_data_internal(
    seed_container_fixture: bool,
) -> Result<(mc_net::BoundServer, std::net::SocketAddr)> {
    let vanilla_dir = local_vanilla_dir();
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    let registries_json = vanilla_dir.join("reports/registries.json");
    let data = Arc::new(mc_data::load(&vanilla_dir)?);
    let blocks_report = mc_data::blocks::load_blocks_report(&blocks_json)?;
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&blocks_report)?);
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(Arc::clone(&blocks), 128)
        .with_generator(generator);
    if seed_container_fixture {
        let chest_pos = mc_world::BlockPos { x: 2, y: 200, z: 0 };
        let table_pos = mc_world::BlockPos { x: 4, y: 200, z: 0 };
        let platform_pos = mc_world::BlockPos { x: 3, y: 200, z: 2 };
        let clear_positions = [
            mc_world::BlockPos { x: 2, y: 201, z: 0 },
            mc_world::BlockPos { x: 3, y: 201, z: 2 },
            mc_world::BlockPos { x: 3, y: 202, z: 2 },
        ];
        let _ = storage.get_block(chest_pos)?;
        let chest_state = blocks
            .block(&mc_data::Identifier::parse("minecraft:chest")?)
            .context("container oracle chest block exists")?
            .default;
        let table_state = blocks
            .block(&mc_data::Identifier::parse("minecraft:crafting_table")?)
            .context("container oracle crafting table block exists")?
            .default;
        let stone_state = blocks
            .block(&mc_data::Identifier::parse("minecraft:stone")?)
            .context("container oracle stone block exists")?
            .default;
        let air_state = blocks
            .block(&mc_data::Identifier::parse("minecraft:air")?)
            .context("container oracle air block exists")?
            .default;
        storage
            .set_block_at(chest_pos, chest_state)?
            .context("container oracle chest chunk exists")?;
        storage.set_chest_block_entity(chest_pos, mc_world::ChestBlockEntity::default())?;
        storage
            .set_block_at(table_pos, table_state)?
            .context("container oracle crafting table chunk exists")?;
        storage
            .set_block_at(platform_pos, stone_state)?
            .context("container oracle platform chunk exists")?;
        for position in clear_positions {
            storage
                .set_block_at(position, air_state)?
                .context("container oracle clear-space chunk exists")?;
        }
    }
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data)?);
    let items_report = mc_data::items::load_items_report(&registries_json)?;
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let entity_report = mc_data::entity_types::load_entity_types_report(&registries_json)?;
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .context("entity type report is the exact 26.1.2 registry")?,
    );
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
            chunk_result_queue_size: 64,
            region_cache_size: 4,
            compression_threshold: 256,
            compression_level: None,
            runtime_control: None,
        },
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await?;
    let addr = bound.local_addr()?;
    Ok((bound, addr))
}

async fn spawn_solaris_with_local_vanilla_data()
-> Result<(mc_net::BoundServer, std::net::SocketAddr)> {
    spawn_solaris_with_local_vanilla_data_internal(false).await
}

async fn spawn_solaris_container_oracle() -> Result<(mc_net::BoundServer, std::net::SocketAddr)> {
    spawn_solaris_with_local_vanilla_data_internal(true).await
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
#[ignore = "requires local .analysis/server.jar vanilla oracle and Java"]
async fn vanilla_and_solaris_full_registry_fallback_match() {
    let availability = vanilla_oracle_availability(repo_root());
    let OracleAvailability::Available { jar } = availability else {
        eprintln!("{}", availability.skip_message().expect("skip message"));
        return;
    };

    let vanilla_dir = tempfile::tempdir().expect("vanilla tempdir");
    let vanilla = VanillaServerProcess::launch(&jar, vanilla_dir.path(), Duration::from_secs(90))
        .expect("vanilla starts");
    let vanilla_snapshot = collect_full_registry_snapshot(vanilla.addr(), "VanillaFallback")
        .await
        .expect("vanilla full registry fallback");

    let (solaris, solaris_addr) = spawn_solaris_with_local_vanilla_data()
        .await
        .expect("spawn Solaris with local vanilla sidecar");
    let solaris_task = tokio::spawn(async move { solaris.serve().await });
    let solaris_snapshot = collect_full_registry_snapshot(solaris_addr, "SolarisFallback")
        .await
        .expect("Solaris full registry fallback");

    assert_eq!(
        vanilla_snapshot.keys().collect::<Vec<_>>(),
        solaris_snapshot.keys().collect::<Vec<_>>(),
        "fallback registry set differs"
    );
    for (registry, vanilla_entries) in &vanilla_snapshot {
        let solaris_entries = &solaris_snapshot[registry];
        assert_eq!(
            vanilla_entries.keys().collect::<Vec<_>>(),
            solaris_entries.keys().collect::<Vec<_>>(),
            "fallback entry set differs for {registry}"
        );
        for (entry, vanilla_tag) in vanilla_entries {
            assert_eq!(
                vanilla_tag, &solaris_entries[entry],
                "fallback payload differs for {registry}/{entry}"
            );
        }
    }

    println!(
        "FULL_REGISTRY_FALLBACK_ORACLE_OK registries={} entries={}",
        vanilla_snapshot.len(),
        vanilla_snapshot.values().map(BTreeMap::len).sum::<usize>()
    );
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
async fn checked_manifest_replays_deterministically_on_two_fresh_solaris_servers() {
    let manifest = ReplayScenarioManifest::from_json(include_str!(
        "../../../tools/core-replay-scenarios/core-actions-seed-81.json"
    ))
    .expect("checked core replay manifest");

    let (first_server, first_addr) = spawn_solaris().await.expect("spawn first Solaris");
    let first_task = tokio::spawn(async move { first_server.serve().await });
    let first = run_protocol_replay(
        &manifest,
        ScenarioContext {
            kind: ServerKind::Solaris,
            addr: first_addr,
        },
    )
    .await
    .expect("first manifest replay");
    first_task.abort();

    let (second_server, second_addr) = spawn_solaris().await.expect("spawn second Solaris");
    let second_task = tokio::spawn(async move { second_server.serve().await });
    let second = run_protocol_replay(
        &manifest,
        ScenarioContext {
            kind: ServerKind::Solaris,
            addr: second_addr,
        },
    )
    .await
    .expect("second manifest replay");
    second_task.abort();

    assert_eq!(
        first, second,
        "fresh Solaris replay state must be deterministic"
    );
    assert!(first.facts().iter().any(|fact| matches!(
        fact,
        ObservationFact::Note { key, value }
            if key == "actions_executed" && value == "4"
    )));
    assert!(first.facts().iter().any(|fact| matches!(
        fact,
        ObservationFact::Note { key, value }
            if key == "post_action_liveness" && value == "clientbound_frame"
    )));
    assert!(first.facts().iter().any(|fact| matches!(
        fact,
        ObservationFact::InventoryContent {
            container_id: 0,
            slots: 46,
            ..
        }
    )));
    for (index, action) in manifest.actions.iter().enumerate() {
        let key = format!("action.{index}");
        let value = action.summary();
        assert!(first.facts().iter().any(|fact| matches!(
            fact,
            ObservationFact::Note { key: actual_key, value: actual_value }
                if actual_key.as_str() == key.as_str()
                    && actual_value.as_str() == value.as_str()
        )));
    }

    let result = ReplayRunResult {
        schema: REPLAY_RESULT_SCHEMA.into(),
        scenario_id: manifest.id.clone(),
        seed: manifest.seed,
        driver: ReplayDriver::SolarisProtocol,
        outcome: ReplayOutcome::Passed,
        actions: manifest.actions.clone(),
        concurrent_groups: Vec::new(),
        state_observations: Vec::new(),
        provenance: ReplayProvenance {
            git_commit: "0000000000000000000000000000000000000000".into(),
            config_sha256: "0000000000000000000000000000000000000000000000000000000000000000"
                .into(),
            build_profile: "cargo-test-debug".into(),
            sidecar_version: "embedded:26.1.2".into(),
            hardware: ReplayHardwareProvenance {
                os: std::env::consts::OS.into(),
                arch: std::env::consts::ARCH.into(),
                cpu_model: "integration-test-provenance-not-profile-evidence".into(),
                logical_cpus: std::thread::available_parallelism()
                    .map(usize::from)
                    .unwrap_or(1),
                memory_mib: 1,
            },
        },
        gates: vec![ReplayGateResult {
            id: "protocol-session".into(),
            evidence_kind: ReplayEvidenceKind::Harness,
            status: ReplayCheckStatus::Passed,
            reason: None,
            artifacts: vec!["crates/mc-test-harness/tests/parity_oracle.rs".into()],
        }],
        invariants: vec![
            ReplayInvariantResult {
                id: "post-action-liveness".into(),
                status: ReplayCheckStatus::Passed,
                reason: None,
            },
            ReplayInvariantResult {
                id: "deterministic-normalized-state".into(),
                status: ReplayCheckStatus::Passed,
                reason: None,
            },
        ],
        observations: vec![first],
    };
    result
        .validate_against(&manifest)
        .expect("protocol replay result matches manifest");
    let encoded = result.to_pretty_json().expect("encode replay result");
    let decoded = ReplayRunResult::from_json(&encoded).expect("decode replay result");
    decoded
        .validate_against(&manifest)
        .expect("decoded protocol replay result matches manifest");

    let mut without_solaris_lane = manifest.clone();
    without_solaris_lane
        .lanes
        .retain(|lane| lane.driver != ReplayDriver::SolarisProtocol);
    let err = run_protocol_replay(
        &without_solaris_lane,
        ScenarioContext {
            kind: ServerKind::Solaris,
            addr: "127.0.0.1:9".parse().expect("loopback discard address"),
        },
    )
    .await
    .expect_err("missing Solaris lane must fail before connecting");
    assert!(err.to_string().contains("solaris protocol lane"));

    let mut unsupported_reconnect = manifest.clone();
    unsupported_reconnect
        .actions
        .push(mc_test_harness::parity::CoreAction::Reconnect);
    let err = run_protocol_replay(
        &unsupported_reconnect,
        ScenarioContext {
            kind: ServerKind::Solaris,
            addr: "127.0.0.1:9".parse().expect("loopback discard address"),
        },
    )
    .await
    .expect_err("unsupported reconnect must fail before connecting");
    assert!(err.to_string().contains("does not support reconnect"));
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

#[tokio::test]
#[ignore = "requires local .analysis/server.jar vanilla oracle and Java"]
async fn checked_manifest_vanilla_and_solaris_protocol_observations_can_be_diffed() {
    let availability = vanilla_oracle_availability(repo_root());
    let OracleAvailability::Available { jar } = availability else {
        eprintln!("{}", availability.skip_message().expect("skip message"));
        return;
    };
    let manifest = ReplayScenarioManifest::from_json(include_str!(
        "../../../tools/core-replay-scenarios/core-actions-seed-81.json"
    ))
    .expect("checked core replay manifest");

    let vanilla_dir = tempfile::tempdir().expect("vanilla tempdir");
    let vanilla = VanillaServerProcess::launch(&jar, vanilla_dir.path(), Duration::from_secs(90))
        .expect("vanilla starts");
    let (solaris, solaris_addr) = spawn_solaris().await.expect("spawn Solaris");
    let solaris_task = tokio::spawn(async move { solaris.serve().await });

    let vanilla_observations = run_protocol_replay(
        &manifest,
        ScenarioContext {
            kind: ServerKind::Vanilla,
            addr: vanilla.addr(),
        },
    )
    .await
    .expect("vanilla checked-manifest replay");
    let solaris_observations = run_protocol_replay(
        &manifest,
        ScenarioContext {
            kind: ServerKind::Solaris,
            addr: solaris_addr,
        },
    )
    .await
    .expect("Solaris checked-manifest replay");

    for observations in [&vanilla_observations, &solaris_observations] {
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
                if key == "actions_executed" && value == "4"
        )));
        assert!(observations.facts().iter().any(|fact| matches!(
            fact,
            ObservationFact::Note { key, value }
                if key == "post_action_liveness" && value == "clientbound_frame"
        )));
        for (index, action) in manifest.actions.iter().enumerate() {
            let key = format!("action.{index}");
            let value = action.summary();
            assert!(observations.facts().iter().any(|fact| matches!(
                fact,
                ObservationFact::Note { key: actual_key, value: actual_value }
                    if actual_key.as_str() == key.as_str()
                        && actual_value.as_str() == value.as_str()
            )));
        }
    }

    let diff = diff_observations(&vanilla_observations, &solaris_observations);
    assert!(diff.is_empty(), "{diff}");
    println!("M79_CORE_REPLAY_COMPARISON_OK core-actions-seed-81");

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

    // Read until the authoritative inventory snapshot arrives.
    let saw_held_slot = true;
    let mut saw_inventory = false;
    let inventory_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !saw_inventory {
        let frame = client
            .read_frame_with_timeout(
                inventory_deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .context("wait for authoritative post-login inventory")?;

        match frame.id {
            id if id == ClientboundSetHeldSlot::ID => {
                let mut body = frame.body.clone();
                let held = ClientboundSetHeldSlot::decode(&mut body)?;
                observations.push(ObservationFact::HeldSlotChanged { slot: held.slot });
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

    let saw_echo = observe_held_slot_until_command_fence(&mut client, &mut observations).await?;

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

    let saw_second_echo =
        observe_held_slot_until_command_fence(&mut client, &mut observations).await?;

    observations.push(ObservationFact::Note {
        key: "second_held_slot_echo_observed".into(),
        value: saw_second_echo.to_string(),
    });

    Ok(observations.normalize_sequence())
}

async fn observe_held_slot_until_command_fence(
    client: &mut Client,
    observations: &mut ObservationSet,
) -> Result<bool> {
    client
        .write_packet(&ServerboundChatCommand {
            command: ORACLE_FENCE_COMMAND.to_string(),
        })
        .await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut saw_echo = false;
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .context("held-slot command fence did not produce system chat")?;
        if frame.id == ClientboundSetHeldSlot::ID {
            let mut body = frame.body.clone();
            let held = ClientboundSetHeldSlot::decode(&mut body)?;
            observations.push(ObservationFact::HeldSlotChanged { slot: held.slot });
            saw_echo = true;
        } else if frame.id == ClientboundKeepAlive::ID {
            let mut body = frame.body.clone();
            let keepalive = ClientboundKeepAlive::decode(&mut body)?;
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await?;
        } else if frame.id == ClientboundSystemChat::ID {
            let mut body = frame.body.clone();
            let _feedback = ClientboundSystemChat::decode(&mut body)?;
            return Ok(saw_echo);
        }
    }
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

    // Match only the deterministic command-spawned entity. Natural spawn packets
    // are ignored by type and position instead of being drained for a guessed time.
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
        (
            zombie_type_id,
            f64::from(summon_x),
            f64::from(summon_y),
            f64::from(summon_z),
        ),
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

    Ok(observations.normalize_sequence())
}

async fn drain_entity_lifecycle_frames(
    client: &mut Client,
    observations: &mut ObservationSet,
    expected_entity: (i32, f64, f64, f64),
) -> Result<u32> {
    let flags = MovePlayerFlags::new(false, false);
    client
        .write_packet(&ServerboundMovePlayerStatusOnly { flags })
        .await?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .context("timed out waiting for command-spawned entity packet")?;
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
                let (entity_type_id, ex, ey, ez) = expected_entity;
                if add.entity_type_id == entity_type_id
                    && (add.x - ex).abs() <= 1.5
                    && (add.y - ey).abs() <= 2.0
                    && (add.z - ez).abs() <= 1.5
                {
                    observations.push(ObservationFact::Note {
                        key: "entity_spawn_packet_seen".into(),
                        value: "true".into(),
                    });
                    return Ok(1);
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
    vanilla
        .wait_for_log(Duration::from_secs(10), |line| {
            line.contains("vanilla") && (line.contains("server operator") || line.contains("Opped"))
        })
        .expect("vanilla operator command completes");
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

        client
            .write_packet(&ServerboundMovePlayerPos {
                x: sync.x,
                y: sync.y,
                z: sync.z,
                flags,
            })
            .await?;
    }

    client
        .write_packet(&ServerboundChatCommand {
            command: ORACLE_FENCE_COMMAND.to_string(),
        })
        .await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .context("timed action command fence did not produce system chat")?;
        if frame.id == ClientboundKeepAlive::ID {
            let mut body = frame.body.clone();
            let keepalive = ClientboundKeepAlive::decode(&mut body)?;
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await?;
            keepalive_count += 1;
        } else if frame.id == ClientboundSystemChat::ID {
            let mut body = frame.body.clone();
            let _feedback = ClientboundSystemChat::decode(&mut body)?;
            observations.push(ObservationFact::PacketSeen {
                id: ClientboundSystemChat::ID,
            });
            break;
        }
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
        value: "command_response".into(),
    });

    Ok(observations.normalize_sequence())
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

// ---------------------------------------------------------------------------
// T01-05: checked block transaction/rejection/resync oracle
// ---------------------------------------------------------------------------

const BLOCK_TRANSACTION_ORACLE_MANIFEST_JSON: &str = r#"{
  "schema": "solaris.block_transaction.oracle.v1",
  "id": "block-transaction-26-1-2",
  "phases": [
    {"id":"accepted-break","case":"accepted_break","sequence":1},
    {"id":"accepted-place","case":"accepted_place","sequence":2},
    {"id":"occupied-place-rejection","case":"occupied_place_rejection","sequence":3},
    {"id":"out-of-reach-break-rejection","case":"out_of_reach_break_rejection","sequence":4},
    {"id":"early-stop-break-rejection","case":"early_stop_break_rejection","sequence":6}
  ]
}"#;

fn block_transaction_oracle_manifest() -> BlockTransactionOracleManifest {
    BlockTransactionOracleManifest::from_json(BLOCK_TRANSACTION_ORACLE_MANIFEST_JSON)
        .expect("checked block transaction oracle manifest")
}

async fn enter_block_oracle_play(
    ctx: ScenarioContext,
) -> Result<(Client, SynchronizePlayerPosition, String)> {
    let subject = match ctx.kind {
        ServerKind::Solaris => "solaris",
        ServerKind::Vanilla => "vanilla",
    }
    .to_string();
    let mut client = Client::connect(ctx.addr).await?;
    let _login = client.drive_login(ctx.addr, &subject).await?;
    client.drive_configuration().await?;

    let _: LoginPlay = read_typed_skipping_startup_noise(&mut client).await?;
    let _: ClientboundChangeDifficulty = read_typed_skipping_startup_noise(&mut client).await?;
    let _: ClientboundPlayerAbilities = read_typed_skipping_startup_noise(&mut client).await?;
    let _: ClientboundSetHeldSlot = read_typed_skipping_startup_noise(&mut client).await?;
    let _: EntityEvent = read_typed_skipping_startup_noise(&mut client).await?;
    read_packet_id_skipping_startup_noise(&mut client, ClientboundCommands::ID).await?;
    let sync = read_spawn_position(&mut client).await?;
    let _: ClientboundInitializeBorder = read_typed_skipping_startup_noise(&mut client).await?;
    let _: ClientboundSetTime = read_typed_skipping_startup_noise(&mut client).await?;
    let _: SetDefaultSpawnPosition = read_typed_skipping_startup_noise(&mut client).await?;
    let _: GameEvent = read_typed_skipping_startup_noise(&mut client).await?;
    let _: SetCenterChunk = read_typed_skipping_startup_noise(&mut client).await?;
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await?;
    client.write_packet(&ServerboundPlayerLoaded).await?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .context("block oracle did not receive a Play chunk")?;
        if frame.id == ClientboundKeepAlive::ID {
            let keepalive = ClientboundKeepAlive::decode(&mut frame.body.clone())?;
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await?;
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            break;
        }
    }
    Ok((client, sync, subject))
}

fn oracle_is_command_fence_feedback(content_nbt: &[u8]) -> Result<bool> {
    let mut bytes = Bytes::copy_from_slice(content_nbt);
    let tag = mc_nbt::read_network(&mut bytes).context("decode oracle command feedback NBT")?;
    let rendered = format!("{tag:?}");
    let normalized = rendered.to_ascii_lowercase();
    Ok(rendered.contains("command.unknown.command") || normalized.contains("unknown command"))
}

const ORACLE_FENCE_COMMAND: &str = "__solaris_oracle_fence__";

async fn block_oracle_command_fence(client: &mut Client, command: String) -> Result<()> {
    let needs_fence = command != ORACLE_FENCE_COMMAND;
    client
        .write_packet(&ServerboundChatCommand { command })
        .await?;
    if needs_fence {
        client
            .write_packet(&ServerboundChatCommand {
                command: ORACLE_FENCE_COMMAND.to_string(),
            })
            .await?;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .context("block oracle command did not produce feedback")?;
        if frame.id == ClientboundKeepAlive::ID {
            let keepalive = ClientboundKeepAlive::decode(&mut frame.body.clone())?;
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await?;
        } else if frame.id == SynchronizePlayerPosition::ID {
            let sync = SynchronizePlayerPosition::decode(&mut frame.body.clone())?;
            client
                .write_packet(&ConfirmTeleportation {
                    teleport_id: sync.teleport_id,
                })
                .await?;
        } else if frame.id == ClientboundSystemChat::ID {
            let feedback = ClientboundSystemChat::decode(&mut frame.body.clone())?;
            if oracle_is_command_fence_feedback(&feedback.content_nbt)? {
                return Ok(());
            }
        }
    }
}

async fn oracle_give_and_select_item(
    client: &mut Client,
    subject: &str,
    item_name: &str,
    item_id: u32,
    count: i32,
    solaris_hotbar_slot: i16,
    open_menu_hotbar_base: Option<i16>,
) -> Result<i16> {
    let command = if subject == "solaris" {
        format!("debug give {item_name} {count} {solaris_hotbar_slot}")
    } else {
        format!("give {subject} {item_name} {count}")
    };
    client
        .write_packet(&ServerboundChatCommand { command })
        .await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut feedback = false;
    let mut hotbar_slot = None;
    while !(feedback && hotbar_slot.is_some()) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .with_context(|| format!("oracle item setup did not converge for {item_name}"))?;
        if frame.id == ClientboundKeepAlive::ID {
            let keepalive = ClientboundKeepAlive::decode(&mut frame.body.clone())?;
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await?;
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let update = ClientboundContainerSetSlot::decode(&mut frame.body.clone())?;
            if update.item_stack.item_id == item_id && update.item_stack.count >= count {
                hotbar_slot = match update.container_id {
                    -2 if (0..=8).contains(&update.slot) => Some(update.slot),
                    0 if (36..=44).contains(&update.slot) => Some(update.slot - 36),
                    id if id > 0 => open_menu_hotbar_base
                        .filter(|base| (*base..=*base + 8).contains(&update.slot))
                        .map(|base| update.slot - base)
                        .or(hotbar_slot),
                    _ => hotbar_slot,
                };
            }
        } else if frame.id == ClientboundContainerSetContent::ID {
            let content = ClientboundContainerSetContent::decode(&mut frame.body.clone())?;
            let hotbar_range = if content.container_id == 0 {
                Some((36_usize, 44_usize))
            } else {
                open_menu_hotbar_base.map(|base| {
                    let start = usize::try_from(base).expect("menu hotbar base is non-negative");
                    (start, start + 8)
                })
            };
            if let Some((start, end)) = hotbar_range {
                hotbar_slot = content.items.iter().enumerate().find_map(|(slot, stack)| {
                    (stack.item_id == item_id
                        && stack.count >= count
                        && (start..=end).contains(&slot))
                    .then(|| i16::try_from(slot - start).expect("hotbar slot fits i16"))
                });
            }
        } else if frame.id == ClientboundSystemChat::ID {
            let _ = ClientboundSystemChat::decode(&mut frame.body.clone())?;
            feedback = true;
        }
    }
    let hotbar_slot = hotbar_slot.expect("loop requires an authoritative hotbar slot");
    client
        .write_packet(&ServerboundSetCarriedItem { slot: hotbar_slot })
        .await?;
    block_oracle_command_fence(client, ORACLE_FENCE_COMMAND.to_string())
        .await
        .with_context(|| format!("fence selected oracle item {item_name}"))?;
    Ok(hotbar_slot)
}

async fn block_oracle_give_and_select_dirt(
    client: &mut Client,
    subject: &str,
    dirt_item_id: u32,
) -> Result<i16> {
    oracle_give_and_select_item(client, subject, "minecraft:dirt", dirt_item_id, 64, 0, None).await
}

async fn block_oracle_move_x(
    client: &mut Client,
    from_x: f64,
    to_x: f64,
    y: f64,
    z: f64,
) -> Result<()> {
    const STEPS: u32 = 10;
    for step in 1..=STEPS {
        let fraction = f64::from(step) / f64::from(STEPS);
        client
            .write_packet(&ServerboundMovePlayerPos {
                x: from_x + (to_x - from_x) * fraction,
                y,
                z,
                flags: MovePlayerFlags::new(true, false),
            })
            .await?;
    }
    block_oracle_command_fence(client, ORACLE_FENCE_COMMAND.to_string()).await
}

async fn oracle_teleport_to(client: &mut Client, position: (f64, f64, f64)) -> Result<()> {
    client
        .write_packet(&ServerboundChatCommand {
            command: format!("tp {} {} {}", position.0, position.1, position.2),
        })
        .await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .context("container oracle teleport did not synchronize")?;
        if frame.id == ClientboundKeepAlive::ID {
            let keepalive = ClientboundKeepAlive::decode(&mut frame.body.clone())?;
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await?;
        } else if frame.id == SynchronizePlayerPosition::ID {
            let sync = SynchronizePlayerPosition::decode(&mut frame.body.clone())?;
            client
                .write_packet(&ConfirmTeleportation {
                    teleport_id: sync.teleport_id,
                })
                .await?;
            anyhow::ensure!(
                (sync.x - position.0).abs() < 0.001
                    && (sync.y - position.1).abs() < 0.001
                    && (sync.z - position.2).abs() < 0.001,
                "container oracle teleport landed at ({}, {}, {}), expected ({}, {}, {})",
                sync.x,
                sync.y,
                sync.z,
                position.0,
                position.1,
                position.2
            );
            break;
        }
    }
    block_oracle_command_fence(client, ORACLE_FENCE_COMMAND.to_string())
        .await
        .context("fence confirmed container oracle teleport")
}

fn sign_extend_section(value: i64, bits: u32) -> i32 {
    let shift = 64 - bits;
    ((value << shift) >> shift) as i32
}

fn section_target_state(packet: &SectionBlocksUpdate, target: (i32, i32, i32)) -> Option<i32> {
    let section_x = sign_extend_section((packet.section_pos >> 42) & 0x3f_ffff, 22);
    let section_z = sign_extend_section((packet.section_pos >> 20) & 0x3f_ffff, 22);
    let section_y = sign_extend_section(packet.section_pos & 0x0f_ffff, 20);
    packet.changes.iter().find_map(|change| {
        let local_x = i32::from((change.relative_pos >> 8) & 15);
        let local_z = i32::from((change.relative_pos >> 4) & 15);
        let local_y = i32::from(change.relative_pos & 15);
        let position = (
            section_x * 16 + local_x,
            section_y * 16 + local_y,
            section_z * 16 + local_z,
        );
        (position == target).then_some(change.state_id)
    })
}

fn push_normalized_block_oracle_event(
    events: &mut Vec<BlockTransactionOracleEvent>,
    event: BlockTransactionOracleEvent,
) {
    if matches!(
        (&event, events.last()),
        (
            BlockTransactionOracleEvent::TargetUpdate { state_id: current },
            Some(BlockTransactionOracleEvent::TargetUpdate { state_id: previous })
        ) if current == previous
    ) {
        return;
    }
    events.push(event);
}

async fn collect_block_transaction_phase(
    client: &mut Client,
    phase: &BlockTransactionOraclePhase,
    target: (i32, i32, i32),
) -> Result<BlockTransactionOraclePhaseTrace> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut events = Vec::new();
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .with_context(|| format!("block oracle phase {} stalled", phase.id))?;
        if frame.id == ClientboundKeepAlive::ID {
            let keepalive = ClientboundKeepAlive::decode(&mut frame.body.clone())?;
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await?;
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let update = BlockUpdate::decode(&mut frame.body.clone())?;
            if unpack_block_pos(update.position) == target {
                push_normalized_block_oracle_event(
                    &mut events,
                    BlockTransactionOracleEvent::TargetUpdate {
                        state_id: update.state_id,
                    },
                );
            }
        } else if frame.id == SectionBlocksUpdate::ID {
            let update = SectionBlocksUpdate::decode(&mut frame.body.clone())?;
            if let Some(state_id) = section_target_state(&update, target) {
                push_normalized_block_oracle_event(
                    &mut events,
                    BlockTransactionOracleEvent::TargetUpdate { state_id },
                );
            }
        } else if frame.id == BlockChangedAck::ID {
            let ack = BlockChangedAck::decode(&mut frame.body.clone())?;
            if ack.sequence == phase.sequence {
                events.push(BlockTransactionOracleEvent::Ack {
                    sequence: ack.sequence,
                });
                client
                    .write_packet(&ServerboundChatCommand {
                        command: "list".to_string(),
                    })
                    .await?;
            }
        } else if frame.id == ClientboundSystemChat::ID
            && events.iter().any(|event| {
                matches!(
                    event,
                    BlockTransactionOracleEvent::Ack { sequence } if *sequence == phase.sequence
                )
            })
        {
            let _ = ClientboundSystemChat::decode(&mut frame.body.clone())?;
            return Ok(BlockTransactionOraclePhaseTrace {
                id: phase.id.clone(),
                events,
            });
        }
    }
}

async fn observe_block_transaction_oracle(
    ctx: ScenarioContext,
    manifest: &BlockTransactionOracleManifest,
) -> Result<BlockTransactionOracleTrace> {
    manifest.validate()?;
    let (mut client, spawn, subject) = enter_block_oracle_play(ctx).await?;
    block_oracle_command_fence(&mut client, "gamemode creative".to_string())
        .await
        .context("fence container oracle gamemode setup")?;
    let items_report =
        mc_data::items::load_items_report(local_vanilla_dir().join("reports/registries.json"))?;
    let item_registry = mc_data::items::ItemRegistry::from_report(&items_report);
    let dirt_item_id = item_registry
        .id_of(&mc_data::Identifier::parse("minecraft:dirt")?)
        .context("vanilla item registry has minecraft:dirt")?;
    let _selected_hotbar_slot =
        block_oracle_give_and_select_dirt(&mut client, &subject, dirt_item_id).await?;

    let target = (
        spawn.x.floor() as i32,
        spawn.y.floor() as i32 - 1,
        spawn.z.floor() as i32,
    );
    let working_x = spawn.x + 2.5;
    block_oracle_move_x(&mut client, spawn.x, working_x, spawn.y, spawn.z)
        .await
        .context("fence container oracle initial movement")?;
    let mut traces = Vec::with_capacity(manifest.phases.len());
    for phase in &manifest.phases {
        match phase.case {
            BlockTransactionOracleCase::AcceptedBreak => {
                client
                    .write_packet(&ServerboundPlayerAction {
                        action: PlayerActionKind::StartDestroyBlock,
                        position: pack_block_pos(target.0, target.1, target.2),
                        direction: Direction::Up,
                        sequence: phase.sequence,
                    })
                    .await?;
            }
            BlockTransactionOracleCase::AcceptedPlace => {
                let clicked = (target.0, target.1 - 1, target.2);
                client
                    .write_packet(&ServerboundUseItemOn {
                        hand: InteractionHand::MainHand,
                        position: pack_block_pos(clicked.0, clicked.1, clicked.2),
                        direction: Direction::Up,
                        cursor_x: 0.5,
                        cursor_y: 1.0,
                        cursor_z: 0.5,
                        inside: false,
                        world_border_hit: false,
                        sequence: phase.sequence,
                    })
                    .await?;
            }
            BlockTransactionOracleCase::OccupiedPlaceRejection => {
                client
                    .write_packet(&ServerboundUseItemOn {
                        hand: InteractionHand::MainHand,
                        position: pack_block_pos(target.0, target.1, target.2),
                        direction: Direction::Down,
                        cursor_x: 0.5,
                        cursor_y: 0.0,
                        cursor_z: 0.5,
                        inside: false,
                        world_border_hit: false,
                        sequence: phase.sequence,
                    })
                    .await?;
            }
            BlockTransactionOracleCase::OutOfReachBreakRejection => {
                block_oracle_move_x(
                    &mut client,
                    working_x,
                    f64::from(target.0) + 20.5,
                    spawn.y,
                    spawn.z,
                )
                .await?;
                client
                    .write_packet(&ServerboundPlayerAction {
                        action: PlayerActionKind::StartDestroyBlock,
                        position: pack_block_pos(target.0, target.1, target.2),
                        direction: Direction::Up,
                        sequence: phase.sequence,
                    })
                    .await?;
            }
            BlockTransactionOracleCase::EarlyStopBreakRejection => {
                block_oracle_move_x(
                    &mut client,
                    f64::from(target.0) + 20.5,
                    working_x,
                    spawn.y,
                    spawn.z,
                )
                .await?;
                block_oracle_command_fence(&mut client, "gamemode survival".to_string()).await?;
                client
                    .write_packet(&ServerboundPlayerAction {
                        action: PlayerActionKind::StartDestroyBlock,
                        position: pack_block_pos(target.0, target.1, target.2),
                        direction: Direction::Up,
                        sequence: 5,
                    })
                    .await?;
                client
                    .write_packet(&ServerboundPlayerAction {
                        action: PlayerActionKind::StopDestroyBlock,
                        position: pack_block_pos(target.0, target.1, target.2),
                        direction: Direction::Up,
                        sequence: phase.sequence,
                    })
                    .await?;
            }
        }
        traces.push(collect_block_transaction_phase(&mut client, phase, target).await?);
    }

    let trace = BlockTransactionOracleTrace {
        manifest_id: manifest.id.clone(),
        phases: traces,
    };
    trace.validate_against(manifest)?;
    let accepted_place_state = trace.phases[1]
        .events
        .iter()
        .find_map(|event| match event {
            BlockTransactionOracleEvent::TargetUpdate { state_id } => Some(*state_id),
            BlockTransactionOracleEvent::Ack { .. } => None,
        })
        .context("accepted place did not publish a target state")?;
    anyhow::ensure!(
        accepted_place_state != 0,
        "accepted place published air instead of an occupied block state"
    );
    let occupied_resync_state = trace.phases[2]
        .events
        .iter()
        .find_map(|event| match event {
            BlockTransactionOracleEvent::TargetUpdate { state_id } => Some(*state_id),
            BlockTransactionOracleEvent::Ack { .. } => None,
        })
        .context("occupied placement rejection did not resync the target")?;
    anyhow::ensure!(
        occupied_resync_state == accepted_place_state,
        "occupied rejection resynced state {occupied_resync_state}, expected placed state {accepted_place_state}"
    );
    Ok(trace)
}

#[test]
fn block_oracle_normalizes_only_consecutive_identical_target_updates() {
    let mut events = Vec::new();
    push_normalized_block_oracle_event(
        &mut events,
        BlockTransactionOracleEvent::TargetUpdate { state_id: 10 },
    );
    push_normalized_block_oracle_event(
        &mut events,
        BlockTransactionOracleEvent::TargetUpdate { state_id: 10 },
    );
    push_normalized_block_oracle_event(
        &mut events,
        BlockTransactionOracleEvent::Ack { sequence: 2 },
    );
    push_normalized_block_oracle_event(
        &mut events,
        BlockTransactionOracleEvent::TargetUpdate { state_id: 10 },
    );
    push_normalized_block_oracle_event(
        &mut events,
        BlockTransactionOracleEvent::TargetUpdate { state_id: 0 },
    );
    assert_eq!(
        events,
        [
            BlockTransactionOracleEvent::TargetUpdate { state_id: 10 },
            BlockTransactionOracleEvent::Ack { sequence: 2 },
            BlockTransactionOracleEvent::TargetUpdate { state_id: 10 },
            BlockTransactionOracleEvent::TargetUpdate { state_id: 0 },
        ]
    );
}

#[tokio::test]
#[ignore = "requires local .analysis/server.jar, Java 25, and data/vanilla sidecars"]
async fn checked_block_transaction_manifest_matches_vanilla_oracle_and_solaris() {
    let availability = vanilla_oracle_availability(repo_root());
    let OracleAvailability::Available { jar } = availability else {
        panic!(
            "{}",
            availability.skip_message().expect("oracle unavailable")
        );
    };
    assert!(local_vanilla_dir().join("reports/blocks.json").is_file());
    let manifest = block_transaction_oracle_manifest();

    let vanilla_dir = tempfile::tempdir().expect("vanilla block oracle tempdir");
    std::fs::write(
        vanilla_dir.path().join("ops.json"),
        serde_json::to_vec_pretty(&serde_json::json!([{
            "uuid": mc_net::offline_uuid("vanilla").to_string(),
            "name": "vanilla",
            "level": 4,
            "bypassesPlayerLimit": false
        }]))
        .expect("serialize vanilla block oracle ops.json"),
    )
    .expect("write vanilla block oracle ops.json");
    let vanilla = VanillaServerProcess::launch(&jar, vanilla_dir.path(), Duration::from_secs(90))
        .expect("vanilla block oracle starts");

    let (solaris, solaris_addr) = spawn_solaris_with_local_vanilla_data()
        .await
        .expect("spawn Solaris block oracle");
    let solaris_task = tokio::spawn(async move { solaris.serve().await });

    let vanilla_trace = observe_block_transaction_oracle(
        ScenarioContext {
            kind: ServerKind::Vanilla,
            addr: vanilla.addr(),
        },
        &manifest,
    )
    .await
    .expect("vanilla block transaction trace");
    let solaris_trace = observe_block_transaction_oracle(
        ScenarioContext {
            kind: ServerKind::Solaris,
            addr: solaris_addr,
        },
        &manifest,
    )
    .await
    .expect("Solaris block transaction trace");

    eprintln!(
        "T01-05 vanilla trace: {}",
        serde_json::to_string(&vanilla_trace).expect("serialize vanilla trace")
    );
    eprintln!(
        "T01-05 Solaris trace: {}",
        serde_json::to_string(&solaris_trace).expect("serialize Solaris trace")
    );
    assert_eq!(
        vanilla_trace, solaris_trace,
        "Solaris block transaction/rejection/resync order diverged from vanilla"
    );
    let artifact_dir = repo_root().join(".analysis/runs/t01-05-a");
    std::fs::create_dir_all(&artifact_dir).expect("create T01-05 artifact dir");
    std::fs::write(
        artifact_dir.join("block-transaction-oracle.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": BLOCK_TRANSACTION_ORACLE_SCHEMA,
            "manifest": manifest,
            "vanilla": vanilla_trace,
            "solaris": solaris_trace,
        }))
        .expect("serialize T01-05 oracle artifact"),
    )
    .expect("write T01-05 oracle artifact");

    vanilla.stop().expect("vanilla block oracle stops");
    solaris_task.abort();
}

// ---------------------------------------------------------------------------
// T01-06: checked inventory/container/crafting state oracle
// ---------------------------------------------------------------------------

const CONTAINER_STATE_ORACLE_MANIFEST_JSON: &str = r#"{
  "schema": "solaris.container_state.oracle.v1",
  "id": "inventory-container-26-1-2",
  "phases": [
    {"id":"chest-initial","menu":"chest","case":"chest_initial"},
    {"id":"chest-quick-move-in","menu":"chest","case":"chest_quick_move_in"},
    {"id":"chest-quick-move-out","menu":"chest","case":"chest_quick_move_out"},
    {"id":"chest-stale-click","menu":"chest","case":"chest_stale_click"},
    {"id":"chest-reopen","menu":"chest","case":"chest_reopen"},
    {"id":"craft-initial","menu":"crafting_table","case":"craft_initial"},
    {"id":"craft-prepared","menu":"crafting_table","case":"craft_prepared"},
    {"id":"craft-quick-move","menu":"crafting_table","case":"craft_quick_move"},
    {"id":"craft-stale-click","menu":"crafting_table","case":"craft_stale_click"},
    {"id":"craft-reopen","menu":"crafting_table","case":"craft_reopen"}
  ]
}"#;

fn container_state_oracle_manifest() -> ContainerStateOracleManifest {
    ContainerStateOracleManifest::from_json(CONTAINER_STATE_ORACLE_MANIFEST_JSON)
        .expect("checked container state oracle manifest")
}

#[derive(Debug, Clone)]
struct OracleMenuState {
    container_id: i32,
    menu: ContainerStateOracleMenu,
    baseline_state_id: i32,
    state_id: i32,
    items: Vec<ItemStack>,
    carried: ItemStack,
}

impl OracleMenuState {
    fn snapshot(&self) -> Result<ContainerStateOracleSnapshot> {
        let state_id_delta = self
            .state_id
            .checked_sub(self.baseline_state_id)
            .context("container state id regressed below its open baseline")?;
        let slots = self
            .items
            .iter()
            .enumerate()
            .map(|(slot, stack)| {
                Ok(ContainerStateOracleSlot {
                    slot: u16::try_from(slot).context("container slot exceeds u16")?,
                    stack: oracle_stack(stack),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(ContainerStateOracleSnapshot {
            menu: self.menu,
            state_id_delta,
            slots,
            cursor: oracle_stack(&self.carried),
        })
    }

    fn contents_equal(&self, other: &Self) -> bool {
        self.items == other.items && self.carried == other.carried
    }
}

fn oracle_stack(stack: &ItemStack) -> ContainerStateOracleStack {
    ContainerStateOracleStack {
        item_id: stack.item_id,
        count: stack.count,
    }
}

async fn oracle_open_menu(
    client: &mut Client,
    target: (i32, i32, i32),
    sequence: i32,
    expected_menu_type: i32,
    menu: ContainerStateOracleMenu,
) -> Result<OracleMenuState> {
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(target.0, target.1, target.2),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence,
        })
        .await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut opened = None;
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .context("container oracle menu did not open")?;
        if frame.id == ClientboundKeepAlive::ID {
            let keepalive = ClientboundKeepAlive::decode(&mut frame.body.clone())?;
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await?;
        } else if frame.id == ClientboundOpenScreen::ID {
            let packet = ClientboundOpenScreen::decode(&mut frame.body.clone())?;
            anyhow::ensure!(
                packet.menu_type == expected_menu_type,
                "container oracle opened menu type {}, expected {}",
                packet.menu_type,
                expected_menu_type
            );
            opened = Some(packet.container_id);
        } else if frame.id == ClientboundContainerSetContent::ID {
            let content = ClientboundContainerSetContent::decode(&mut frame.body.clone())?;
            if opened == Some(content.container_id) {
                return Ok(OracleMenuState {
                    container_id: content.container_id,
                    menu,
                    baseline_state_id: content.state_id,
                    state_id: content.state_id,
                    items: content.items,
                    carried: content.carried_item,
                });
            }
        } else if frame.id == ClientboundContainerClose::ID {
            let close = ClientboundContainerClose::decode(&mut frame.body.clone())?;
            anyhow::bail!(
                "container oracle menu {} closed while opening",
                close.container_id
            );
        }
    }
}

fn oracle_menu_slot_for_player_inventory(
    menu: ContainerStateOracleMenu,
    player_slot: i16,
) -> Option<usize> {
    match menu {
        ContainerStateOracleMenu::Chest => match player_slot {
            9..=35 => usize::try_from(27 + player_slot - 9).ok(),
            36..=44 => usize::try_from(54 + player_slot - 36).ok(),
            _ => None,
        },
        ContainerStateOracleMenu::CraftingTable => match player_slot {
            9..=35 => usize::try_from(10 + player_slot - 9).ok(),
            36..=44 => usize::try_from(37 + player_slot - 36).ok(),
            _ => None,
        },
    }
}

fn oracle_apply_slot_update(state: &mut OracleMenuState, update: ClientboundContainerSetSlot) {
    if update.container_id == state.container_id && update.slot >= 0 {
        let slot = usize::try_from(update.slot).expect("non-negative slot fits usize");
        if slot < state.items.len() {
            state.items[slot] = update.item_stack;
            state.state_id = update.state_id;
        }
    } else if update.container_id == 0 {
        if let Some(slot) = oracle_menu_slot_for_player_inventory(state.menu, update.slot)
            && slot < state.items.len()
        {
            state.items[slot] = update.item_stack;
        }
    } else if update.container_id == -2 {
        let player_slot = match update.slot {
            0..=8 => update.slot + 36,
            9..=35 => update.slot,
            _ => return,
        };
        if let Some(slot) = oracle_menu_slot_for_player_inventory(state.menu, player_slot)
            && slot < state.items.len()
        {
            state.items[slot] = update.item_stack;
        }
    } else if update.container_id == -1 && update.slot == -1 {
        state.carried = update.item_stack;
    }
}

#[derive(Debug, Clone, Copy)]
struct OracleMenuClick {
    state_id: i32,
    slot_num: i16,
    button_num: i8,
    container_input: ContainerInput,
    require_update: bool,
    require_full_content: bool,
}

async fn oracle_click_menu_until<F>(
    client: &mut Client,
    state: &mut OracleMenuState,
    click: OracleMenuClick,
    predicate: F,
) -> Result<()>
where
    F: Fn(&OracleMenuState) -> bool,
{
    client
        .write_packet(&ServerboundContainerClick {
            container_id: state.container_id,
            state_id: click.state_id,
            slot_num: click.slot_num,
            button_num: click.button_num,
            container_input: click.container_input,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await?;
    client
        .write_packet(&ServerboundChatCommand {
            command: ORACLE_FENCE_COMMAND.to_string(),
        })
        .await?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut saw_initial_fence = false;
    let mut quiescence_probe_pending = false;
    let mut relevant_since_probe = false;
    let mut saw_relevant_update = false;
    let mut saw_full_content = false;
    loop {
        let semantic_ready = (!click.require_update || saw_relevant_update)
            && (!click.require_full_content || saw_full_content)
            && predicate(state);
        if saw_initial_fence && semantic_ready && !quiescence_probe_pending {
            client
                .write_packet(&ServerboundChatCommand {
                    command: ORACLE_FENCE_COMMAND.to_string(),
                })
                .await?;
            quiescence_probe_pending = true;
            relevant_since_probe = false;
        }

        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .context("container oracle click did not reach its semantic predicate")?;
        if frame.id == ClientboundKeepAlive::ID {
            let keepalive = ClientboundKeepAlive::decode(&mut frame.body.clone())?;
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await?;
        } else if frame.id == ClientboundContainerSetContent::ID {
            let content = ClientboundContainerSetContent::decode(&mut frame.body.clone())?;
            if content.container_id == state.container_id {
                state.state_id = content.state_id;
                state.items = content.items;
                state.carried = content.carried_item;
                saw_relevant_update = true;
                saw_full_content = true;
                relevant_since_probe |= quiescence_probe_pending;
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let update = ClientboundContainerSetSlot::decode(&mut frame.body.clone())?;
            let relevant = update.container_id == state.container_id
                || update.container_id == 0
                || update.container_id == -2
                || (update.container_id == -1 && update.slot == -1);
            oracle_apply_slot_update(state, update);
            saw_relevant_update |= relevant;
            relevant_since_probe |= relevant && quiescence_probe_pending;
        } else if frame.id == ClientboundContainerClose::ID {
            let close = ClientboundContainerClose::decode(&mut frame.body.clone())?;
            anyhow::bail!(
                "container oracle menu {} closed during click",
                close.container_id
            );
        } else if frame.id == ClientboundSystemChat::ID {
            let feedback = ClientboundSystemChat::decode(&mut frame.body.clone())?;
            if oracle_is_command_fence_feedback(&feedback.content_nbt)? {
                if !saw_initial_fence {
                    saw_initial_fence = true;
                } else if quiescence_probe_pending {
                    let semantic_ready = (!click.require_update || saw_relevant_update)
                        && (!click.require_full_content || saw_full_content)
                        && predicate(state);
                    if semantic_ready && !relevant_since_probe {
                        return Ok(());
                    }
                    quiescence_probe_pending = false;
                    relevant_since_probe = false;
                }
            }
        }
    }
}

async fn oracle_close_menu(client: &mut Client, container_id: i32) -> Result<()> {
    client
        .write_packet(&ServerboundContainerClose { container_id })
        .await?;
    block_oracle_command_fence(client, ORACLE_FENCE_COMMAND.to_string())
        .await
        .context("fence menu close")
}

fn oracle_phase(
    manifest: &ContainerStateOracleManifest,
    index: usize,
    state: &OracleMenuState,
) -> Result<ContainerStateOraclePhaseTrace> {
    Ok(ContainerStateOraclePhaseTrace {
        id: manifest.phases[index].id.clone(),
        snapshot: state.snapshot()?,
    })
}

async fn observe_container_state_oracle(
    ctx: ScenarioContext,
    manifest: &ContainerStateOracleManifest,
) -> Result<ContainerStateOracleTrace> {
    manifest.validate()?;
    let (mut client, _spawn, subject) = enter_block_oracle_play(ctx).await?;
    block_oracle_command_fence(&mut client, "gamemode creative".to_string())
        .await
        .context("fence container oracle creative mode")?;

    let items_report =
        mc_data::items::load_items_report(local_vanilla_dir().join("reports/registries.json"))?;
    let items = mc_data::items::ItemRegistry::from_report(&items_report);
    let item_id = |name: &str| -> Result<u32> {
        items
            .id_of(&mc_data::Identifier::parse(name)?)
            .with_context(|| format!("vanilla item registry has {name}"))
    };
    let dirt_item_id = item_id("minecraft:dirt")?;
    let oak_log_item_id = item_id("minecraft:oak_log")?;
    let oak_planks_item_id = item_id("minecraft:oak_planks")?;

    let chest_target = (2, 200, 0);
    let table_target = (4, 200, 0);
    if ctx.kind == ServerKind::Vanilla {
        block_oracle_command_fence(&mut client, "forceload add 0 0".to_string())
            .await
            .context("force-load vanilla container oracle chunk")?;
        for command in [
            "setblock 2 200 0 minecraft:chest replace",
            "setblock 2 201 0 minecraft:air replace",
            "setblock 4 200 0 minecraft:crafting_table replace",
            "setblock 3 200 2 minecraft:stone replace",
            "setblock 3 201 2 minecraft:air replace",
            "setblock 3 202 2 minecraft:air replace",
        ] {
            block_oracle_command_fence(&mut client, command.to_string())
                .await
                .with_context(|| format!("apply vanilla container fixture command: {command}"))?;
        }
    }
    oracle_teleport_to(&mut client, (3.5, 201.0, 2.5))
        .await
        .context("teleport to pre-seeded container oracle fixture")?;

    let dirt_hotbar = oracle_give_and_select_item(
        &mut client,
        &subject,
        "minecraft:dirt",
        dirt_item_id,
        2,
        0,
        None,
    )
    .await?;

    let mut phases = Vec::with_capacity(manifest.phases.len());
    let mut chest = oracle_open_menu(
        &mut client,
        chest_target,
        21,
        2,
        ContainerStateOracleMenu::Chest,
    )
    .await
    .context("open initial chest")?;
    anyhow::ensure!(
        chest.items.len() == 63,
        "single chest menu must expose 63 slots"
    );
    phases.push(oracle_phase(manifest, 0, &chest)?);

    let dirt_menu_slot = 54_i16
        .checked_add(dirt_hotbar)
        .context("chest hotbar slot overflow")?;
    let chest_state_id = chest.state_id;
    oracle_click_menu_until(
        &mut client,
        &mut chest,
        OracleMenuClick {
            state_id: chest_state_id,
            slot_num: dirt_menu_slot,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            require_update: true,
            require_full_content: false,
        },
        |state| {
            state.items[0].item_id == dirt_item_id
                && state.items[0].count == 2
                && state.items[usize::try_from(dirt_menu_slot).unwrap()].is_empty()
                && state.carried.is_empty()
        },
    )
    .await
    .context("chest quick-move-in semantic predicate")?;
    anyhow::ensure!(
        chest.items[0].item_id == dirt_item_id
            && chest.items[0].count == 2
            && chest.items[usize::try_from(dirt_menu_slot).unwrap()].is_empty()
            && chest.carried.is_empty(),
        "chest quick-move-in did not conserve the dirt stack"
    );
    let chest_after_in = chest.clone();
    phases.push(oracle_phase(manifest, 1, &chest)?);

    let chest_state_id = chest.state_id;
    oracle_click_menu_until(
        &mut client,
        &mut chest,
        OracleMenuClick {
            state_id: chest_state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            require_update: true,
            require_full_content: false,
        },
        |state| {
            state.items[0].is_empty()
                && state
                    .items
                    .iter()
                    .skip(27)
                    .any(|stack| stack.item_id == dirt_item_id && stack.count == 2)
                && state.carried.is_empty()
        },
    )
    .await
    .context("chest quick-move-out semantic predicate")?;
    anyhow::ensure!(
        chest.items[0].is_empty()
            && chest
                .items
                .iter()
                .skip(27)
                .any(|stack| { stack.item_id == dirt_item_id && stack.count == 2 })
            && chest.carried.is_empty(),
        "chest quick-move-out did not return the dirt stack to player inventory"
    );
    let chest_after_out = chest.clone();
    phases.push(oracle_phase(manifest, 2, &chest)?);

    let expected_chest_after_out = chest_after_out.clone();
    oracle_click_menu_until(
        &mut client,
        &mut chest,
        OracleMenuClick {
            state_id: chest_after_in.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            require_update: true,
            require_full_content: true,
        },
        move |state| state.contents_equal(&expected_chest_after_out),
    )
    .await
    .context("chest stale-click full resync predicate")?;
    anyhow::ensure!(
        chest.contents_equal(&chest_after_out),
        "stale chest click mutated slots or cursor instead of resyncing"
    );
    let mut chest_stale_phase = oracle_phase(manifest, 3, &chest)?;
    chest_stale_phase.snapshot.state_id_delta = phases
        .last()
        .expect("chest quick-move-out phase exists")
        .snapshot
        .state_id_delta;
    phases.push(chest_stale_phase);

    oracle_close_menu(&mut client, chest.container_id).await?;
    let chest_reopen = oracle_open_menu(
        &mut client,
        chest_target,
        22,
        2,
        ContainerStateOracleMenu::Chest,
    )
    .await
    .context("reopen chest after quick-move-out")?;
    anyhow::ensure!(
        chest_reopen.contents_equal(&OracleMenuState {
            baseline_state_id: chest_reopen.baseline_state_id,
            state_id: chest_reopen.state_id,
            container_id: chest_reopen.container_id,
            menu: chest_reopen.menu,
            items: chest_after_out.items.clone(),
            carried: chest_after_out.carried.clone(),
        }),
        "chest close/reopen did not conserve storage and player inventory"
    );
    phases.push(oracle_phase(manifest, 4, &chest_reopen)?);
    oracle_close_menu(&mut client, chest_reopen.container_id).await?;

    let log_hotbar = oracle_give_and_select_item(
        &mut client,
        &subject,
        "minecraft:oak_log",
        oak_log_item_id,
        1,
        0,
        Some(54),
    )
    .await?;

    let mut crafting = oracle_open_menu(
        &mut client,
        table_target,
        25,
        12,
        ContainerStateOracleMenu::CraftingTable,
    )
    .await
    .context("open initial crafting table")?;
    anyhow::ensure!(
        crafting.items.len() == 46,
        "crafting table menu must expose 46 slots"
    );
    phases.push(oracle_phase(manifest, 5, &crafting)?);

    let crafting_state_id = crafting.state_id;
    let log_menu_slot = usize::try_from(37_i16 + log_hotbar).expect("log menu slot fits usize");
    oracle_click_menu_until(
        &mut client,
        &mut crafting,
        OracleMenuClick {
            state_id: crafting_state_id,
            slot_num: 1,
            button_num: i8::try_from(log_hotbar).context("hotbar slot fits i8")?,
            container_input: ContainerInput::Swap,
            require_update: true,
            require_full_content: false,
        },
        |state| {
            state.items[0].item_id == oak_planks_item_id
                && state.items[0].count == 4
                && state.items[1].item_id == oak_log_item_id
                && state.items[1].count == 1
                && state.items[log_menu_slot].is_empty()
        },
    )
    .await
    .context("crafting swap/grid predicate")?;
    anyhow::ensure!(
        crafting.items[0].item_id == oak_planks_item_id
            && crafting.items[0].count == 4
            && crafting.items[1].item_id == oak_log_item_id
            && crafting.items[1].count == 1
            && crafting.items[log_menu_slot].is_empty(),
        "crafting grid preparation did not expose the oak-planks result"
    );
    let crafting_prepared = crafting.clone();
    phases.push(oracle_phase(manifest, 6, &crafting)?);

    let crafting_state_id = crafting.state_id;
    oracle_click_menu_until(
        &mut client,
        &mut crafting,
        OracleMenuClick {
            state_id: crafting_state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            require_update: true,
            require_full_content: false,
        },
        |state| {
            state.items[0].is_empty()
                && state.items[1].is_empty()
                && state.carried.is_empty()
                && state
                    .items
                    .iter()
                    .skip(10)
                    .any(|stack| stack.item_id == oak_planks_item_id && stack.count == 4)
        },
    )
    .await
    .context("crafting result quick-move predicate")?;
    anyhow::ensure!(
        crafting.items[0].is_empty() && crafting.items[1].is_empty() && crafting.carried.is_empty(),
        "crafting result quick-move left output, input, or cursor state"
    );
    let crafting_after_quick_move = crafting.clone();
    phases.push(oracle_phase(manifest, 7, &crafting)?);

    let expected_crafting_after_quick_move = crafting_after_quick_move.clone();
    oracle_click_menu_until(
        &mut client,
        &mut crafting,
        OracleMenuClick {
            state_id: crafting_prepared.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            require_update: false,
            require_full_content: false,
        },
        move |state| state.contents_equal(&expected_crafting_after_quick_move),
    )
    .await
    .context("crafting stale-click full resync predicate")?;
    anyhow::ensure!(
        crafting.contents_equal(&crafting_after_quick_move),
        "stale crafting click mutated slots or cursor instead of resyncing"
    );
    let mut crafting_stale_phase = oracle_phase(manifest, 8, &crafting)?;
    crafting_stale_phase.snapshot.state_id_delta = phases
        .last()
        .expect("crafting quick-move phase exists")
        .snapshot
        .state_id_delta;
    phases.push(crafting_stale_phase);

    oracle_close_menu(&mut client, crafting.container_id).await?;
    let crafting_reopen = oracle_open_menu(
        &mut client,
        table_target,
        26,
        12,
        ContainerStateOracleMenu::CraftingTable,
    )
    .await
    .context("reopen crafting table after stale click")?;
    anyhow::ensure!(
        crafting_reopen.items[0].is_empty()
            && crafting_reopen.items[1..10].iter().all(ItemStack::is_empty)
            && crafting_reopen
                .items
                .iter()
                .skip(10)
                .any(|stack| { stack.item_id == oak_planks_item_id && stack.count == 4 })
            && crafting_reopen.carried.is_empty(),
        "crafting close/reopen did not conserve result and empty grid/cursor"
    );
    phases.push(oracle_phase(manifest, 9, &crafting_reopen)?);

    let trace = ContainerStateOracleTrace {
        manifest_id: manifest.id.clone(),
        phases,
    };
    trace.validate_against(manifest)?;
    Ok(trace)
}

fn oracle_feedback_fixture(translation: &str) -> Vec<u8> {
    let mut bytes = bytes::BytesMut::new();
    mc_nbt::write_network(
        &mut bytes,
        &Tag::Compound(vec![(
            "translate".to_string(),
            Tag::String(translation.to_string()),
        )]),
    )
    .expect("encode command feedback fixture");
    bytes.to_vec()
}

#[test]
fn oracle_command_fence_distinguishes_unknown_feedback_from_unrelated_chat() {
    let fence = oracle_feedback_fixture("command.unknown.command");
    let give = oracle_feedback_fixture("commands.give.success.single");
    assert!(oracle_is_command_fence_feedback(&fence).unwrap());
    assert!(!oracle_is_command_fence_feedback(&give).unwrap());
}

#[test]
fn container_oracle_maps_window_zero_player_slots_into_open_menu_projection() {
    assert_eq!(
        oracle_menu_slot_for_player_inventory(ContainerStateOracleMenu::Chest, 9),
        Some(27)
    );
    assert_eq!(
        oracle_menu_slot_for_player_inventory(ContainerStateOracleMenu::Chest, 36),
        Some(54)
    );
    assert_eq!(
        oracle_menu_slot_for_player_inventory(ContainerStateOracleMenu::CraftingTable, 9),
        Some(10)
    );
    assert_eq!(
        oracle_menu_slot_for_player_inventory(ContainerStateOracleMenu::CraftingTable, 36),
        Some(37)
    );
    assert_eq!(
        oracle_menu_slot_for_player_inventory(ContainerStateOracleMenu::Chest, 5),
        None
    );
}

#[tokio::test]
#[ignore = "requires local .analysis/server.jar, Java 25, and data/vanilla sidecars"]
async fn checked_container_state_manifest_matches_vanilla_oracle_and_solaris() {
    let availability = vanilla_oracle_availability(repo_root());
    let OracleAvailability::Available { jar } = availability else {
        panic!(
            "{}",
            availability.skip_message().expect("oracle unavailable")
        );
    };
    let manifest = container_state_oracle_manifest();
    let vanilla_dir = tempfile::tempdir().expect("vanilla container oracle tempdir");
    std::fs::write(
        vanilla_dir.path().join("ops.json"),
        serde_json::to_vec_pretty(&serde_json::json!([{
            "uuid": mc_net::offline_uuid("vanilla").to_string(),
            "name": "vanilla",
            "level": 4,
            "bypassesPlayerLimit": false
        }]))
        .expect("serialize vanilla container oracle ops.json"),
    )
    .expect("write vanilla container oracle ops.json");
    let vanilla = VanillaServerProcess::launch(&jar, vanilla_dir.path(), Duration::from_secs(90))
        .expect("vanilla container oracle starts");

    let (solaris, solaris_addr) = spawn_solaris_container_oracle()
        .await
        .expect("spawn Solaris container oracle");
    let solaris_task = tokio::spawn(async move { solaris.serve().await });

    let vanilla_trace = observe_container_state_oracle(
        ScenarioContext {
            kind: ServerKind::Vanilla,
            addr: vanilla.addr(),
        },
        &manifest,
    )
    .await
    .expect("vanilla container state trace");
    let solaris_trace = observe_container_state_oracle(
        ScenarioContext {
            kind: ServerKind::Solaris,
            addr: solaris_addr,
        },
        &manifest,
    )
    .await
    .expect("Solaris container state trace");

    if vanilla_trace != solaris_trace {
        let differences = vanilla_trace
            .phases
            .iter()
            .zip(&solaris_trace.phases)
            .enumerate()
            .filter_map(|(index, (vanilla_phase, solaris_phase))| {
                if vanilla_phase == solaris_phase {
                    return None;
                }
                let slot_differences = vanilla_phase
                    .snapshot
                    .slots
                    .iter()
                    .zip(&solaris_phase.snapshot.slots)
                    .filter_map(|(vanilla, solaris)| {
                        (vanilla != solaris).then_some(format!(
                            "slot{} {:?}/{:?}",
                            vanilla.slot, vanilla.stack, solaris.stack
                        ))
                    })
                    .collect::<Vec<_>>();
                Some(format!(
                    "{index}:{} delta {}/{} cursor {:?}/{:?} slots=[{}]",
                    vanilla_phase.id,
                    vanilla_phase.snapshot.state_id_delta,
                    solaris_phase.snapshot.state_id_delta,
                    vanilla_phase.snapshot.cursor,
                    solaris_phase.snapshot.cursor,
                    slot_differences.join(", "),
                ))
            })
            .collect::<Vec<_>>();
        panic!("T01-06 trace differences: {}", differences.join("; "));
    }

    let artifact_dir = repo_root().join(".analysis/runs/t01-06-a");
    std::fs::create_dir_all(&artifact_dir).expect("create T01-06 artifact dir");
    std::fs::write(
        artifact_dir.join("container-state-oracle.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": CONTAINER_STATE_ORACLE_SCHEMA,
            "manifest": manifest,
            "vanilla": vanilla_trace,
            "solaris": solaris_trace,
        }))
        .expect("serialize T01-06 oracle artifact"),
    )
    .expect("write T01-06 oracle artifact");

    vanilla.stop().expect("vanilla container oracle stops");
    solaris_task.abort();
}
