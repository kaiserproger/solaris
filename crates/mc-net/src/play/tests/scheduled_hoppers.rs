use super::{
    BlockReport, BlockStateId, CAMPFIRE_BLOCK_ENTITY_TYPE_ID, Chunk, ChunkPos, Compression,
    FURNACE_CONTAINER_ID_MIN, FurnaceSlot, GameMode, HOPPER_TICK_DELAY_TICKS,
    HOPPER_TRANSFER_DELAY_TICKS, HOPPER_TRANSFER_MAX_STACK, HopperTransferContext, Identifier,
    InteractionState, ItemFactsTable, ItemRegistry, ItemReport, ItemStack, ItemToBlockTable,
    LightCache, LightWorkspace, ListTag, LoggedInProfile, OutboundCommand, PlayerInventory,
    PlayerPersistedState, PlayerPose, QuickCraftState, RESIDENT_HOPPER_TRANSFER_COMMIT_COUNT,
    SIMULATION_COMMAND_BATCH_LIMIT, ScheduledBlockTickReport, ServerConfig, SessionRegistry,
    SimulationWorldAccess, Tag, TagsData, container_redstone_signal_at,
    dispatch_and_clear_setup_packets, handle_block_item_placement,
    insert_hopper_stack_into_campfire, pack_block_pos, play_loop_slow_client_test_config,
    prop_schema, register_interaction_player, register_loaded_button_session,
    run_scheduled_block_ticks, scheduled_hopper_transfer, simple_block, simulation_channel, state,
    test_use_item_on,
};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
async fn run_scheduled_block_ticks_for_range(
    config: &ServerConfig,
    sessions: &SessionRegistry,
    start: u64,
    end: u64,
) -> ScheduledBlockTickReport {
    let mut report = ScheduledBlockTickReport::default();
    for tick in start..=end {
        report = run_scheduled_block_ticks(config, sessions, tick).await;
    }
    report
}

#[tokio::test]
async fn scheduled_hopper_tick_pulls_one_item_into_hopper_before_ejecting_without_generating_neighbors()
 {
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

    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
        ])
        .unwrap(),
    );
    let generated_chunks = Arc::new(AtomicUsize::new(0));
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks)).with_generator(
        Arc::new(CountingAirGenerator {
            calls: Arc::clone(&generated_chunks),
        }),
    );
    storage
        .insert_generated_chunk(
            ChunkPos { x: 0, z: 0 },
            Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
        )
        .unwrap();
    let source_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let target_pos = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    storage.set_block_at(source_pos, BlockStateId(1)).unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(target_pos, BlockStateId(1)).unwrap();
    let mut source = mc_world::ChestBlockEntity::default();
    source.slots[0] = mc_world::FurnaceSlot {
        count: 2,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };
    storage.set_chest_block_entity(source_pos, source).unwrap();
    storage
        .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
        .unwrap();
    storage
        .set_chest_block_entity(target_pos, mc_world::ChestBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        items: Arc::new(ItemRegistry::from_report(&[ItemReport {
            id: Identifier::parse("minecraft:apple").unwrap(),
            protocol_id: 42,
        }])),
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let source_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(42),
        name: "HopperSourceViewer".to_string(),
    };
    let target_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(43),
        name: "HopperTargetViewer".to_string(),
    };
    let (source_tx, mut source_rx) = mpsc::channel(16);
    let (source_session_id, _) = sessions.register(
        &source_profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        source_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let (target_tx, mut target_rx) = mpsc::channel(16);
    let (target_session_id, _) = sessions.register(
        &target_profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        target_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let mut setup_dispatches = sessions.mark_loaded(source_session_id, (0, 0));
    setup_dispatches.extend(sessions.mark_loaded(target_session_id, (0, 0)));
    dispatch_and_clear_setup_packets(setup_dispatches, &mut [&mut source_rx, &mut target_rx]);
    assert_eq!(
        sessions.register_chest_viewer(source_session_id, source_pos),
        1
    );
    assert_eq!(
        sessions.register_chest_viewer(target_session_id, target_pos),
        1
    );
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);

    let storage = world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let (_simulation, owner) = simulation_channel();
    let world_writer = world.lock().await;
    let report = tokio::time::timeout(
        Duration::from_secs(5),
        owner.run_scheduled_block_ticks_with_budget(
            &config,
            &sessions,
            SimulationWorldAccess {
                read: Some(&world_read),
                mutation: Some(&world_mutation),
                ..SimulationWorldAccess::default()
            },
            20,
            1,
        ),
    )
    .await
    .expect("resident hopper transfer journal completion event");
    drop(world_writer);

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    {
        let mut storage = world.lock().await;
        assert_eq!(generated_chunks.load(Ordering::Relaxed), 0);
        assert_eq!(storage.cache_len(), 1);
        let source = storage
            .chest_block_entity(source_pos)
            .unwrap()
            .expect("source chest");
        let hopper = storage
            .hopper_block_entity(hopper_pos)
            .unwrap()
            .expect("hopper");
        let target = storage
            .chest_block_entity(target_pos)
            .unwrap()
            .expect("target chest");
        assert_eq!(source.slots[0].count, 1);
        assert_eq!(
            hopper.slots[0],
            mc_world::FurnaceSlot {
                count: 1,
                item_id: 42,
                damage: None,
                enchantments: Vec::new(),
            }
        );
        assert!(
            hopper.slots[1..]
                .iter()
                .all(mc_world::FurnaceSlot::is_empty)
        );
        assert!(target.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        let scheduled = storage
            .scheduled_block_ticks(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .expect("chunk scheduled ticks");
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].pos, hopper_pos);
        assert_eq!(scheduled[0].trigger_tick, 21);
        assert_eq!(
            scheduled[0].block,
            Identifier::parse("minecraft:hopper").unwrap()
        );
    }
    match source_rx
        .try_recv()
        .expect("source viewer receives chest slots")
    {
        OutboundCommand::ChestSlots {
            position,
            state_id,
            slots,
        } => {
            assert_eq!(position, source_pos);
            assert_eq!(state_id, 2);
            assert_eq!(slots[0], ItemStack::new(42, 1));
        }
        other => panic!("unexpected outbound command: {other:?}"),
    }
    assert!(target_rx.try_recv().is_err());

    let (reopened, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    let restored = reopened.decode_pending(&pending).unwrap();
    assert_eq!(restored.len(), 1);
    let source = restored[0]
        .chests
        .get(&source_pos)
        .expect("journaled source chest");
    let hopper = restored[0]
        .hoppers
        .get(&hopper_pos)
        .expect("journaled hopper");
    assert_eq!(source.slots[0].count, 1);
    assert_eq!(hopper.slots[0].item_id, 42);
    assert_eq!(hopper.slots[0].count, 1);
}

#[tokio::test]
async fn scheduled_hopper_ejection_schedules_comparator_tick_for_target_chest() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
            BlockReport {
                id: Identifier::parse("minecraft:comparator").unwrap(),
                properties: prop_schema(&[
                    ("facing", &["west"]),
                    ("mode", &["compare"]),
                    ("powered", &["false", "true"]),
                ]),
                states: vec![
                    state(
                        3,
                        true,
                        &[
                            ("facing", "west"),
                            ("mode", "compare"),
                            ("powered", "false"),
                        ],
                    ),
                    state(
                        4,
                        false,
                        &[("facing", "west"), ("mode", "compare"), ("powered", "true")],
                    ),
                ],
            },
        ])
        .unwrap(),
    );
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            ChunkPos { x: 0, z: 0 },
            Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
        )
        .unwrap();
    let hopper_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let target_pos = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let comparator_pos = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(target_pos, BlockStateId(1)).unwrap();
    storage
        .set_block_at(comparator_pos, BlockStateId(3))
        .unwrap();
    let mut hopper = mc_world::HopperBlockEntity::default();
    hopper.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };
    storage.set_hopper_block_entity(hopper_pos, hopper).unwrap();
    storage
        .set_chest_block_entity(target_pos, mc_world::ChestBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "HopperComparator");
    RESIDENT_HOPPER_TRANSFER_COMMIT_COUNT.with(|count| count.set(0));

    let first = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(first.drained, 1);
    assert_eq!(first.applied, 1);
    RESIDENT_HOPPER_TRANSFER_COMMIT_COUNT.with(|count| {
        assert_eq!(
            count.get(),
            1,
            "same-region hopper transfer uses resident CAS"
        )
    });
    {
        let mut storage = world.lock().await;
        let target = storage
            .chest_block_entity(target_pos)
            .unwrap()
            .expect("target chest");
        assert_eq!(
            target.slots[0],
            mc_world::FurnaceSlot {
                count: 1,
                item_id: 42,
                damage: None,
                enchantments: Vec::new(),
            }
        );
        assert_eq!(
            storage.get_cached_block(comparator_pos),
            Some(BlockStateId(3))
        );
        let scheduled = storage
            .scheduled_block_ticks(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .expect("chunk scheduled ticks");
        assert!(
            scheduled.iter().any(|tick| {
                tick.pos == comparator_pos
                    && tick.block == Identifier::parse("minecraft:comparator").unwrap()
                    && tick.trigger_tick == 22
            }),
            "hopper target mutation should schedule a delayed comparator refresh"
        );
    }

    let final_report = run_scheduled_block_ticks_for_range(&config, &sessions, 21, 22).await;

    assert_eq!(final_report.applied, 1);
    let storage = world.lock().await;
    assert_eq!(
        storage.get_cached_block(comparator_pos),
        Some(BlockStateId(4))
    );
}

