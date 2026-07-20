//! M17 - server-owned vanilla entity visibility baseline.

use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, BlockChangedAck, ClientboundContainerSetSlot, ClientboundKeepAlive,
    ClientboundSetExperience, ClientboundSetHealth, ClientboundSetTime, ConfirmTeleportation,
    EntityAnimation, EntityEvent, GameEvent, InteractionHand, LevelChunkWithLight,
    MoveEntityPosRot, MovePlayerFlags, RemoveEntities, ServerboundAttack, ServerboundChatCommand,
    ServerboundKeepAlive, ServerboundMovePlayerPos, ServerboundMovePlayerPosRot,
    ServerboundUseItem, SetCenterChunk, SetEntityMotion, SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;

const VIEW_DISTANCE: i32 = 2;
// These scenarios drive independent debug servers, but their timed survival
// combat loops contend for Tokio scheduling in the same test process.
static SURVIVAL_MOB_TEST_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

#[tokio::test]
async fn vanilla_client_receives_server_owned_passive_mob_and_motion() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    if !blocks_json.exists() {
        eprintln!("skipping: {} missing", blocks_json.display());
        return;
    }

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let block_light_path = vanilla_dir.join("reports/block_light.json");
    let block_light = mc_data::block_light::load(&block_light_path)
        .ok()
        .map(Arc::new);
    let registries_json = vanilla_dir.join("reports/registries.json");
    let entity_report = mc_data::entity_types::load_entity_types_report(&registries_json)
        .expect("entity type report loads");
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("entity type report is the exact 26.1.2 registry"),
    );
    let biome_spawns_path = vanilla_dir.join("data/minecraft/worldgen/biome");
    let biome_spawns = mc_data::biomes::load_biome_spawn_rules(&biome_spawns_path)
        .map(Arc::new)
        .unwrap_or_default();

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M17 mob presence".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light,
        items: Arc::new(mc_data::items::ItemRegistry::default()),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types,
        biome_spawns,
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, _) = connect_to_play(addr, "M17MobProbe").await;
    let mob = wait_for_chunk_and_entity_spawn(&mut client, (0, 0), |_| true).await;
    wait_for_mob_motion_after_spawn(&mut client, mob.entity_id).await;
}

#[tokio::test]
async fn embedded_playable_seed_spawns_food_mob_in_initial_window() {
    let data = Arc::new(mc_data::solaris_required_data());
    let report = mc_data::blocks::solaris_required_blocks_report();
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let items = Arc::new(mc_data::items::solaris_required_items());
    let entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    let food_mob_type_ids = [
        "minecraft:cow",
        "minecraft:pig",
        "minecraft:chicken",
        "minecraft:sheep",
    ]
    .into_iter()
    .map(|id| {
        entity_types
            .id_of(&mc_data::Identifier::parse(id).unwrap())
            .and_then(|id| i32::try_from(id).ok())
            .unwrap_or_else(|| panic!("embedded entity type {id}"))
    })
    .collect::<Vec<_>>();

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "P10 embedded passive food mob".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags: Arc::new(mc_data::tags::solaris_required_item_tags(&items)),
        recipes: Arc::new(mc_data::recipes::solaris_required_recipes()),
        loot: Arc::new(mc_data::loot::builtin().clone()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::solaris_required_item_facts()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &report,
        )),
        entity_types,
        biome_spawns: Arc::new(mc_data::biomes::solaris_required_biome_spawn_rules()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), false),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, _) = connect_to_play(addr, "P10EmbeddedFood").await;
    let mob = wait_for_chunk_and_entity_spawn(&mut client, (0, 0), |entity| {
        food_mob_type_ids.contains(&entity.entity_type_id)
    })
    .await;

    assert!(
        mob.x.abs() <= f64::from((VIEW_DISTANCE + 1) * 16)
            && mob.z.abs() <= f64::from((VIEW_DISTANCE + 1) * 16),
        "embedded playable food mob should spawn inside the initial view window: {mob:?}"
    );
}

