//! M17 - server-owned vanilla entity visibility baseline.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, ClientboundContainerSetSlot, ClientboundKeepAlive, ClientboundSetHealth,
    ConfirmTeleportation, EntityEvent, GameEvent, LevelChunkWithLight, MoveEntityPosRot,
    MovePlayerFlags, RemoveEntities, ServerboundAttack, ServerboundChatCommand,
    ServerboundKeepAlive, ServerboundMovePlayerPos, SetCenterChunk, SetEntityMotion,
    SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;

const VIEW_DISTANCE: i32 = 2;
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
    let entity_types = mc_data::entity_types::load_entity_types_report(&registries_json)
        .map(|report| {
            Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(
                &report,
            ))
        })
        .unwrap_or_default();
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
    drain_until_chunk(&mut client, (0, 0)).await;
    let mob = wait_for_passive_mob_spawn(&mut client).await;
    wait_for_mob_motion_after_spawn(&mut client, mob.entity_id).await;
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
    let entity_types = mc_data::entity_types::load_entity_types_report(&registries_json)
        .map(|report| {
            Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(
                &report,
            ))
        })
        .unwrap_or_default();
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
    drain_until_chunk(&mut alice, (0, 0)).await;
    let mob = wait_for_passive_mob_spawn(&mut alice).await;

    let (mut bob, _) = connect_to_play(addr, "M32MobBob").await;
    drain_until_chunk(&mut bob, (0, 0)).await;
    wait_for_entity_spawn(&mut bob, mob.entity_id).await;
    wait_for_mob_motion_after_spawn(&mut alice, mob.entity_id).await;
    drop(bob);
}

#[tokio::test]
async fn survival_attack_passive_mob_drops_food() {
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
    let entity_types = Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(
        &entity_report,
    ));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
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
        loot: Arc::new(mc_data::loot::LootTables::default()),
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
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, _) = connect_to_play(addr, "M23MobFood").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let (mob, drop_item) = wait_for_food_mob_spawn(&mut client, &entity_report).await;
    let drop_item_id = items.id_of(&drop_item).expect("drop item id");

    client
        .write_packet(&ServerboundMovePlayerPos {
            x: mob.x,
            y: mob.y,
            z: mob.z,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move to mob");
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(400)).await;
        client
            .write_packet(&ServerboundAttack {
                entity_id: mob.entity_id,
            })
            .await
            .expect("attack mob");
    }

    let mut saw_mob_remove = false;
    let mut saw_food_slot = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_mob_remove && saw_food_slot) {
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
            if pkt.item_stack.item_id == drop_item_id && pkt.item_stack.count >= 1 {
                saw_food_slot = true;
            }
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
    let entity_types = Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(
        &entity_report,
    ));
    let zombie_type_id = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:zombie").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("zombie entity type");
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let rotten_flesh_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:rotten_flesh").unwrap())
        .expect("rotten_flesh item");
    let zombie = mc_data::Identifier::parse("minecraft:zombie").unwrap();
    let loot = Arc::new(
        mc_data::loot::load_vanilla_subset(vanilla_dir.join("data/minecraft/loot_table"))
            .expect("loot tables load"),
    );
    assert_eq!(
        loot.entity_drop(&zombie),
        Some(&mc_data::Identifier::parse("minecraft:rotten_flesh").unwrap()),
        "zombie drop must come from sidecar loot tables"
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
        .write_packet(&ServerboundAttack {
            entity_id: zombie.entity_id,
        })
        .await
        .expect("first attack zombie");
    wait_for_entity_hurt(&mut client, zombie.entity_id).await;

    for _ in 0..8 {
        tokio::time::sleep(Duration::from_millis(400)).await;
        client
            .write_packet(&ServerboundAttack {
                entity_id: zombie.entity_id,
            })
            .await
            .expect("attack zombie");
        wait_for_entity_hurt(&mut client, zombie.entity_id).await;
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
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

async fn wait_for_passive_mob_spawn(client: &mut Client) -> AddEntity {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for passive mob spawn");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let pkt = AddEntity::decode(&mut body).expect("decode AddEntity");
            return pkt;
        }
    }
}

async fn wait_for_food_mob_spawn(
    client: &mut Client,
    entity_report: &[mc_data::entity_types::EntityTypeReport],
) -> (AddEntity, mc_data::Identifier) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for food mob spawn");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let pkt = AddEntity::decode(&mut body).expect("decode AddEntity");
            let Some(entity_type) = entity_report
                .iter()
                .find(|entry| entry.protocol_id as i32 == pkt.entity_type_id)
                .map(|entry| entry.id.as_str())
            else {
                continue;
            };
            if let Some(drop) = passive_food_drop(entity_type) {
                return (pkt, drop);
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

async fn wait_for_entity_spawn(client: &mut Client, entity_id: i32) -> AddEntity {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for entity spawn");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let pkt = AddEntity::decode(&mut body).expect("decode AddEntity");
            if pkt.entity_id == entity_id {
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

fn passive_food_drop(entity_type: &str) -> Option<mc_data::Identifier> {
    let entity = mc_data::Identifier::parse(entity_type).ok()?;
    mc_data::loot::builtin().entity_drop(&entity).cloned()
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