#[tokio::test]
async fn scheduled_hopper_transfer_across_region_boundary_uses_atomic_resident_commit() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
        ])
        .unwrap(),
    );
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    for chunk_x in [7, 8] {
        let position = ChunkPos { x: chunk_x, z: 0 };
        storage
            .insert_generated_chunk(
                position,
                Chunk::empty(position, BlockStateId(0), biome.clone()),
            )
            .unwrap();
    }
    let hopper_pos = mc_world::BlockPos {
        x: 127,
        y: 64,
        z: 1,
    };
    let target_pos = mc_world::BlockPos {
        x: 128,
        y: 64,
        z: 1,
    };
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(target_pos, BlockStateId(1)).unwrap();
    let mut hopper = mc_world::HopperBlockEntity::default();
    hopper.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };
    storage.set_hopper_block_entity(hopper_pos, hopper).unwrap();
    storage
        .set_chest_block_entity(target_pos, mc_world::ChestBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(44),
        name: "CrossRegionHopper".to_string(),
    };
    let (tx, _rx) = mpsc::channel(16);
    let loaded = HashSet::from([(7, 0), (8, 0)]);
    let (session_id, _) = sessions.register(
        &profile,
        (7, 0),
        0,
        loaded.clone(),
        tx,
        PlayerPose::new(127.5, 64.0, 1.5),
    );
    for chunk in loaded {
        let _ = sessions.mark_loaded(session_id, chunk);
    }
    RESIDENT_HOPPER_TRANSFER_COMMIT_COUNT.with(|count| count.set(0));

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    RESIDENT_HOPPER_TRANSFER_COMMIT_COUNT.with(|count| {
        assert_eq!(
            count.get(),
            1,
            "cross-region hopper transfer must use one atomic resident commit"
        )
    });
    let mut storage = world.lock().await;
    let hopper = storage
        .hopper_block_entity(hopper_pos)
        .unwrap()
        .expect("hopper");
    let target = storage
        .chest_block_entity(target_pos)
        .unwrap()
        .expect("target chest");
    assert!(hopper.slots.iter().all(mc_world::FurnaceSlot::is_empty));
    assert_eq!(target.slots[0].count, 1);
    assert_eq!(target.slots[0].item_id, 42);
}

#[test]
fn comparator_container_signal_uses_vanilla_discrete_fullness_formula() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
        ])
        .unwrap(),
    );
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            ChunkPos { x: 0, z: 0 },
            Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
        )
        .unwrap();
    let chest_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    storage.set_block_at(chest_pos, BlockStateId(1)).unwrap();
    storage
        .set_chest_block_entity(chest_pos, mc_world::ChestBlockEntity::default())
        .unwrap();
    assert_eq!(
        container_redstone_signal_at(&blocks, &mut storage, chest_pos),
        0
    );

    let mut chest = mc_world::ChestBlockEntity::default();
    chest.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };
    storage.set_chest_block_entity(chest_pos, chest).unwrap();
    assert_eq!(
        container_redstone_signal_at(&blocks, &mut storage, chest_pos),
        1
    );

    let mut chest = mc_world::ChestBlockEntity::default();
    for slot in &mut chest.slots {
        *slot = mc_world::FurnaceSlot {
            count: HOPPER_TRANSFER_MAX_STACK,
            item_id: 42,
            damage: None,
            enchantments: Vec::new(),
        };
    }
    storage.set_chest_block_entity(chest_pos, chest).unwrap();
    assert_eq!(
        container_redstone_signal_at(&blocks, &mut storage, chest_pos),
        15
    );
}