#[tokio::test]
async fn two_clients_receive_same_server_owned_mob() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    if !blocks_json.exists() {
        eprintln!("skipping: {} missing", blocks_json.display());
        return;
    }

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let block_light_path = vanilla_dir.join("reports/block_light.json");
    let block_light = mc_data::block_light::load(&block_light_path)
        .ok()
        .map(Arc::new);
    let registries_json = vanilla_dir.join("reports/registries.json");
    let entity_report = mc_data::entity_types::load_entity_types_report(&registries_json)
        .expect("entity type report loads");
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("entity type report is the exact 26.1.2 registry"),
    );
    let biome_spawns_path = vanilla_dir.join("data/minecraft/worldgen/biome");
    let biome_spawns = mc_data::biomes::load_biome_spawn_rules(&biome_spawns_path)
        .map(Arc::new)
        .unwrap_or_default();

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M32 multi-client mob visibility".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light,
        items: Arc::new(mc_data::items::ItemRegistry::default()),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types,
        biome_spawns,
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut alice, _) = connect_to_play(addr, "M32MobAlice").await;
    let mob = wait_for_chunk_and_entity_spawn(&mut alice, (0, 0), |_| true).await;

    let (mut bob, _) = connect_to_play(addr, "M32MobBob").await;
    wait_for_chunk_and_entity_spawn(&mut bob, (0, 0), |entity| entity.entity_id == mob.entity_id)
        .await;
    wait_for_mob_motion_after_spawn(&mut alice, mob.entity_id).await;
    drop(bob);
}

#[tokio::test]
async fn survival_attack_passive_mob_uses_all_configured_drops() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    let registries_json = vanilla_dir.join("reports/registries.json");
    if !blocks_json.exists() || !registries_json.exists() {
        eprintln!(
            "skipping: missing {} or {}",
            blocks_json.display(),
            registries_json.display()
        );
        return;
    }

    let _survival_mob_guard = SURVIVAL_MOB_TEST_LOCK.lock().await;
    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let block_light = mc_data::block_light::load(vanilla_dir.join("reports/block_light.json"))
        .ok()
        .map(Arc::new);
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("entity type report is the exact 26.1.2 registry"),
    );
    let cow = mc_data::Identifier::parse("minecraft:cow").unwrap();
    let cow_type_id = entity_types
        .id_of(&cow)
        .and_then(|id| i32::try_from(id).ok())
        .expect("cow entity type");
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let item_facts = Arc::new(
        mc_data::item_components::load_item_facts(
            vanilla_dir.join("reports/minecraft/components/item"),
        )
        .expect("item component facts load"),
    );
    let leather = mc_data::Identifier::parse("minecraft:leather").unwrap();
    let beef = mc_data::Identifier::parse("minecraft:beef").unwrap();
    let weapon_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:netherite_axe").unwrap())
        .expect("netherite axe item");
    let loot = Arc::new(mc_data::loot::LootTables::from_drop_lists(
        std::collections::BTreeMap::from([(
            cow,
            vec![
                mc_data::loot::LootDrop::single(leather.clone()),
                mc_data::loot::LootDrop::single(beef.clone()),
            ],
        )]),
        std::collections::BTreeMap::new(),
    ));
    let biome_spawns =
        mc_data::biomes::load_biome_spawn_rules(vanilla_dir.join("data/minecraft/worldgen/biome"))
            .map(Arc::new)
            .unwrap_or_default();

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M23 mob food".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::clone(&loot),
        block_light,
        items: Arc::clone(&items),
        item_facts,
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types,
        biome_spawns,
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "M23MobFood").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:netherite_axe 1 0".into(),
        })
        .await
        .expect("give one-hit axe");
    wait_for_slot_stack(&mut client, weapon_item_id, 1).await;

    let summon_pos = (sync.x + 1.0, sync.y, sync.z);
    client
        .write_packet(&ServerboundChatCommand {
            command: format!(
                "summon minecraft:cow {} {} {}",
                summon_pos.0, summon_pos.1, summon_pos.2
            ),
        })
        .await
        .expect("summon configured-drop cow");
    let mob = wait_for_entity_type_spawn_at(&mut client, cow_type_id, summon_pos).await;
    let leather_item_id = items.id_of(&leather).expect("leather item id");
    let beef_item_id = items.id_of(&beef).expect("beef item id");

    client
        .write_packet(&ServerboundMovePlayerPos {
            x: mob.x,
            y: mob.y,
            z: mob.z,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move to mob");
    client
        .write_packet(&ServerboundAttack {
            entity_id: mob.entity_id,
        })
        .await
        .expect("attack mob");

    let mut saw_mob_remove = false;
    let mut saw_leather_slot = false;
    let mut saw_beef_slot = false;
    let mut saw_xp_credit = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_mob_remove && saw_leather_slot && saw_beef_slot && saw_xp_credit) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("mob attack response");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == RemoveEntities::ID {
            let mut body = frame.body;
            let pkt = RemoveEntities::decode(&mut body).expect("decode RemoveEntities");
            if pkt.entity_ids.contains(&mob.entity_id) {
                saw_mob_remove = true;
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            saw_leather_slot |=
                pkt.item_stack.item_id == leather_item_id && pkt.item_stack.count >= 1;
            saw_beef_slot |= pkt.item_stack.item_id == beef_item_id && pkt.item_stack.count >= 1;
        } else if frame.id == ClientboundSetExperience::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetExperience::decode(&mut body).expect("decode SetExperience");
            saw_xp_credit |= pkt.total_experience > 0;
        }
    }
}