#[tokio::test]
async fn placing_hopper_schedules_initial_transfer_tick() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:dirt"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["down"])]),
                states: vec![state(2, true, &[("facing", "down")])],
            },
        ])
        .unwrap(),
    );
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:hopper").unwrap(),
        protocol_id: 42,
    }]));
    let storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let world_read = storage.read_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let item_to_block = ItemToBlockTable::build(&items, &blocks);
    let mut state = InteractionState {
        world: Arc::clone(&world),
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
    };
    *state.inventory.held_mut(0).unwrap() = ItemStack::new(42, 1);
    let session_id = register_interaction_player(&mut state, "HopperPlacementBuilder");
    let (simulation, mut simulation_owner) = simulation_channel();
    state.simulation = simulation.for_session(session_id);
    let owner_sessions = Arc::clone(&state.sessions);
    let owner_world = Arc::clone(&world);
    let (owner_stop_tx, mut owner_stop_rx) = tokio::sync::oneshot::channel();
    let owner_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = &mut owner_stop_rx => {
                    simulation_owner.shutdown();
                    break;
                }
                ready = simulation_owner.wait_for_command() => {
                    if !ready {
                        break;
                    }
                    simulation_owner.process_tick_with_world(
                        &owner_sessions,
                        Some(&owner_world),
                        None,
                        SIMULATION_COMMAND_BATCH_LIMIT,
                    );
                }
            }
        }
    });
    let cpos = ChunkPos { x: 0, z: 0 };
    let clicked_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let target_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    {
        let mut storage = world.lock().await;
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
        storage.set_block_at(clicked_pos, BlockStateId(1)).unwrap();
    }
    let action = test_use_item_on(pack_block_pos(clicked_pos.x, clicked_pos.y, clicked_pos.z));
    let mut writer = tokio::io::sink();

    handle_block_item_placement(
        &mut state,
        &mut writer,
        None,
        GameMode::Survival,
        PlayerPose::new(1.5, 64.0, 1.5),
        clicked_pos,
        &action,
        (clicked_pos.x, clicked_pos.y, clicked_pos.z),
    )
    .await
    .unwrap();
    let _ = owner_stop_tx.send(());
    owner_task.await.unwrap();

    let mut storage = world.lock().await;
    assert_eq!(storage.get_cached_block(target_pos), Some(BlockStateId(2)));
    let scheduled = storage
        .scheduled_block_ticks(cpos)
        .unwrap()
        .expect("chunk scheduled ticks");
    assert_eq!(scheduled.len(), 1);
    assert_eq!(scheduled[0].pos, target_pos);
    assert_eq!(scheduled[0].trigger_tick, HOPPER_TICK_DELAY_TICKS);
    assert_eq!(
        scheduled[0].block,
        Identifier::parse("minecraft:hopper").unwrap()
    );
}

#[tokio::test]
async fn scheduled_block_pass_backfills_loaded_hopper_missing_initial_tick() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
        ])
        .unwrap(),
    );
    let cpos = ChunkPos { x: 0, z: 0 };
    let source_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let target_pos = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
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
    storage.set_block_at(source_pos, BlockStateId(1)).unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(target_pos, BlockStateId(1)).unwrap();
    let mut source = mc_world::ChestBlockEntity::default();
    source.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };
    storage.set_chest_block_entity(source_pos, source).unwrap();
    storage
        .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
        .unwrap();
    storage
        .set_chest_block_entity(target_pos, mc_world::ChestBlockEntity::default())
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "BackfillHopper");

    let first = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(first.drained, 0);
    assert_eq!(first.applied, 0);
    {
        let mut storage = world.lock().await;
        let source = storage
            .chest_block_entity(source_pos)
            .unwrap()
            .expect("source chest");
        let target = storage
            .chest_block_entity(target_pos)
            .unwrap()
            .expect("target chest");
        assert_eq!(source.slots[0].count, 1);
        assert!(target.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        let scheduled = storage
            .scheduled_block_ticks(cpos)
            .unwrap()
            .expect("chunk scheduled ticks");
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].pos, hopper_pos);
        assert_eq!(scheduled[0].trigger_tick, 21);
        assert_eq!(
            scheduled[0].block,
            Identifier::parse("minecraft:hopper").unwrap()
        );
    }

    let second = run_scheduled_block_ticks(&config, &sessions, 21).await;

    assert_eq!(second.drained, 1);
    assert_eq!(second.applied, 1);
    let mut storage = world.lock().await;
    let source = storage
        .chest_block_entity(source_pos)
        .unwrap()
        .expect("source chest");
    let target = storage
        .chest_block_entity(target_pos)
        .unwrap()
        .expect("target chest");
    assert!(source.slots.iter().all(mc_world::FurnaceSlot::is_empty));
    let hopper = storage
        .hopper_block_entity(hopper_pos)
        .unwrap()
        .expect("hopper");
    assert_eq!(
        hopper.slots[0],
        mc_world::FurnaceSlot {
            count: 1,
            item_id: 42,
            damage: None,
            enchantments: Vec::new(),
        }
    );
    assert!(target.slots.iter().all(mc_world::FurnaceSlot::is_empty));
}

#[tokio::test]
async fn scheduled_block_pass_does_not_duplicate_existing_hopper_tick() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["down"])]),
                states: vec![state(1, true, &[("facing", "down")])],
            },
        ])
        .unwrap(),
    );
    let cpos = ChunkPos { x: 0, z: 0 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
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
    storage.set_block_at(hopper_pos, BlockStateId(1)).unwrap();
    storage
        .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            40,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "ExistingHopperTick");

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 0);
    assert_eq!(report.applied, 0);
    let mut storage = world.lock().await;
    let scheduled = storage
        .scheduled_block_ticks(cpos)
        .unwrap()
        .expect("chunk scheduled ticks");
    assert_eq!(scheduled.len(), 1);
    assert_eq!(scheduled[0].pos, hopper_pos);
    assert_eq!(scheduled[0].trigger_tick, 40);
    assert_eq!(
        scheduled[0].block,
        Identifier::parse("minecraft:hopper").unwrap()
    );
}

#[tokio::test]
async fn scheduled_hopper_cooldown_tick_uses_resident_commit() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["down"])]),
                states: vec![state(1, true, &[("facing", "down")])],
            },
        ])
        .unwrap(),
    );
    let cpos = ChunkPos { x: 0, z: 0 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
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
    storage.set_block_at(hopper_pos, BlockStateId(1)).unwrap();
    storage
        .set_hopper_block_entity(
            hopper_pos,
            mc_world::HopperBlockEntity {
                transfer_cooldown: 8,
                ..mc_world::HopperBlockEntity::default()
            },
        )
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "ResidentHopperCooldown");
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);

    let storage = world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let (_simulation, owner) = simulation_channel();
    let world_writer = world.lock().await;
    let report = tokio::time::timeout(
        Duration::from_secs(5),
        owner.run_scheduled_block_ticks_with_budget(
            &config,
            &sessions,
            SimulationWorldAccess {
                read: Some(&world_read),
                mutation: Some(&world_mutation),
                ..SimulationWorldAccess::default()
            },
            20,
            1,
        ),
    )
    .await
    .expect("resident hopper journal completion event");
    drop(world_writer);

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 0);
    let mut storage = world.lock().await;
    assert_eq!(
        storage
            .hopper_block_entity(hopper_pos)
            .unwrap()
            .unwrap()
            .transfer_cooldown,
        7
    );
    let scheduled = storage
        .scheduled_block_ticks(cpos)
        .unwrap()
        .expect("hopper cooldown schedules its next tick");
    assert_eq!(scheduled.len(), 1);
    assert_eq!(scheduled[0].pos, hopper_pos);
    assert_eq!(scheduled[0].trigger_tick, 21);

    let (reopened, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    let restored = reopened.decode_pending(&pending).unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(
        restored[0]
            .hoppers
            .get(&hopper_pos)
            .expect("journaled hopper")
            .transfer_cooldown,
        7
    );
    assert_eq!(restored[0].scheduled_block_ticks().len(), 1);
    assert_eq!(restored[0].scheduled_block_ticks()[0].trigger_tick, 1);
}

#[tokio::test]
async fn scheduled_hopper_cooldowns_share_one_wal_decision() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["down"])]),
                states: vec![state(1, true, &[("facing", "down")])],
            },
        ])
        .unwrap(),
    );
    let cpos = ChunkPos { x: 0, z: 0 };
    let positions = [
        mc_world::BlockPos { x: 1, y: 64, z: 1 },
        mc_world::BlockPos { x: 2, y: 64, z: 1 },
    ];
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
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
    for position in positions {
        storage.set_block_at(position, BlockStateId(1)).unwrap();
        storage
            .set_hopper_block_entity(
                position,
                mc_world::HopperBlockEntity {
                    transfer_cooldown: 8,
                    ..mc_world::HopperBlockEntity::default()
                },
            )
            .unwrap();
        storage
            .schedule_block_tick(mc_world::ScheduledBlockTick::new(
                position,
                Identifier::parse("minecraft:hopper").unwrap(),
                20,
                0,
            ))
            .unwrap();
    }
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "GroupedHopperCooldowns");
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 2);
    let (reopened, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert_eq!(pending.len(), 1, "one hopper pass uses one WAL decision");
    let restored = reopened.decode_pending(&pending).unwrap();
    assert_eq!(restored.len(), 1);
    assert!(positions.iter().all(|position| {
        restored[0]
            .hoppers
            .get(position)
            .is_some_and(|hopper| hopper.transfer_cooldown == 7)
    }));
}

#[test]
fn scheduled_hopper_container_dispatch_does_not_hold_world_writer() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
        ])
        .unwrap(),
    );
    let cpos = ChunkPos { x: 0, z: 0 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let target_pos = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
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
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(target_pos, BlockStateId(1)).unwrap();
    let mut hopper = mc_world::HopperBlockEntity::default();
    hopper.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };
    storage.set_hopper_block_entity(hopper_pos, hopper).unwrap();
    storage
        .set_chest_block_entity(target_pos, mc_world::ChestBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = Arc::new(SessionRegistry::new());
    register_loaded_button_session(&sessions, "HopperWorldLock");
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    sessions.install_server_container_dispatch_probe(reached_tx, resume_rx);

    let tick_sessions = Arc::clone(&sessions);
    let tick_thread = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(run_scheduled_block_ticks(&config, &tick_sessions, 20))
    });
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("scheduled hopper reaches container dispatch");
    let writer_available = world.try_lock().is_ok();
    resume_tx.send(()).expect("release container dispatch");
    let report = tick_thread.join().expect("scheduled hopper tick joins");

    assert!(
        writer_available,
        "container dispatch must run after releasing the world writer"
    );
    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
}