#[tokio::test]
async fn survival_zombie_damages_player_and_drops_rotten_flesh() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    let registries_json = vanilla_dir.join("reports/registries.json");
    if !blocks_json.exists() || !registries_json.exists() {
        eprintln!(
            "skipping: missing {} or {}",
            blocks_json.display(),
            registries_json.display()
        );
        return;
    }

    let _survival_mob_guard = SURVIVAL_MOB_TEST_LOCK.lock().await;
    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let block_light = mc_data::block_light::load(vanilla_dir.join("reports/block_light.json"))
        .ok()
        .map(Arc::new);
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("entity type report is the exact 26.1.2 registry"),
    );
    let zombie = mc_data::Identifier::parse("minecraft:zombie").unwrap();
    let zombie_type_id = entity_types
        .id_of(&zombie)
        .and_then(|id| i32::try_from(id).ok())
        .expect("zombie entity type");
    let zombie_max_health = entity_types
        .facts_of(&zombie)
        .and_then(|facts| facts.attributes.max_health)
        .expect("zombie max health");
    let zombie_bare_hand_hits =
        usize::try_from(zombie_max_health as u64).expect("zombie max health fits attack count");
    assert_eq!(zombie_bare_hand_hits as f64, zombie_max_health);
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let rotten_flesh_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:rotten_flesh").unwrap())
        .expect("rotten_flesh item");
    let loot = Arc::new(mc_data::loot::builtin().clone());
    assert_eq!(
        loot.entity_drop(&zombie),
        Some(&mc_data::Identifier::parse("minecraft:rotten_flesh").unwrap()),
        "zombie drop must remain available through embedded fallback loot"
    );
    let biome_spawns =
        mc_data::biomes::load_biome_spawn_rules(vanilla_dir.join("data/minecraft/worldgen/biome"))
            .map(Arc::new)
            .unwrap_or_default();

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M24 zombie pressure".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot,
        block_light,
        items: Arc::clone(&items),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types,
        biome_spawns,
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    let mut simulation_ticks = bound
        .runtime_telemetry_handle()
        .subscribe_simulation_ticks();
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, _) = connect_to_play(addr, "M24Zombie").await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "summon minecraft:zombie".to_string(),
        })
        .await
        .expect("summon zombie");
    let zombie = wait_for_zombie_spawn(&mut client, zombie_type_id).await;
    client
        .write_packet(&ServerboundMovePlayerPos {
            x: zombie.x,
            y: zombie.y,
            z: zombie.z,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move to zombie");
    wait_for_health_below(&mut client, 20.0).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug survival heal 20".into(),
        })
        .await
        .expect("heal after hostile damage check");
    wait_for_health_near(&mut client, 20.0, f32::EPSILON).await;

    client
        .write_packet(&ServerboundAttack {
            entity_id: zombie.entity_id,
        })
        .await
        .expect("first attack zombie");
    wait_for_entity_hurt(&mut client, zombie.entity_id).await;

    for _ in 0..zombie_bare_hand_hits.saturating_sub(2) {
        wait_for_additional_simulation_ticks(&mut simulation_ticks, 7).await;
        client
            .write_packet(&ServerboundAttack {
                entity_id: zombie.entity_id,
            })
            .await
            .expect("attack zombie");
        wait_for_entity_hurt(&mut client, zombie.entity_id).await;
    }
    wait_for_additional_simulation_ticks(&mut simulation_ticks, 7).await;
    client
        .write_packet(&ServerboundAttack {
            entity_id: zombie.entity_id,
        })
        .await
        .expect("lethal attack zombie");

    let mut saw_zombie_remove = false;
    let mut saw_rotten_flesh = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_zombie_remove && saw_rotten_flesh) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("zombie attack response");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == RemoveEntities::ID {
            let mut body = frame.body;
            let pkt = RemoveEntities::decode(&mut body).expect("decode RemoveEntities");
            if pkt.entity_ids.contains(&zombie.entity_id) {
                saw_zombie_remove = true;
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            if pkt.item_stack.item_id == rotten_flesh_id && pkt.item_stack.count >= 1 {
                saw_rotten_flesh = true;
            }
        }
    }
}