#[tokio::test]
async fn scheduled_hopper_tick_feeds_valid_input_into_furnace_below() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let iron_ore = Identifier::parse("minecraft:iron_ore").unwrap();
    let iron_ingot = Identifier::parse("minecraft:iron_ingot").unwrap();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["down"])]),
                states: vec![state(2, true, &[("facing", "down")])],
            },
            simple_block(3, "minecraft:furnace"),
        ])
        .unwrap(),
    );
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: iron_ore.clone(),
            protocol_id: 42,
        },
        ItemReport {
            id: iron_ingot.clone(),
            protocol_id: 43,
        },
    ]));
    let recipes = Arc::new(vec![Recipe {
        id: Identifier::parse("minecraft:test_iron_ore").unwrap(),
        kind: RecipeKind::Smelting(SmeltingRecipe {
            ingredient: Ingredient {
                alternatives: vec![IngredientAlternative::Item(iron_ore)],
            },
            cooking_time: 200,
            experience_milli: 0,
        }),
        result: RecipeResult {
            item: iron_ingot,
            count: 1,
        },
    }]);
    let cpos = ChunkPos { x: 0, z: 0 };
    let source_pos = mc_world::BlockPos { x: 1, y: 66, z: 1 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let furnace_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
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
    storage.set_block_at(source_pos, BlockStateId(1)).unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(furnace_pos, BlockStateId(3)).unwrap();
    let mut source = mc_world::ChestBlockEntity::default();
    source.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };
    storage.set_chest_block_entity(source_pos, source).unwrap();
    storage
        .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
        .unwrap();
    storage
        .set_furnace_block_entity(furnace_pos, mc_world::FurnaceBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        items,
        tags: Arc::new(TagsData::default()),
        recipes,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let source_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(44),
        name: "HopperFurnaceSourceViewer".to_string(),
    };
    let furnace_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(45),
        name: "HopperFurnaceViewer".to_string(),
    };
    let (source_tx, mut source_rx) = mpsc::channel(16);
    let (source_session_id, _) = sessions.register(
        &source_profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        source_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let (furnace_tx, mut furnace_rx) = mpsc::channel(16);
    let (furnace_session_id, _) = sessions.register(
        &furnace_profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        furnace_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let mut setup_dispatches = sessions.mark_loaded(source_session_id, (0, 0));
    setup_dispatches.extend(sessions.mark_loaded(furnace_session_id, (0, 0)));
    dispatch_and_clear_setup_packets(setup_dispatches, &mut [&mut source_rx, &mut furnace_rx]);
    assert_eq!(
        sessions.register_chest_viewer(source_session_id, source_pos),
        1
    );
    assert_eq!(
        sessions.register_furnace_viewer(furnace_session_id, furnace_pos),
        1
    );

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    let final_report = run_scheduled_block_ticks_for_range(&config, &sessions, 21, 28).await;
    assert_eq!(final_report.drained, 1);
    assert_eq!(final_report.applied, 1);
    {
        let mut storage = world.lock().await;
        let source = storage
            .chest_block_entity(source_pos)
            .unwrap()
            .expect("source chest");
        let hopper = storage
            .hopper_block_entity(hopper_pos)
            .unwrap()
            .expect("hopper");
        let furnace = storage
            .furnace_block_entity(furnace_pos)
            .unwrap()
            .expect("furnace");
        assert!(source.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        assert!(hopper.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        assert_eq!(
            furnace.slots[0],
            mc_world::FurnaceSlot {
                count: 1,
                item_id: 42,
                damage: None,
                enchantments: Vec::new(),
            }
        );
        assert!(furnace.slots[1].is_empty());
        assert!(furnace.slots[2].is_empty());
        let scheduled = storage
            .scheduled_block_ticks(cpos)
            .unwrap()
            .expect("chunk scheduled ticks");
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].pos, hopper_pos);
        assert_eq!(scheduled[0].trigger_tick, 29);
    }
    match source_rx
        .try_recv()
        .expect("source viewer receives chest slots")
    {
        OutboundCommand::ChestSlots {
            position,
            state_id,
            slots,
        } => {
            assert_eq!(position, source_pos);
            assert_eq!(state_id, 2);
            assert_eq!(slots[0], ItemStack::EMPTY);
        }
        other => panic!("unexpected outbound command: {other:?}"),
    }
    match furnace_rx
        .try_recv()
        .expect("furnace viewer receives furnace slots")
    {
        OutboundCommand::FurnaceSlots {
            position,
            state_id,
            slots,
        } => {
            assert_eq!(position, furnace_pos);
            assert_eq!(state_id, 2);
            assert_eq!(slots[0], ItemStack::new(42, 1));
            assert_eq!(slots[1], ItemStack::EMPTY);
            assert_eq!(slots[2], ItemStack::EMPTY);
        }
        other => panic!("unexpected outbound command: {other:?}"),
    }
}

#[tokio::test]
async fn scheduled_hopper_tick_feeds_side_fuel_into_furnace() {
    let oak_stairs = Identifier::parse("minecraft:oak_stairs").unwrap();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
            simple_block(3, "minecraft:furnace"),
        ])
        .unwrap(),
    );
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: oak_stairs,
        protocol_id: 44,
    }]));
    let recipes: Arc<Vec<mc_data::recipes::Recipe>> = Arc::new(Vec::new());
    let cpos = ChunkPos { x: 0, z: 0 };
    let source_pos = mc_world::BlockPos { x: 1, y: 66, z: 1 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let furnace_pos = mc_world::BlockPos { x: 2, y: 65, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
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
    storage.set_block_at(source_pos, BlockStateId(1)).unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(furnace_pos, BlockStateId(3)).unwrap();
    let mut source = mc_world::ChestBlockEntity::default();
    source.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 44,
        damage: None,
        enchantments: Vec::new(),
    };
    storage.set_chest_block_entity(source_pos, source).unwrap();
    storage
        .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
        .unwrap();
    storage
        .set_furnace_block_entity(furnace_pos, mc_world::FurnaceBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let tags = mc_data::tags::solaris_required_item_tags(&items);
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        items,
        tags: Arc::new(tags),
        recipes,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let source_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(46),
        name: "HopperFuelSourceViewer".to_string(),
    };
    let furnace_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(47),
        name: "HopperFuelViewer".to_string(),
    };
    let (source_tx, mut source_rx) = mpsc::channel(16);
    let (source_session_id, _) = sessions.register(
        &source_profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        source_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let (furnace_tx, mut furnace_rx) = mpsc::channel(16);
    let (furnace_session_id, _) = sessions.register(
        &furnace_profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        furnace_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let mut setup_dispatches = sessions.mark_loaded(source_session_id, (0, 0));
    setup_dispatches.extend(sessions.mark_loaded(furnace_session_id, (0, 0)));
    dispatch_and_clear_setup_packets(setup_dispatches, &mut [&mut source_rx, &mut furnace_rx]);
    assert_eq!(
        sessions.register_chest_viewer(source_session_id, source_pos),
        1
    );
    assert_eq!(
        sessions.register_furnace_viewer(furnace_session_id, furnace_pos),
        1
    );

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    let final_report = run_scheduled_block_ticks_for_range(&config, &sessions, 21, 28).await;
    assert_eq!(final_report.drained, 1);
    assert_eq!(final_report.applied, 1);
    {
        let mut storage = world.lock().await;
        let source = storage
            .chest_block_entity(source_pos)
            .unwrap()
            .expect("source chest");
        let hopper = storage
            .hopper_block_entity(hopper_pos)
            .unwrap()
            .expect("hopper");
        let furnace = storage
            .furnace_block_entity(furnace_pos)
            .unwrap()
            .expect("furnace");
        assert!(source.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        assert!(hopper.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        assert!(furnace.slots[0].is_empty());
        assert_eq!(
            furnace.slots[1],
            mc_world::FurnaceSlot {
                count: 1,
                item_id: 44,
                damage: None,
                enchantments: Vec::new(),
            }
        );
        assert!(furnace.slots[2].is_empty());
        let scheduled = storage
            .scheduled_block_ticks(cpos)
            .unwrap()
            .expect("chunk scheduled ticks");
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].pos, hopper_pos);
        assert_eq!(scheduled[0].trigger_tick, 29);
    }
    match source_rx
        .try_recv()
        .expect("source viewer receives chest slots")
    {
        OutboundCommand::ChestSlots {
            position,
            state_id,
            slots,
        } => {
            assert_eq!(position, source_pos);
            assert_eq!(state_id, 2);
            assert_eq!(slots[0], ItemStack::EMPTY);
        }
        other => panic!("unexpected outbound command: {other:?}"),
    }
    match furnace_rx
        .try_recv()
        .expect("furnace viewer receives furnace slots")
    {
        OutboundCommand::FurnaceSlots {
            position,
            state_id,
            slots,
        } => {
            assert_eq!(position, furnace_pos);
            assert_eq!(state_id, 2);
            assert_eq!(slots[0], ItemStack::EMPTY);
            assert_eq!(slots[1], ItemStack::new(44, 1));
            assert_eq!(slots[2], ItemStack::EMPTY);
        }
        other => panic!("unexpected outbound command: {other:?}"),
    }
}

#[tokio::test]
async fn scheduled_hopper_tick_extracts_furnace_output_into_chest() {
    let iron_ingot = Identifier::parse("minecraft:iron_ingot").unwrap();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
            simple_block(3, "minecraft:furnace"),
        ])
        .unwrap(),
    );
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: iron_ingot,
        protocol_id: 43,
    }]));
    let recipes: Arc<Vec<mc_data::recipes::Recipe>> = Arc::new(Vec::new());
    let cpos = ChunkPos { x: 0, z: 0 };
    let furnace_pos = mc_world::BlockPos { x: 1, y: 66, z: 1 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let target_pos = mc_world::BlockPos { x: 2, y: 65, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
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
    storage.set_block_at(furnace_pos, BlockStateId(3)).unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(target_pos, BlockStateId(1)).unwrap();
    let mut furnace = mc_world::FurnaceBlockEntity::default();
    furnace.slots[2] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 43,
        damage: None,
        enchantments: Vec::new(),
    };
    storage
        .set_furnace_block_entity(furnace_pos, furnace)
        .unwrap();
    storage
        .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
        .unwrap();
    storage
        .set_chest_block_entity(target_pos, mc_world::ChestBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        items,
        tags: Arc::new(TagsData::default()),
        recipes,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let furnace_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(48),
        name: "HopperOutputFurnaceViewer".to_string(),
    };
    let target_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(49),
        name: "HopperOutputTargetViewer".to_string(),
    };
    let (furnace_tx, mut furnace_rx) = mpsc::channel(16);
    let (furnace_session_id, _) = sessions.register(
        &furnace_profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        furnace_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let (target_tx, mut target_rx) = mpsc::channel(16);
    let (target_session_id, _) = sessions.register(
        &target_profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        target_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let mut setup_dispatches = sessions.mark_loaded(furnace_session_id, (0, 0));
    setup_dispatches.extend(sessions.mark_loaded(target_session_id, (0, 0)));
    dispatch_and_clear_setup_packets(setup_dispatches, &mut [&mut furnace_rx, &mut target_rx]);
    assert_eq!(
        sessions.register_furnace_viewer(furnace_session_id, furnace_pos),
        1
    );
    assert_eq!(
        sessions.register_chest_viewer(target_session_id, target_pos),
        1
    );

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    let final_report = run_scheduled_block_ticks_for_range(&config, &sessions, 21, 28).await;
    assert_eq!(final_report.drained, 1);
    assert_eq!(final_report.applied, 1);
    {
        let mut storage = world.lock().await;
        let furnace = storage
            .furnace_block_entity(furnace_pos)
            .unwrap()
            .expect("furnace");
        let hopper = storage
            .hopper_block_entity(hopper_pos)
            .unwrap()
            .expect("hopper");
        let target = storage
            .chest_block_entity(target_pos)
            .unwrap()
            .expect("target chest");
        assert!(furnace.slots[0].is_empty());
        assert!(furnace.slots[1].is_empty());
        assert!(furnace.slots[2].is_empty());
        assert!(hopper.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        assert_eq!(
            target.slots[0],
            mc_world::FurnaceSlot {
                count: 1,
                item_id: 43,
                damage: None,
                enchantments: Vec::new(),
            }
        );
        let scheduled = storage
            .scheduled_block_ticks(cpos)
            .unwrap()
            .expect("chunk scheduled ticks");
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].pos, hopper_pos);
        assert_eq!(scheduled[0].trigger_tick, 29);
    }
    match furnace_rx
        .try_recv()
        .expect("furnace viewer receives furnace slots")
    {
        OutboundCommand::FurnaceSlots {
            position,
            state_id,
            slots,
        } => {
            assert_eq!(position, furnace_pos);
            assert_eq!(state_id, 2);
            assert_eq!(slots[0], ItemStack::EMPTY);
            assert_eq!(slots[1], ItemStack::EMPTY);
            assert_eq!(slots[2], ItemStack::EMPTY);
        }
        other => panic!("unexpected outbound command: {other:?}"),
    }
    match target_rx
        .try_recv()
        .expect("target viewer receives chest slots")
    {
        OutboundCommand::ChestSlots {
            position,
            state_id,
            slots,
        } => {
            assert_eq!(position, target_pos);
            assert_eq!(state_id, 2);
            assert_eq!(slots[0], ItemStack::new(43, 1));
        }
        other => panic!("unexpected outbound command: {other:?}"),
    }
}

#[tokio::test]
async fn scheduled_hopper_tick_feeds_campfire_cooking_slot() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let porkchop = Identifier::parse("minecraft:porkchop").unwrap();
    let cooked_porkchop = Identifier::parse("minecraft:cooked_porkchop").unwrap();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["down"])]),
                states: vec![state(2, true, &[("facing", "down")])],
            },
            BlockReport {
                id: Identifier::parse("minecraft:campfire").unwrap(),
                properties: prop_schema(&[("lit", &["true"])]),
                states: vec![state(3, true, &[("lit", "true")])],
            },
        ])
        .unwrap(),
    );
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: porkchop.clone(),
            protocol_id: 42,
        },
        ItemReport {
            id: cooked_porkchop.clone(),
            protocol_id: 43,
        },
    ]));
    let recipes = Arc::new(vec![Recipe {
        id: Identifier::parse("minecraft:test_campfire").unwrap(),
        kind: RecipeKind::CampfireCooking(SmeltingRecipe {
            ingredient: Ingredient {
                alternatives: vec![IngredientAlternative::Item(porkchop.clone())],
            },
            cooking_time: 1,
            experience_milli: 0,
        }),
        result: RecipeResult {
            item: cooked_porkchop,
            count: 1,
        },
    }]);
    let cpos = ChunkPos { x: 0, z: 0 };
    let source_pos = mc_world::BlockPos { x: 1, y: 66, z: 1 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let campfire_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
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
    storage.set_block_at(source_pos, BlockStateId(1)).unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(campfire_pos, BlockStateId(3)).unwrap();
    let mut source = mc_world::ChestBlockEntity::default();
    source.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };
    storage.set_chest_block_entity(source_pos, source).unwrap();
    storage
        .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        items,
        tags: Arc::new(TagsData::default()),
        recipes,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(51),
        name: "HopperCampfireViewer".to_string(),
    };
    let (tx, mut rx) = mpsc::channel(16);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let _ = sessions.mark_loaded(session_id, (0, 0));
    assert_eq!(sessions.register_chest_viewer(session_id, source_pos), 1);

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    let final_report = run_scheduled_block_ticks_for_range(&config, &sessions, 21, 28).await;
    assert_eq!(final_report.drained, 1);
    assert_eq!(final_report.applied, 1);
    {
        let mut storage = world.lock().await;
        let source = storage
            .chest_block_entity(source_pos)
            .unwrap()
            .expect("source chest");
        let hopper = storage
            .hopper_block_entity(hopper_pos)
            .unwrap()
            .expect("hopper");
        assert!(source.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        assert!(hopper.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        let scheduled = storage
            .scheduled_block_ticks(cpos)
            .unwrap()
            .expect("chunk scheduled ticks");
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].pos, hopper_pos);
        assert_eq!(scheduled[0].trigger_tick, 29);
    }

    let mut saw_chest = false;
    let mut saw_campfire = false;
    for _ in 0..2 {
        match rx.try_recv().expect("hopper campfire update") {
            OutboundCommand::ChestSlots {
                position,
                state_id,
                slots,
            } => {
                assert_eq!(position, source_pos);
                assert_eq!(state_id, 2);
                assert_eq!(slots[0], ItemStack::EMPTY);
                saw_chest = true;
            }
            OutboundCommand::BlockEntityData {
                position,
                block_entity_type,
                nbt,
            } => {
                assert_eq!(position, campfire_pos);
                assert_eq!(block_entity_type, CAMPFIRE_BLOCK_ENTITY_TYPE_ID);
                assert_eq!(
                    nbt,
                    Tag::Compound(vec![(
                        "Items".into(),
                        Tag::List(ListTag {
                            element_type: mc_nbt::tag_type::COMPOUND,
                            elements: vec![Tag::Compound(vec![
                                ("Slot".into(), Tag::Int(0)),
                                ("id".into(), Tag::String(porkchop.as_str().to_string())),
                                ("count".into(), Tag::Int(1)),
                            ])],
                        }),
                    )])
                );
                saw_campfire = true;
            }
            other => panic!("unexpected outbound command: {other:?}"),
        }
    }
    assert!(saw_chest);
    assert!(saw_campfire);

    let (_simulation, owner) = simulation_channel();
    let cook_report = owner
        .run_campfire_cooking_ticks(&config, &sessions, None, None)
        .await;

    assert_eq!(cook_report.persisted, 1);
    assert_eq!(cook_report.completed, 1);
    assert_eq!(cook_report.dropped, 1);
    assert!(sessions.campfire_cooking_state(campfire_pos).is_empty());
}