#[tokio::test]
async fn survival_shield_blocks_frontal_zombie_damage() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    let registries_json = vanilla_dir.join("reports/registries.json");
    if !blocks_json.exists() || !registries_json.exists() {
        eprintln!(
            "skipping: missing {} or {}",
            blocks_json.display(),
            registries_json.display()
        );
        return;
    }

    let _survival_mob_guard = SURVIVAL_MOB_TEST_LOCK.lock().await;
    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let block_light = mc_data::block_light::load(vanilla_dir.join("reports/block_light.json"))
        .ok()
        .map(Arc::new);
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("entity type report is the exact 26.1.2 registry"),
    );
    let zombie_type_id = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:zombie").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("zombie entity type");
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let shield_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:shield").unwrap())
        .expect("shield item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M56 shield block".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light,
        items,
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types,
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "M56Shield").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug survival damage 4".into(),
        })
        .await
        .expect("baseline damage");
    wait_for_health_near(&mut client, 16.0, f32::EPSILON).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug survival heal 20".into(),
        })
        .await
        .expect("heal before shield check");
    wait_for_health_near(&mut client, 20.0, f32::EPSILON).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:shield 1 0".into(),
        })
        .await
        .expect("give shield");
    wait_for_slot_stack(&mut client, shield_id, 1).await;
    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: sync.x,
            y: sync.y,
            z: sync.z,
            yaw: 0.0,
            pitch: 0.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("face shield target");
    client
        .write_packet(&ServerboundUseItem {
            hand: InteractionHand::MainHand,
            sequence: 56,
            y_rot: 0.0,
            x_rot: 0.0,
        })
        .await
        .expect("start shield use");
    wait_for_block_ack(&mut client, 56).await;
    wait_for_world_ticks(&mut client, 5).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: format!(
                "summon minecraft:zombie {} {} {}",
                sync.x,
                sync.y,
                sync.z + 1.0
            ),
        })
        .await
        .expect("summon frontal zombie");
    let zombie = wait_for_zombie_spawn(&mut client, zombie_type_id).await;
    wait_for_blocked_zombie_attack(&mut client, zombie.entity_id, shield_id).await;
}

async fn wait_for_entity_hurt(client: &mut Client, entity_id: i32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for entity hurt event");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == EntityEvent::ID {
            let mut body = frame.body;
            let pkt = EntityEvent::decode(&mut body).expect("decode EntityEvent");
            if pkt.entity_id == entity_id && pkt.event_id == 2 {
                return;
            }
        }
    }
}

async fn connect_to_play(
    addr: std::net::SocketAddr,
    name: &str,
) -> (Client, SynchronizePlayerPosition) {
    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client.drive_login(addr, name).await.expect("drive login");
    client
        .drive_configuration()
        .await
        .expect("drive configuration");
    let _ = client.read_play_login().await.expect("play entry");
    let _: mc_protocol::packets::play::ClientboundCommands =
        client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    let _: mc_protocol::packets::play::ClientboundInitializeBorder =
        client.read_typed().await.expect("InitializeBorder");
    let _: mc_protocol::packets::play::ClientboundSetTime =
        client.read_typed().await.expect("SetTime");
    let _: mc_protocol::packets::play::SetDefaultSpawnPosition =
        client.read_typed().await.expect("SetDefaultSpawnPosition");
    let _: GameEvent = client.read_typed().await.expect("GameEvent");
    let _: SetCenterChunk = client.read_typed().await.expect("SetCenterChunk");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");
    (client, sync)
}

async fn drain_until_chunk(client: &mut Client, target: (i32, i32)) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("drain chunks");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            let mut body = frame.body;
            let pkt = LevelChunkWithLight::decode(&mut body).expect("decode chunk");
            if (pkt.chunk_x, pkt.chunk_z) == target {
                return;
            }
        }
    }
}

async fn wait_for_chunk_and_entity_spawn(
    client: &mut Client,
    target: (i32, i32),
    matches: impl Fn(&AddEntity) -> bool,
) -> AddEntity {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut saw_chunk = false;
    let mut entity = None;
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for chunk and entity spawn");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            let mut body = frame.body;
            let packet = LevelChunkWithLight::decode(&mut body).expect("decode chunk");
            saw_chunk |= (packet.chunk_x, packet.chunk_z) == target;
        } else if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode AddEntity");
            if matches(&packet) {
                entity = Some(packet);
            }
        }
        if saw_chunk && let Some(entity) = entity.take() {
            return entity;
        }
    }
}