#[test]
fn hopper_campfire_persistence_failure_does_not_publish_cooking_state() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let porkchop = Identifier::parse("minecraft:porkchop").unwrap();
    let cooked_porkchop = Identifier::parse("minecraft:cooked_porkchop").unwrap();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[simple_block(0, "minecraft:air")])
            .expect("air registry"),
    );
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: porkchop.clone(),
            protocol_id: 42,
        },
        ItemReport {
            id: cooked_porkchop.clone(),
            protocol_id: 43,
        },
    ]);
    let recipes = vec![Recipe {
        id: Identifier::parse("minecraft:test_campfire").unwrap(),
        kind: RecipeKind::CampfireCooking(SmeltingRecipe {
            ingredient: Ingredient {
                alternatives: vec![IngredientAlternative::Item(porkchop)],
            },
            cooking_time: 20,
            experience_milli: 0,
        }),
        result: RecipeResult {
            item: cooked_porkchop,
            count: 1,
        },
    }];
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let cpos = ChunkPos { x: 0, z: 0 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
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
    let sessions = SessionRegistry::new();
    let tags = TagsData::default();
    let context = HopperTransferContext {
        blocks: blocks.as_ref(),
        items: &items,
        tags: &tags,
        recipes: &recipes,
        sessions: &sessions,
    };
    let moving = FurnaceSlot {
        count: 1,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };

    assert!(
        insert_hopper_stack_into_campfire(&context, &mut storage, position, &moving).is_none(),
        "failed persistence must not tell the hopper to debit its source slot"
    );
    assert!(sessions.campfire_cooking_state(position).is_empty());
}

#[tokio::test]
async fn scheduled_hopper_tick_pulls_from_second_half_of_double_chest() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
        ])
        .unwrap(),
    );
    let cpos = ChunkPos { x: 0, z: 0 };
    let source_left_pos = mc_world::BlockPos { x: 1, y: 66, z: 1 };
    let source_right_pos = mc_world::BlockPos { x: 2, y: 66, z: 1 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let target_pos = mc_world::BlockPos { x: 2, y: 65, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
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
    storage
        .set_block_at(source_left_pos, BlockStateId(1))
        .unwrap();
    storage
        .set_block_at(source_right_pos, BlockStateId(1))
        .unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(target_pos, BlockStateId(1)).unwrap();
    storage
        .set_chest_block_entity(source_left_pos, mc_world::ChestBlockEntity::default())
        .unwrap();
    let mut source_right = mc_world::ChestBlockEntity::default();
    source_right.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };
    storage
        .set_chest_block_entity(source_right_pos, source_right)
        .unwrap();
    storage
        .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
        .unwrap();
    storage
        .set_chest_block_entity(target_pos, mc_world::ChestBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "DoubleChestSourceHopper");

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    let mut storage = world.lock().await;
    let source_left = storage
        .chest_block_entity(source_left_pos)
        .unwrap()
        .expect("source left chest");
    let source_right = storage
        .chest_block_entity(source_right_pos)
        .unwrap()
        .expect("source right chest");
    let target = storage
        .chest_block_entity(target_pos)
        .unwrap()
        .expect("target chest");
    let hopper = storage
        .hopper_block_entity(hopper_pos)
        .unwrap()
        .expect("hopper");
    assert!(
        source_left
            .slots
            .iter()
            .all(mc_world::FurnaceSlot::is_empty)
    );
    assert!(
        source_right
            .slots
            .iter()
            .all(mc_world::FurnaceSlot::is_empty)
    );
    assert_eq!(
        hopper.slots[0],
        mc_world::FurnaceSlot {
            count: 1,
            item_id: 42,
            damage: None,
            enchantments: Vec::new(),
        }
    );
    assert!(target.slots.iter().all(mc_world::FurnaceSlot::is_empty));
}

#[tokio::test]
async fn scheduled_hopper_tick_inserts_into_second_half_of_double_chest() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
            BlockReport {
                id: Identifier::parse("minecraft:comparator").unwrap(),
                properties: prop_schema(&[
                    ("facing", &["west"]),
                    ("mode", &["compare"]),
                    ("powered", &["false", "true"]),
                ]),
                states: vec![
                    state(
                        3,
                        true,
                        &[
                            ("facing", "west"),
                            ("mode", "compare"),
                            ("powered", "false"),
                        ],
                    ),
                    state(
                        4,
                        false,
                        &[("facing", "west"), ("mode", "compare"), ("powered", "true")],
                    ),
                ],
            },
        ])
        .unwrap(),
    );
    let cpos = ChunkPos { x: 0, z: 0 };
    let source_pos = mc_world::BlockPos { x: 1, y: 66, z: 1 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let target_left_pos = mc_world::BlockPos { x: 2, y: 65, z: 1 };
    let target_right_pos = mc_world::BlockPos { x: 3, y: 65, z: 1 };
    let comparator_pos = mc_world::BlockPos { x: 4, y: 65, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
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
    storage.set_block_at(source_pos, BlockStateId(1)).unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage
        .set_block_at(target_left_pos, BlockStateId(1))
        .unwrap();
    storage
        .set_block_at(target_right_pos, BlockStateId(1))
        .unwrap();
    storage
        .set_block_at(comparator_pos, BlockStateId(3))
        .unwrap();
    let mut source = mc_world::ChestBlockEntity::default();
    source.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 42,
        damage: None,
        enchantments: Vec::new(),
    };
    storage.set_chest_block_entity(source_pos, source).unwrap();
    storage
        .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
        .unwrap();
    let mut target_left = mc_world::ChestBlockEntity::default();
    for slot in &mut target_left.slots {
        *slot = mc_world::FurnaceSlot {
            count: 64,
            item_id: 42,
            damage: None,
            enchantments: Vec::new(),
        };
    }
    storage
        .set_chest_block_entity(target_left_pos, target_left)
        .unwrap();
    storage
        .set_chest_block_entity(target_right_pos, mc_world::ChestBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(52),
        name: "DoubleChestTargetViewer".to_string(),
    };
    let (tx, mut rx) = mpsc::channel(16);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let _ = sessions.mark_loaded(session_id, (0, 0));
    assert_eq!(
        sessions.register_chest_viewer(session_id, target_left_pos),
        1
    );

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    let final_report = run_scheduled_block_ticks_for_range(&config, &sessions, 21, 28).await;
    assert_eq!(final_report.drained, 1);
    assert_eq!(final_report.applied, 1);
    let comparator_report = run_scheduled_block_ticks_for_range(&config, &sessions, 29, 30).await;
    assert_eq!(comparator_report.applied, 1);
    {
        let mut storage = world.lock().await;
        let source = storage
            .chest_block_entity(source_pos)
            .unwrap()
            .expect("source chest");
        let target_left = storage
            .chest_block_entity(target_left_pos)
            .unwrap()
            .expect("target left chest");
        let target_right = storage
            .chest_block_entity(target_right_pos)
            .unwrap()
            .expect("target right chest");
        assert!(source.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        assert!(target_left.slots.iter().all(|slot| {
            *slot
                == mc_world::FurnaceSlot {
                    count: 64,
                    item_id: 42,
                    damage: None,
                    enchantments: Vec::new(),
                }
        }));
        assert_eq!(
            target_right.slots[0],
            mc_world::FurnaceSlot {
                count: 1,
                item_id: 42,
                damage: None,
                enchantments: Vec::new(),
            }
        );
        assert_eq!(
            storage.get_cached_block(comparator_pos),
            Some(BlockStateId(4))
        );
    }

    match rx.try_recv().expect("double chest target receives slots") {
        OutboundCommand::ChestSlots {
            position,
            state_id,
            slots,
        } => {
            assert_eq!(position, target_left_pos);
            assert_eq!(state_id, 2);
            assert_eq!(slots.len(), 54);
            assert_eq!(slots[0], ItemStack::new(42, 64));
            assert_eq!(slots[27], ItemStack::new(42, 1));
        }
        other => panic!("unexpected outbound command: {other:?}"),
    }
}

#[tokio::test]
async fn scheduled_hopper_tick_does_not_extract_empty_furnace_output() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
            simple_block(3, "minecraft:furnace"),
        ])
        .unwrap(),
    );
    let cpos = ChunkPos { x: 0, z: 0 };
    let furnace_pos = mc_world::BlockPos { x: 1, y: 66, z: 1 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let target_pos = mc_world::BlockPos { x: 2, y: 65, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
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
    storage.set_block_at(furnace_pos, BlockStateId(3)).unwrap();
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(target_pos, BlockStateId(1)).unwrap();
    storage
        .set_furnace_block_entity(furnace_pos, mc_world::FurnaceBlockEntity::default())
        .unwrap();
    storage
        .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
        .unwrap();
    storage
        .set_chest_block_entity(target_pos, mc_world::ChestBlockEntity::default())
        .unwrap();
    storage
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            hopper_pos,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        ))
        .unwrap();

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        items: Arc::new(ItemRegistry::from_report(&[])),
        tags: Arc::new(TagsData::default()),
        recipes: Arc::new(Vec::new()),
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(50),
        name: "EmptyFurnaceOutputLoadedViewer".to_string(),
    };
    let (tx, _rx) = mpsc::channel(16);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let _ = sessions.mark_loaded(session_id, (0, 0));

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 0);
    {
        let mut storage = world.lock().await;
        let furnace = storage
            .furnace_block_entity(furnace_pos)
            .unwrap()
            .expect("furnace");
        let hopper = storage
            .hopper_block_entity(hopper_pos)
            .unwrap()
            .expect("hopper");
        let target = storage
            .chest_block_entity(target_pos)
            .unwrap()
            .expect("target chest");
        assert!(furnace.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        assert!(hopper.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        assert!(target.slots.iter().all(mc_world::FurnaceSlot::is_empty));
        let scheduled = storage
            .scheduled_block_ticks(cpos)
            .unwrap()
            .expect("chunk scheduled ticks");
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].pos, hopper_pos);
        assert_eq!(scheduled[0].trigger_tick, 21);
    }
}