async fn wait_for_entity_type_spawn_at(
    client: &mut Client,
    entity_type_id: i32,
    position: (f64, f64, f64),
) -> AddEntity {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for summoned entity spawn");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let pkt = AddEntity::decode(&mut body).expect("decode AddEntity");
            if pkt.entity_type_id == entity_type_id
                && (pkt.x - position.0).abs() < 0.01
                && (pkt.y - position.1).abs() < 0.01
                && (pkt.z - position.2).abs() < 0.01
            {
                return pkt;
            }
        }
    }
}

async fn wait_for_zombie_spawn(client: &mut Client, zombie_type_id: i32) -> AddEntity {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for zombie spawn");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let pkt = AddEntity::decode(&mut body).expect("decode AddEntity");
            if pkt.entity_type_id == zombie_type_id {
                return pkt;
            }
        }
    }
}

async fn wait_for_health_below(client: &mut Client, health: f32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for hostile damage");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetHealth::decode(&mut body).expect("decode SetHealth");
            if pkt.health < health {
                return;
            }
        }
    }
}

async fn wait_for_health_near(client: &mut Client, health: f32, tolerance: f32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for health level");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetHealth::decode(&mut body).expect("decode SetHealth");
            if (pkt.health - health).abs() <= tolerance {
                return;
            }
        }
    }
}

async fn wait_for_slot_stack(client: &mut Client, item_id: u32, count: i32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("slot stack update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            if pkt.item_stack.item_id == item_id && pkt.item_stack.count == count {
                return;
            }
        }
    }
}

async fn wait_for_block_ack(client: &mut Client, sequence: i32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("block ack");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode BlockChangedAck");
            if pkt.sequence == sequence {
                return;
            }
        }
    }
}

async fn wait_for_additional_simulation_ticks(
    ticks: &mut tokio::sync::watch::Receiver<u64>,
    additional: u64,
) {
    let target = (*ticks.borrow()).saturating_add(additional);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let current = *ticks.borrow_and_update();
            if current >= target {
                return;
            }
            ticks
                .changed()
                .await
                .expect("simulation tick publisher remains active");
        }
    })
    .await
    .expect("simulation did not reach the entity attack cooldown tick");
}

async fn wait_for_world_ticks(client: &mut Client, ticks: i64) {
    let mut baseline = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for shield activation ticks");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id != ClientboundSetTime::ID {
            continue;
        }
        let mut body = frame.body;
        let packet = ClientboundSetTime::decode(&mut body).expect("decode SetTime");
        let start = *baseline.get_or_insert(packet.game_time);
        if packet.game_time.saturating_sub(start) >= ticks {
            return;
        }
    }
}

async fn wait_for_blocked_zombie_attack(client: &mut Client, entity_id: i32, shield_id: u32) {
    let mut saw_swing = false;
    let mut saw_damaged_shield = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_swing && saw_damaged_shield) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for blocked zombie damage");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == EntityAnimation::ID {
            let mut body = frame.body;
            let pkt = EntityAnimation::decode(&mut body).expect("decode EntityAnimation");
            saw_swing |= pkt.entity_id == entity_id;
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            saw_damaged_shield |= pkt.item_stack.item_id == shield_id
                && pkt.item_stack.count == 1
                && pkt.item_stack.damage.is_some_and(|damage| damage > 0);
        } else if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetHealth::decode(&mut body).expect("decode SetHealth");
            assert!(
                (pkt.health - 20.0).abs() <= f32::EPSILON,
                "frontal shield use should keep health unchanged: health={}",
                pkt.health,
            );
        }
    }
}

async fn wait_for_mob_motion_after_spawn(client: &mut Client, entity_id: i32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for cow motion");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == MoveEntityPosRot::ID {
            let mut body = frame.body;
            let pkt = MoveEntityPosRot::decode(&mut body).expect("decode MoveEntityPosRot");
            if pkt.entity_id == entity_id
                && (pkt.delta_x != 0 || pkt.delta_y != 0 || pkt.delta_z != 0)
            {
                return;
            }
        } else if frame.id == SetEntityMotion::ID {
            let mut body = frame.body;
            let _ = SetEntityMotion::decode(&mut body).expect("decode SetEntityMotion");
        }
    }
}

async fn handle_keepalive(client: &mut Client, id: i32, body: &bytes::Bytes) -> bool {
    if id != ClientboundKeepAlive::ID {
        return false;
    }
    let mut body = body.clone();
    let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode KeepAlive");
    client
        .write_packet(&ServerboundKeepAlive { id: keepalive.id })
        .await
        .expect("echo KeepAlive");
    true
}