#[test]
fn scheduled_hopper_transfer_preserves_enchantments_when_merging_matching_stacks() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
        ])
        .unwrap(),
    );
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:iron_ingot").unwrap(),
        protocol_id: 43,
    }]);
    let cpos = ChunkPos { x: 0, z: 0 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let target_pos = mc_world::BlockPos { x: 2, y: 65, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
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
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(target_pos, BlockStateId(1)).unwrap();
    let efficiency = mc_data::ItemEnchantment {
        id: Identifier::parse("minecraft:efficiency").unwrap(),
        level: 1,
    };
    let mut hopper = mc_world::HopperBlockEntity {
        transfer_cooldown: 0,
        ..Default::default()
    };
    hopper.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 43,
        damage: None,
        enchantments: vec![efficiency.clone()],
    };
    let mut target = mc_world::ChestBlockEntity::default();
    target.slots[0] = mc_world::FurnaceSlot {
        count: 63,
        item_id: 43,
        damage: None,
        enchantments: vec![efficiency.clone()],
    };
    storage.set_hopper_block_entity(hopper_pos, hopper).unwrap();
    storage.set_chest_block_entity(target_pos, target).unwrap();

    let tags = TagsData::default();
    let recipes = Vec::new();
    let sessions = SessionRegistry::new();
    let context = HopperTransferContext {
        blocks: blocks.as_ref(),
        items: &items,
        tags: &tags,
        recipes: recipes.as_slice(),
        sessions: &sessions,
    };
    let result = scheduled_hopper_transfer(&context, &mut storage, hopper_pos, BlockStateId(2))
        .expect("transfer should apply");

    assert!(result.moved);
    assert_eq!(result.updates.len(), 1);
    let hopper = storage
        .hopper_block_entity(hopper_pos)
        .unwrap()
        .expect("hopper");
    let target = storage
        .chest_block_entity(target_pos)
        .unwrap()
        .expect("target chest");
    assert!(hopper.slots.iter().all(mc_world::FurnaceSlot::is_empty));
    assert_eq!(hopper.transfer_cooldown, HOPPER_TRANSFER_DELAY_TICKS as i32);
    assert_eq!(
        target.slots[0],
        mc_world::FurnaceSlot {
            count: 64,
            item_id: 43,
            damage: None,
            enchantments: vec![efficiency],
        }
    );
}

#[test]
fn scheduled_hopper_transfer_preserves_hopper_slot_when_target_has_no_room() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:chest"),
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: prop_schema(&[("facing", &["east"])]),
                states: vec![state(2, true, &[("facing", "east")])],
            },
        ])
        .unwrap(),
    );
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:iron_ingot").unwrap(),
        protocol_id: 43,
    }]);
    let cpos = ChunkPos { x: 0, z: 0 };
    let hopper_pos = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    let target_pos = mc_world::BlockPos { x: 2, y: 65, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
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
    storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(target_pos, BlockStateId(1)).unwrap();
    let mut hopper = mc_world::HopperBlockEntity {
        transfer_cooldown: 0,
        ..Default::default()
    };
    hopper.slots[0] = mc_world::FurnaceSlot {
        count: 1,
        item_id: 43,
        damage: None,
        enchantments: Vec::new(),
    };
    let mut target = mc_world::ChestBlockEntity::default();
    for slot in &mut target.slots {
        *slot = mc_world::FurnaceSlot {
            count: 64,
            item_id: 43,
            damage: None,
            enchantments: Vec::new(),
        };
    }
    storage.set_hopper_block_entity(hopper_pos, hopper).unwrap();
    storage.set_chest_block_entity(target_pos, target).unwrap();

    let tags = TagsData::default();
    let recipes = Vec::new();
    let sessions = SessionRegistry::new();
    let context = HopperTransferContext {
        blocks: blocks.as_ref(),
        items: &items,
        tags: &tags,
        recipes: recipes.as_slice(),
        sessions: &sessions,
    };
    let result = scheduled_hopper_transfer(&context, &mut storage, hopper_pos, BlockStateId(2))
        .expect("hopper tick runs");

    assert!(!result.moved);
    assert!(result.updates.is_empty());
    let hopper = storage
        .hopper_block_entity(hopper_pos)
        .unwrap()
        .expect("hopper");
    let target = storage
        .chest_block_entity(target_pos)
        .unwrap()
        .expect("target chest");
    assert_eq!(
        hopper.slots[0],
        mc_world::FurnaceSlot {
            count: 1,
            item_id: 43,
            damage: None,
            enchantments: Vec::new(),
        }
    );
    assert_eq!(hopper.transfer_cooldown, 0);
    assert!(target.slots.iter().all(|slot| {
        *slot
            == mc_world::FurnaceSlot {
                count: 64,
                item_id: 43,
                damage: None,
                enchantments: Vec::new(),
            }
    }));
}
