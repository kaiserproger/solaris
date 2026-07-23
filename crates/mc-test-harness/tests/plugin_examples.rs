use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, BlockChangedAck, ClientboundCommands, ClientboundContainerSetContent,
    ClientboundContainerSetSlot, ClientboundOpenScreen, ClientboundSystemChat, CommandNodeKind,
    ConfirmTeleportation, ContainerInput, Direction, HashedStack, HashedStackComponentHashes,
    InteractionHand, MovePlayerFlags, PlayerActionKind, RemoveEntities, ServerboundAttack,
    ServerboundChatCommand, ServerboundContainerClick, ServerboundMovePlayerPos,
    ServerboundMovePlayerStatusOnly, ServerboundPlayerAction, ServerboundUseItemOn,
    SynchronizePlayerPosition, pack_block_pos,
};
use mc_script::{
    PlayerCommandAdmission, ScriptCommand, ScriptEvent, ScriptInventoryClick, ScriptPlayerContext,
    ScriptPlayerId, ScriptStorageMutation,
};
use mc_test_harness::client::{Client, FrameWaitLimits};

const FRAME_LIMITS: FrameWaitLimits = FrameWaitLimits {
    max_skipped_frames: Some(4096),
    max_skipped_bytes: Some(32 * 1024 * 1024),
};

struct CatalogMenu {
    container_id: i32,
    state_id: i32,
    product_id: u32,
    product_count: i32,
}

#[tokio::test]
async fn shipped_basic_economy_and_land_claims_route_real_lua_commands() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    copy_example_plugin("basic-economy", plugins.path());
    copy_example_plugin("land-claims", plugins.path());
    let (boundary, host) = mc_script::start_lua_host(mc_script::LuaHostConfig::new(plugins.path()))
        .expect("start economy and claims plugins");
    assert_eq!(host.loaded_plugins(), 2);

    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .expect("enqueue server start");
    let mut economy_zone_registered = false;
    let mut claims_loaded = false;
    for _ in 0..2 {
        let command = boundary
            .recv_command()
            .await
            .expect("plugin startup command");
        let admitted = boundary
            .accept_host_command(command)
            .expect("admit plugin startup command");
        let plugin_id = admitted.plugin_id().to_owned();
        match plugin_id.as_str() {
            "basic-economy" => {
                let (target, zone) = admitted
                    .into_upsert_zone()
                    .expect("consume economy market zone");
                assert_eq!(zone.id(), "economy-market");
                boundary
                    .try_enqueue_event(
                        target
                            .zone_command_result(zone.id(), true)
                            .expect("accepted economy zone result"),
                    )
                    .expect("deliver economy zone result");
                economy_zone_registered = true;
            }
            "land-claims" => {
                assert!(matches!(
                    admitted.request(),
                    ScriptCommand::PluginStorageGet { request }
                        if request.key() == "claims:v1"
                ));
                boundary
                    .try_enqueue_event(
                        admitted
                            .plugin_storage_get_result(None, None)
                            .expect("empty claim storage result"),
                    )
                    .expect("deliver empty claim storage result");
                claims_loaded = true;
            }
            other => panic!("unexpected startup plugin {other}"),
        }
    }
    assert!(economy_zone_registered);
    assert!(claims_loaded);

    let player_id = ScriptPlayerId::new(7);
    let context = ScriptPlayerContext::new(
        "12345678-1234-5678-1234-567812345678",
        "ClaimOwner",
        false,
        1.0,
        64.0,
        1.0,
    );
    assert_eq!(
        boundary
            .try_enqueue_player_command_with_context(player_id, context.clone(), "claim create")
            .expect("enqueue claim command"),
        PlayerCommandAdmission::Enqueued
    );
    let save = boundary.recv_command().await.expect("claim save command");
    let admitted = boundary
        .accept_host_command(save)
        .expect("admit claim save");
    assert_eq!(admitted.plugin_id(), "land-claims");
    assert!(matches!(
        admitted.request(),
        ScriptCommand::PluginStorageCompareAndSwap { request }
            if request.key() == "claims:v1" && request.expected_version().is_none()
    ));
    boundary
        .try_enqueue_event(
            admitted
                .plugin_storage_cas_result(true, Some(1))
                .expect("claim storage commit result"),
        )
        .expect("deliver claim storage commit result");

    let first = boundary.recv_command().await.expect("claim zone command");
    let first = boundary
        .accept_host_command(first)
        .expect("admit claim zone");
    assert!(matches!(
        first.request(),
        ScriptCommand::UpsertZone { zone }
            if zone.id() == "claim-12345678123456781234567812345678-p0-p0"
                && zone.dimension() == "minecraft:overworld"
    ));
    let (claim_target, claim_zone) = first.into_upsert_zone().expect("consume claim zone");
    boundary
        .try_enqueue_event(
            claim_target
                .zone_command_result(claim_zone.id(), true)
                .expect("accepted claim zone result"),
        )
        .expect("deliver accepted claim zone result");
    let second = boundary
        .recv_command()
        .await
        .expect("claim success message");
    let second = boundary
        .accept_host_command(second)
        .expect("admit claim message");
    assert!(matches!(
        second.request(),
        ScriptCommand::SendChatMessage { player_id: target, message }
            if *target == player_id && message.contains("Chunk claimed")
    ));

    assert_eq!(
        boundary
            .try_enqueue_player_command_with_context(player_id, context.clone(), "economy")
            .expect("enqueue economy command"),
        PlayerCommandAdmission::Enqueued
    );
    let ledger = boundary.recv_command().await.expect("ledger read command");
    let ledger = boundary
        .accept_host_command(ledger)
        .expect("admit ledger read");
    assert_eq!(ledger.plugin_id(), "basic-economy");
    assert!(matches!(
        ledger.request(),
        ScriptCommand::PluginStorageGet { request }
            if request.key() == "shop:economy:12345678-1234-5678-1234-567812345678"
    ));
    boundary
        .try_enqueue_event(
            ledger
                .plugin_storage_get_result(None, None)
                .expect("new economy ledger result"),
        )
        .expect("deliver economy ledger result");
    let menu = boundary.recv_command().await.expect("economy menu command");
    let menu = boundary
        .accept_host_command(menu)
        .expect("admit economy menu");
    assert_eq!(menu.plugin_id(), "basic-economy");
    assert!(matches!(
        menu.request(),
        ScriptCommand::OpenInventoryMenu { player_id: target, menu }
            if *target == player_id
                && menu.title() == "Market - Emeralds"
                && menu.slots()[0].item().resource_id() == "minecraft:apple"
                && menu.slots()[0].item().label()
                    == Some("Apples | buy 3 Emeralds | refund | owned 0")
    ));

    assert_eq!(
        boundary
            .try_enqueue_player_command_with_context(player_id, context, "claim remove")
            .expect("enqueue claim removal"),
        PlayerCommandAdmission::Enqueued
    );
    let removal_save = boundary.recv_command().await.expect("claim removal save");
    let removal_save = boundary
        .accept_host_command(removal_save)
        .expect("admit claim removal save");
    assert!(matches!(
        removal_save.request(),
        ScriptCommand::PluginStorageCompareAndSwap { request }
            if request.key() == "claims:v1" && request.expected_version() == Some(1)
    ));
    boundary
        .try_enqueue_event(
            removal_save
                .plugin_storage_cas_result(true, Some(2))
                .expect("claim removal storage result"),
        )
        .expect("deliver claim removal storage result");
    let removal = boundary.recv_command().await.expect("claim zone removal");
    let removal = boundary
        .accept_host_command(removal)
        .expect("admit claim zone removal");
    let (claim_target, zone_id) = removal.into_remove_zone().expect("consume zone removal");
    boundary
        .try_enqueue_event(
            claim_target
                .zone_command_result(&zone_id, false)
                .expect("rejected zone removal result"),
        )
        .expect("deliver rejected zone removal result");
    let rollback = boundary.recv_command().await.expect("claim rollback save");
    let rollback = boundary
        .accept_host_command(rollback)
        .expect("admit claim rollback save");
    assert!(matches!(
        rollback.request(),
        ScriptCommand::PluginStorageCompareAndSwap { request }
            if request.key() == "claims:v1"
                && request.expected_version() == Some(2)
                && request.value().contains("12345678123456781234567812345678")
    ));
    boundary
        .try_enqueue_event(
            rollback
                .plugin_storage_cas_result(true, Some(3))
                .expect("claim rollback result"),
        )
        .expect("deliver claim rollback result");
    let rollback_message = boundary
        .recv_command()
        .await
        .expect("claim rollback message");
    let rollback_message = boundary
        .accept_host_command(rollback_message)
        .expect("admit claim rollback message");
    assert!(matches!(
        rollback_message.request(),
        ScriptCommand::SendChatMessage { player_id: target, message }
            if *target == player_id
                && message == "Claim protection was unavailable; no claim change was kept."
    ));

    drop(boundary);
    tokio::task::spawn_blocking(move || host.join())
        .await
        .expect("Lua host join task")
        .expect("Lua host thread");
}

#[tokio::test]
async fn shipped_basic_economy_retains_refund_terms_and_bounds_purchase_count() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    copy_example_plugin("basic-economy", plugins.path());
    let config_path = plugins.path().join("basic-economy/config.toml");
    let mut config: toml::Table =
        toml::from_str(&std::fs::read_to_string(&config_path).expect("read economy config"))
            .expect("parse economy config");
    config
        .get_mut("catalog")
        .and_then(toml::Value::as_array_mut)
        .and_then(|catalog| catalog.first_mut())
        .and_then(toml::Value::as_table_mut)
        .expect("first economy product")
        .insert("price".to_owned(), toml::Value::Integer(4));
    std::fs::write(
        &config_path,
        toml::to_string_pretty(&config).expect("encode changed economy config"),
    )
    .expect("write changed economy config");

    let (boundary, host) = mc_script::start_lua_host(mc_script::LuaHostConfig::new(plugins.path()))
        .expect("start changed economy plugin");
    let context = ScriptPlayerContext::new(
        "12345678-1234-5678-1234-567812345678",
        "EconomyPlayer",
        false,
        1.0,
        64.0,
        1.0,
    );

    let old_terms = "v2|apples,minecraft:apple,2,3,minecraft:emerald,1";
    let (target, player_id, menu) =
        open_economy_menu_from_ledger(&boundary, 7, context.clone(), old_terms, 7).await;
    assert_eq!(
        menu.slots()[0].item().label(),
        Some("Apples | buy 4 Emeralds | changed terms: refund only | refund | owned 1")
    );
    boundary
        .try_enqueue_event(
            target
                .inventory_menu_clicked(
                    player_id,
                    context.clone(),
                    &menu,
                    0,
                    ScriptInventoryClick::Secondary,
                )
                .expect("build legacy refund click"),
        )
        .expect("enqueue legacy refund click");
    let mut transaction = None;
    for _ in 0..2 {
        let command = boundary.recv_command().await.expect("refund command");
        let admitted = boundary
            .accept_host_command(command)
            .expect("admit refund command");
        if let ScriptCommand::InventoryStorageTransaction {
            transaction: candidate,
        } = admitted.into_request()
        {
            transaction = Some(candidate);
        }
    }
    let transaction = transaction.expect("legacy refund transaction");
    assert!(
        transaction
            .inventory()
            .iter()
            .any(|delta| { delta.resource_id() == "minecraft:emerald" && delta.delta() == 3 })
    );
    assert!(
        transaction
            .inventory()
            .iter()
            .any(|delta| { delta.resource_id() == "minecraft:apple" && delta.delta() == -2 })
    );
    assert!(matches!(
        &transaction.storage()[0],
        ScriptStorageMutation::CompareAndSwap {
            expected_version: Some(7),
            value,
            ..
        } if value == "v2|"
    ));

    let maxed = "v2|apples,minecraft:apple,2,4,minecraft:emerald,999999";
    let (target, player_id, menu) =
        open_economy_menu_from_ledger(&boundary, 8, context.clone(), maxed, 8).await;
    boundary
        .try_enqueue_event(
            target
                .inventory_menu_clicked(
                    player_id,
                    context.clone(),
                    &menu,
                    0,
                    ScriptInventoryClick::Primary,
                )
                .expect("build maxed purchase click"),
        )
        .expect("enqueue maxed purchase click");
    let limit = boundary
        .recv_command()
        .await
        .expect("purchase limit message");
    let limit = boundary
        .accept_host_command(limit)
        .expect("admit purchase limit message");
    assert!(matches!(
        limit.request(),
        ScriptCommand::SendChatMessage { player_id: target, message }
            if *target == player_id && message == "Purchase limit reached for this product."
    ));

    let corrupt_player = ScriptPlayerId::new(9);
    boundary
        .try_enqueue_player_command_with_context(corrupt_player, context.clone(), "economy")
        .expect("enqueue corrupt-ledger economy command");
    let read = boundary.recv_command().await.expect("corrupt ledger read");
    let read = boundary
        .accept_host_command(read)
        .expect("admit corrupt ledger read");
    boundary
        .try_enqueue_event(
            read.plugin_storage_get_result(Some("v1|1"), Some(9))
                .expect("corrupt ledger result"),
        )
        .expect("deliver corrupt ledger result");
    let corrupt = boundary
        .recv_command()
        .await
        .expect("corrupt ledger message");
    let corrupt = boundary
        .accept_host_command(corrupt)
        .expect("admit corrupt ledger message");
    assert!(matches!(
        corrupt.request(),
        ScriptCommand::SendChatMessage { player_id: target, message }
            if *target == corrupt_player
                && message == "Economy unavailable: invalid ledger record."
    ));
    assert_eq!(
        boundary
            .try_enqueue_player_command_with_context(corrupt_player, context, "economy")
            .expect("retry economy after corrupt ledger"),
        PlayerCommandAdmission::Enqueued
    );
    let retry = boundary.recv_command().await.expect("retry ledger read");
    let retry = boundary
        .accept_host_command(retry)
        .expect("admit retry ledger read");
    assert!(matches!(
        retry.request(),
        ScriptCommand::PluginStorageGet { .. }
    ));

    drop(boundary);
    tokio::task::spawn_blocking(move || host.join())
        .await
        .expect("Lua host join task")
        .expect("Lua host thread");
}

#[tokio::test]
async fn shipped_land_claim_blocks_stranger_break_and_placement_over_wire() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    copy_example_plugin("land-claims", plugins.path());
    write_dirt_fixture_plugin(plugins.path());
    let (boundary, host) = mc_script::start_lua_host(mc_script::LuaHostConfig::new(plugins.path()))
        .expect("start land claims plugin");
    assert_eq!(host.loaded_plugins(), 2);

    let world_dir = tempfile::tempdir().expect("disk-backed world tempdir");
    std::fs::create_dir_all(world_dir.path().join("region")).expect("create world region");
    let block_report = mc_data::blocks::solaris_required_blocks_report();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&block_report).expect("embedded block registry"),
    );
    let short_grass = blocks
        .block(&mc_data::Identifier::parse("minecraft:short_grass").unwrap())
        .unwrap()
        .default;
    let stone = blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default;
    let air = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .unwrap()
        .default;
    let items = Arc::new(mc_data::items::solaris_required_items());
    let dirt_item_id = item_id(&items, "minecraft:dirt");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let fixture_y = generator.surface_height(0, 0) + 1;
    let target = mc_world::BlockPos {
        x: 0,
        y: fixture_y,
        z: 0,
    };
    let base = mc_world::BlockPos {
        x: 1,
        y: fixture_y - 1,
        z: 0,
    };
    let placement = mc_world::BlockPos {
        x: base.x,
        y: base.y + 1,
        z: base.z,
    };
    let mut world =
        mc_world::WorldStorage::open_with_capacity(world_dir.path(), Arc::clone(&blocks), 49)
            .expect("open disk-backed world")
            .with_item_registry(Arc::clone(&items))
            .with_generator(generator);
    world
        .set_block_at(target, short_grass)
        .expect("seed protected short grass before publication");
    world
        .set_block_at(base, stone)
        .expect("seed placement base before publication");
    world
        .set_block_at(placement, air)
        .expect("seed protected air before publication");
    let world = Arc::new(tokio::sync::Mutex::new(world));
    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Land claim wire test".into(),
        max_players: 3,
        view_distance: 1,
        data: Arc::new(mc_data::solaris_required_data()),
        blocks: Arc::clone(&blocks),
        world: Some(Arc::clone(&world)),
        tags: Arc::new(mc_data::tags::solaris_required_item_tags(&items)),
        recipes: Arc::new(mc_data::recipes::solaris_required_recipes()),
        loot: Arc::new(mc_data::loot::builtin().clone()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::solaris_required_item_facts()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &block_report,
        )),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::solaris_required_biome_spawn_rules()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(["ClaimAdmin"], true),
        shutdown: shutdown.clone(),
    };
    let bound = mc_net::bind_with_scripts(cfg, boundary)
        .await
        .expect("bind claimed server");
    let addr = bound.local_addr().expect("local address");
    let server = tokio::spawn(async move { bound.serve().await });

    let mut owner = Client::connect(addr).await.expect("owner connect");
    owner
        .drive_login(addr, "ClaimOwner")
        .await
        .expect("owner login");
    owner
        .drive_configuration()
        .await
        .expect("owner configuration");
    owner.read_play_login().await.expect("owner play entry");
    let _: ClientboundCommands = owner.read_typed().await.expect("owner Commands");
    let owner_sync: SynchronizePlayerPosition = owner.read_typed().await.expect("owner sync");
    owner
        .write_packet(&ConfirmTeleportation {
            teleport_id: owner_sync.teleport_id,
        })
        .await
        .expect("ack owner teleport");
    owner
        .write_packet(&ServerboundMovePlayerStatusOnly {
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("owner grounded");
    claim_current_chunk(&mut owner).await;

    let mut stranger = Client::connect(addr).await.expect("stranger connect");
    stranger
        .drive_login(addr, "ClaimStranger")
        .await
        .expect("stranger login");
    stranger
        .drive_configuration()
        .await
        .expect("stranger configuration");
    stranger
        .read_play_login()
        .await
        .expect("stranger play entry");
    let _: ClientboundCommands = stranger.read_typed().await.expect("stranger Commands");
    let stranger_sync: SynchronizePlayerPosition =
        stranger.read_typed().await.expect("stranger sync");
    stranger
        .write_packet(&ConfirmTeleportation {
            teleport_id: stranger_sync.teleport_id,
        })
        .await
        .expect("ack stranger teleport");
    stranger
        .write_packet(&ServerboundMovePlayerStatusOnly {
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("stranger grounded");
    send_command(&mut stranger, "fixture-dirt").await;
    let dirt_grant = wait_for_message_and_inventory(&mut stranger, "fixture-dirt-ready").await;
    assert_eq!(total_count(&dirt_grant, dirt_item_id), 1);

    stranger
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: pack_block_pos(target.x, target.y, target.z),
            direction: Direction::Up,
            sequence: 41,
        })
        .await
        .expect("attempt protected break");
    wait_for_block_ack(&mut stranger, 41).await;
    assert_eq!(
        world
            .lock()
            .await
            .get_block(target)
            .expect("read protected short grass"),
        Some(short_grass)
    );

    stranger
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(base.x, base.y, base.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 42,
        })
        .await
        .expect("attempt protected placement");
    wait_for_block_ack(&mut stranger, 42).await;
    assert_eq!(
        world
            .lock()
            .await
            .get_block(placement)
            .expect("read protected air"),
        Some(air)
    );

    drop(stranger);
    drop(owner);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server shutdown timeout")
        .expect("server task")
        .expect("server result");
    tokio::task::spawn_blocking(move || host.join())
        .await
        .expect("Lua host join task")
        .expect("Lua host thread");
}

#[tokio::test]
async fn shipped_inventory_plugins_work_over_wire() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    copy_example_plugin("basic-economy", plugins.path());
    copy_example_plugin("online-roster", plugins.path());
    let config_path = plugins.path().join("basic-economy/config.toml");
    let mut config: toml::Table =
        toml::from_str(&std::fs::read_to_string(&config_path).expect("read catalog config"))
            .expect("parse catalog config");
    let currency = config
        .get_mut("currency")
        .and_then(toml::Value::as_table_mut)
        .expect("currency config table");
    currency.insert(
        "resource".to_owned(),
        toml::Value::String("minecraft:gold_ingot".to_owned()),
    );
    currency.insert(
        "singular".to_owned(),
        toml::Value::String("Gold Ingot".to_owned()),
    );
    currency.insert(
        "plural".to_owned(),
        toml::Value::String("Gold Ingots".to_owned()),
    );
    let product = config
        .get_mut("catalog")
        .and_then(toml::Value::as_array_mut)
        .and_then(|catalog| catalog.first_mut())
        .and_then(toml::Value::as_table_mut)
        .expect("first catalog product");
    product.insert(
        "resource".to_owned(),
        toml::Value::String("minecraft:stone_axe".to_owned()),
    );
    product.insert("count".to_owned(), toml::Value::Integer(1));
    product.insert(
        "label".to_owned(),
        toml::Value::String("Stone Axe".to_owned()),
    );
    product.insert("price".to_owned(), toml::Value::Integer(2));
    let zone = config
        .get_mut("zone")
        .and_then(toml::Value::as_table_mut)
        .expect("zone config table");
    for bound in ["minimum", "maximum"] {
        let coordinates = zone
            .get_mut(bound)
            .and_then(toml::Value::as_table_mut)
            .expect("zone coordinate table");
        let value = if bound == "minimum" { 24 } else { 40 };
        coordinates.insert("x".to_owned(), toml::Value::Integer(value));
        coordinates.insert("z".to_owned(), toml::Value::Integer(value));
    }
    std::fs::write(
        &config_path,
        toml::to_string_pretty(&config).expect("encode catalog config"),
    )
    .expect("write customized catalog config");

    let currency_resource = "minecraft:gold_ingot";
    let currency_plural = "Gold Ingots";
    let product_resource = "minecraft:stone_axe";
    let product_label = "Stone Axe";
    let product_count = 1;
    let product_price = 2;
    let (boundary, host) = mc_script::start_lua_host(mc_script::LuaHostConfig::new(plugins.path()))
        .expect("start shipped item economy");
    assert_eq!(host.loaded_plugins(), 2);

    let world_dir = tempfile::tempdir().expect("disk-backed world tempdir");
    std::fs::create_dir_all(world_dir.path().join("region")).expect("create world region");
    let block_report = mc_data::blocks::solaris_required_blocks_report();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&block_report).expect("embedded block registry"),
    );
    let items = Arc::new(mc_data::items::solaris_required_items());
    let currency_id = item_id(&items, currency_resource);
    let product_id = item_id(&items, product_resource);
    let roster_item_id = item_id(&items, "minecraft:paper");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let world =
        mc_world::WorldStorage::open_with_capacity(world_dir.path(), Arc::clone(&blocks), 49)
            .expect("open disk-backed world")
            .with_item_registry(Arc::clone(&items))
            .with_generator(generator);
    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Shipped item economy wire test".into(),
        max_players: 1,
        view_distance: 1,
        data: Arc::new(mc_data::solaris_required_data()),
        blocks,
        world: Some(Arc::new(tokio::sync::Mutex::new(world))),
        tags: Arc::new(mc_data::tags::solaris_required_item_tags(&items)),
        recipes: Arc::new(mc_data::recipes::solaris_required_recipes()),
        loot: Arc::new(mc_data::loot::builtin().clone()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::solaris_required_item_facts()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &block_report,
        )),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::solaris_required_biome_spawn_rules()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: shutdown.clone(),
    };
    let bound = mc_net::bind_with_scripts(cfg, boundary)
        .await
        .expect("bind scripted server");
    let addr = bound.local_addr().expect("local address");
    let server = tokio::spawn(async move { bound.serve().await });

    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client
        .drive_login(addr, "CatalogPlayer")
        .await
        .expect("login");
    client.drive_configuration().await.expect("configuration");
    let _ = client.read_play_login().await.expect("play entry");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");
    client
        .write_packet(&ServerboundMovePlayerStatusOnly {
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("report grounded spawn pose");
    client
        .write_packet(&ServerboundChatCommand {
            command: format!("give {currency_resource} {product_price}"),
        })
        .await
        .expect("give configured catalog currency");
    wait_for_slot(&mut client, currency_id, product_price).await;

    client
        .write_packet(&ServerboundMovePlayerPos {
            x: 20.0,
            y: sync.y,
            z: 20.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move outside catalog zone");
    client
        .write_packet(&ServerboundMovePlayerPos {
            x: 32.0,
            y: sync.y,
            z: 32.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("enter catalog zone");

    let initial_menu = wait_for_catalog_menu(
        &mut client,
        product_id,
        product_count,
        product_label,
        product_price,
        currency_plural,
        0,
    )
    .await;
    click_catalog_product(&mut client, &initial_menu, 0).await;
    let purchased = wait_for_message_and_inventory(&mut client, "Purchased Stone Axe.").await;
    assert_inventory(&purchased, currency_id, 0, product_id, product_count);

    let owned_menu = wait_for_catalog_menu(
        &mut client,
        product_id,
        product_count,
        product_label,
        product_price,
        currency_plural,
        1,
    )
    .await;
    click_catalog_product(&mut client, &owned_menu, 0).await;
    let rejected = wait_for_message_and_optional_inventory(
        &mut client,
        "Transaction rejected: inventory or storage precondition changed.",
    )
    .await;
    if let Some(inventory) = rejected {
        assert_inventory(&inventory, currency_id, 0, product_id, product_count);
    }

    let unchanged_menu = wait_for_catalog_menu(
        &mut client,
        product_id,
        product_count,
        product_label,
        product_price,
        currency_plural,
        1,
    )
    .await;
    click_catalog_product(&mut client, &unchanged_menu, 1).await;
    let refunded = wait_for_message_and_inventory(&mut client, "Refunded Stone Axe.").await;
    assert_inventory(&refunded, currency_id, product_price, product_id, 0);

    let final_menu = wait_for_catalog_menu(
        &mut client,
        product_id,
        product_count,
        product_label,
        product_price,
        currency_plural,
        0,
    )
    .await;
    assert_eq!(final_menu.product_count, product_count);

    client
        .write_packet(&ServerboundMovePlayerPos {
            x: 20.0,
            y: sync.y,
            z: 20.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("leave catalog zone before command entry");
    send_command(&mut client, "economy").await;
    let command_menu = wait_for_catalog_menu(
        &mut client,
        product_id,
        product_count,
        product_label,
        product_price,
        currency_plural,
        0,
    )
    .await;
    assert_eq!(command_menu.product_count, product_count);

    send_command(&mut client, "who").await;
    wait_for_roster_menu(&mut client, roster_item_id, "CatalogPlayer").await;

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server shutdown timeout")
        .expect("server task")
        .expect("server result");
    tokio::task::spawn_blocking(move || host.join())
        .await
        .expect("Lua host join task")
        .expect("Lua host thread");
}

#[tokio::test]
async fn shipped_colony_scaffold_recruits_and_applies_updated_order_over_wire() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    copy_example_plugin("colony-villager-scaffold", plugins.path());
    write_villager_fixture_plugin(plugins.path());
    let (boundary, host) = mc_script::start_lua_host(mc_script::LuaHostConfig::new(plugins.path()))
        .expect("start shipped colony scaffold");
    assert_eq!(host.loaded_plugins(), 2);

    let world_dir = tempfile::tempdir().expect("disk-backed world tempdir");
    std::fs::create_dir_all(world_dir.path().join("region")).expect("create world region");
    let block_report = mc_data::blocks::solaris_required_blocks_report();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&block_report).expect("embedded block registry"),
    );
    let items = Arc::new(mc_data::items::solaris_required_items());
    let stone_axe_id = item_id(&items, "minecraft:stone_axe");
    let entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    let villager_type_id = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("embedded villager entity type");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let world =
        mc_world::WorldStorage::open_with_capacity(world_dir.path(), Arc::clone(&blocks), 49)
            .expect("open disk-backed world")
            .with_item_registry(Arc::clone(&items))
            .with_generator(generator);
    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Shipped colony scaffold wire test".into(),
        max_players: 1,
        view_distance: 1,
        data: Arc::new(mc_data::solaris_required_data()),
        blocks,
        world: Some(Arc::new(tokio::sync::Mutex::new(world))),
        tags: Arc::new(mc_data::tags::solaris_required_item_tags(&items)),
        recipes: Arc::new(mc_data::recipes::solaris_required_recipes()),
        loot: Arc::new(mc_data::loot::builtin().clone()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::solaris_required_item_facts()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &block_report,
        )),
        entity_types,
        biome_spawns: Arc::new(mc_data::biomes::solaris_required_biome_spawn_rules()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: shutdown.clone(),
    };
    let bound = mc_net::bind_with_scripts(cfg, boundary)
        .await
        .expect("bind scripted server");
    let addr = bound.local_addr().expect("local address");
    let server = tokio::spawn(async move { bound.serve().await });

    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client
        .drive_login(addr, "ColonyPlayer")
        .await
        .expect("login");
    client.drive_configuration().await.expect("configuration");
    let _ = client.read_play_login().await.expect("play entry");
    let commands: ClientboundCommands = client.read_typed().await.expect("Commands");
    let roots = commands.nodes[commands.root_index as usize]
        .children
        .iter()
        .filter_map(|index| match &commands.nodes[*index as usize].kind {
            CommandNodeKind::Literal(root) => Some(root.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(roots.contains(&"colony"));

    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");
    client
        .write_packet(&ServerboundMovePlayerStatusOnly {
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("report grounded spawn pose");
    let villager_entity_id = wait_for_colony_startup(
        &mut client,
        villager_type_id,
        (sync.x + 1.0, sync.y, sync.z),
    )
    .await;

    send_command(&mut client, "colony status").await;
    wait_for_system_chat(&mut client, "No villager is recruited for this player.").await;

    send_command(&mut client, "colony recruit worker").await;
    wait_for_system_chat(&mut client, "Villager recruitment recorded durably.").await;
    send_command(&mut client, "colony status").await;
    wait_for_system_chat(
        &mut client,
        "Recruited villager: role metadata=worker, stored order intent=home.",
    )
    .await;

    send_command(&mut client, "colony order hold").await;
    wait_for_system_chat(&mut client, "Applied villager order hold.").await;
    send_command(&mut client, "colony status").await;
    wait_for_system_chat(
        &mut client,
        "Recruited villager: role metadata=worker, stored order intent=hold.",
    )
    .await;

    send_command(&mut client, "debug give minecraft:stone_axe 1 0").await;
    wait_for_slot(&mut client, stone_axe_id, 1).await;
    for attack_index in 0..3 {
        client
            .write_packet(&ServerboundAttack {
                entity_id: villager_entity_id,
            })
            .await
            .expect("attack the bound villager");
        if attack_index < 2 {
            send_command(&mut client, "fixture-attack-cooldown").await;
            wait_for_system_chat(&mut client, "fixture-attack-ready").await;
        }
    }
    wait_for_entity_removal(&mut client, villager_entity_id).await;

    send_command(&mut client, "colony order home").await;
    wait_for_system_chat(
        &mut client,
        "Stored order intent, but no villager was available to apply it.",
    )
    .await;

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server shutdown timeout")
        .expect("server task")
        .expect("server result");
    tokio::task::spawn_blocking(move || host.join())
        .await
        .expect("Lua host join task")
        .expect("Lua host thread");
}

fn copy_example_plugin(name: &str, destination_root: &Path) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/plugins")
        .join(name);
    let destination = destination_root.join(name);
    std::fs::create_dir(&destination).expect("create copied example plugin directory");
    for file in ["plugin.toml", "main.lua"] {
        std::fs::copy(source.join(file), destination.join(file))
            .unwrap_or_else(|error| panic!("copy shipped {name}/{file}: {error}"));
    }
    let config = source.join("config.toml");
    if config.is_file() {
        std::fs::copy(config, destination.join("config.toml"))
            .unwrap_or_else(|error| panic!("copy shipped {name}/config.toml: {error}"));
    }
}

async fn open_economy_menu_from_ledger(
    boundary: &mc_script::ScriptBoundary,
    player_value: u64,
    context: ScriptPlayerContext,
    ledger: &str,
    version: u64,
) -> (
    mc_script::ScriptPluginTarget,
    ScriptPlayerId,
    mc_script::ScriptInventoryMenu,
) {
    let player_id = ScriptPlayerId::new(player_value);
    assert_eq!(
        boundary
            .try_enqueue_player_command_with_context(player_id, context, "economy")
            .expect("enqueue economy command"),
        PlayerCommandAdmission::Enqueued
    );
    let read = boundary.recv_command().await.expect("economy ledger read");
    let read = boundary
        .accept_host_command(read)
        .expect("admit economy ledger read");
    boundary
        .try_enqueue_event(
            read.plugin_storage_get_result(Some(ledger), Some(version))
                .expect("economy ledger result"),
        )
        .expect("deliver economy ledger result");
    boundary
        .accept_host_command(boundary.recv_command().await.expect("economy menu command"))
        .expect("admit economy menu")
        .into_open_inventory_menu()
        .expect("consume economy menu")
}

fn write_dirt_fixture_plugin(destination_root: &Path) {
    let destination = destination_root.join("dirt-fixture");
    std::fs::create_dir(&destination).expect("create dirt fixture plugin directory");
    std::fs::write(
        destination.join("plugin.toml"),
        r#"
            id = "dirt-fixture"
            name = "Dirt Fixture"
            version = "0.1.0"
            api = "0.6.0"
            capabilities = ["player_inventory"]
            player_commands = ["fixture-dirt"]
        "#,
    )
    .expect("write dirt fixture manifest");
    std::fs::write(
        destination.join("main.lua"),
        r#"
            function on_player_command(event)
                solaris.inventory_transaction(event.player_id, "dirt-grant", {
                    { resource = "minecraft:dirt", delta = 1 },
                })
            end

            function on_player_inventory_transaction_result(event)
                if event.request_id == "dirt-grant" and event.committed then
                    solaris.send_message(event.player_id, "fixture-dirt-ready")
                end
            end
        "#,
    )
    .expect("write dirt fixture source");
}

fn write_villager_fixture_plugin(destination_root: &Path) {
    let destination = destination_root.join("villager-fixture");
    std::fs::create_dir(&destination).expect("create villager fixture plugin directory");
    std::fs::write(
        destination.join("plugin.toml"),
        r#"
            id = "villager-fixture"
            name = "Villager Fixture"
            version = "0.1.0"
            api = "0.6.0"
            events = ["player.joined", "server.tick"]
            player_commands = ["fixture-attack-cooldown"]
            spawn_entities = ["minecraft:villager"]
        "#,
    )
    .expect("write villager fixture manifest");
    std::fs::write(
        destination.join("main.lua"),
        r#"
            local last_tick = 0
            local cooldown_player = nil
            local cooldown_target = nil

            function on_player_joined(event)
                solaris.spawn_entity(
                    event.player_id,
                    "minecraft:villager",
                    event.x + 1,
                    event.y,
                    event.z
                )
                solaris.send_message(event.player_id, "fixture-villager-ready")
            end

            function on_player_command(event)
                cooldown_player = event.player_id
                cooldown_target = last_tick + 7
            end

            function on_server_tick(event)
                last_tick = event.tick
                if cooldown_target ~= nil and event.tick >= cooldown_target then
                    solaris.send_message(cooldown_player, "fixture-attack-ready")
                    cooldown_player = nil
                    cooldown_target = nil
                end
            end
        "#,
    )
    .expect("write villager fixture source");
}

fn item_id(items: &mc_data::items::ItemRegistry, resource: &str) -> u32 {
    items
        .id_of(&mc_data::Identifier::parse(resource).expect("checked item identifier"))
        .unwrap_or_else(|| panic!("missing embedded item {resource}"))
}

async fn wait_for_slot(client: &mut Client, item_id: u32, count: i32) {
    loop {
        let outcome = client
            .wait_for_frame_id_with_timeout_and_limits(
                ClientboundContainerSetSlot::ID,
                Duration::from_secs(5),
                FRAME_LIMITS,
            )
            .await
            .expect("inventory slot update");
        let packet = ClientboundContainerSetSlot::decode(&mut outcome.frame.body.clone())
            .expect("decode inventory slot update");
        if packet.container_id == 0
            && packet.item_stack.item_id == item_id
            && packet.item_stack.count == count
        {
            return;
        }
    }
}

async fn wait_for_catalog_menu(
    client: &mut Client,
    product_id: u32,
    product_count: i32,
    product_label: &str,
    product_price: i32,
    currency_plural: &str,
    owned: usize,
) -> CatalogMenu {
    let open = client
        .wait_for_frame_id_with_timeout_and_limits(
            ClientboundOpenScreen::ID,
            Duration::from_secs(5),
            FRAME_LIMITS,
        )
        .await
        .expect("catalog open frame");
    let screen = ClientboundOpenScreen::decode(&mut open.frame.body.clone())
        .expect("decode catalog OpenScreen");
    assert_eq!(screen.menu_type, 0);
    assert_eq!(
        literal_text_component_text(&screen.title_nbt),
        format!("Market - {currency_plural}")
    );

    let content = loop {
        let outcome = client
            .wait_for_frame_id_with_timeout_and_limits(
                ClientboundContainerSetContent::ID,
                Duration::from_secs(5),
                FRAME_LIMITS,
            )
            .await
            .expect("catalog content frame");
        let content = ClientboundContainerSetContent::decode(&mut outcome.frame.body.clone())
            .expect("decode catalog content");
        if content.container_id == screen.container_id {
            break content;
        }
    };
    assert_eq!(content.items.len(), 45);
    assert_eq!(content.items[0].item_id, product_id);
    assert_eq!(content.items[0].count, product_count);
    assert_eq!(
        content.items[0].custom_name.as_deref(),
        Some(
            format!(
                "{product_label} | buy {product_price} {currency_plural} | refund | owned {owned}"
            )
            .as_str()
        )
    );
    CatalogMenu {
        container_id: content.container_id,
        state_id: content.state_id,
        product_id,
        product_count: content.items[0].count,
    }
}

async fn wait_for_roster_menu(client: &mut Client, item_id: u32, username: &str) {
    let open = client
        .wait_for_frame_id_with_timeout_and_limits(
            ClientboundOpenScreen::ID,
            Duration::from_secs(5),
            FRAME_LIMITS,
        )
        .await
        .expect("online roster open frame");
    let screen = ClientboundOpenScreen::decode(&mut open.frame.body.clone())
        .expect("decode online roster OpenScreen");
    assert_eq!(screen.menu_type, 0);
    assert_eq!(
        literal_text_component_text(&screen.title_nbt),
        "Online Players (1)"
    );

    let content = loop {
        let outcome = client
            .wait_for_frame_id_with_timeout_and_limits(
                ClientboundContainerSetContent::ID,
                Duration::from_secs(5),
                FRAME_LIMITS,
            )
            .await
            .expect("online roster content frame");
        let content = ClientboundContainerSetContent::decode(&mut outcome.frame.body.clone())
            .expect("decode online roster content");
        if content.container_id == screen.container_id {
            break content;
        }
    };
    assert_eq!(content.items.len(), 45);
    assert_eq!(content.items[0].item_id, item_id);
    assert_eq!(content.items[0].count, 1);
    assert_eq!(
        content.items[0].custom_name.as_deref(),
        Some(format!("{username} | minecraft:overworld").as_str())
    );
}

async fn click_catalog_product(client: &mut Client, menu: &CatalogMenu, button_num: i8) {
    client
        .write_packet(&ServerboundContainerClick {
            container_id: menu.container_id,
            state_id: menu.state_id,
            slot_num: 0,
            button_num,
            container_input: ContainerInput::Pickup,
            changed_slots: vec![(0, HashedStack::empty())],
            carried_item: HashedStack::Actual {
                item_id: menu.product_id,
                count: menu.product_count,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("click catalog product");
}

async fn wait_for_message_and_inventory(
    client: &mut Client,
    expected: &str,
) -> ClientboundContainerSetContent {
    wait_for_message_and_optional_inventory(client, expected)
        .await
        .unwrap_or_else(|| {
            panic!("{expected:?} arrived before an authoritative inventory snapshot")
        })
}

async fn wait_for_message_and_optional_inventory(
    client: &mut Client,
    expected: &str,
) -> Option<ClientboundContainerSetContent> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut inventory = None;
        loop {
            let mut frame = client.read_frame().await.expect("catalog result frame");
            if frame.id == ClientboundContainerSetContent::ID {
                let content = ClientboundContainerSetContent::decode(&mut frame.body)
                    .expect("decode catalog inventory snapshot");
                if content.container_id == 0 {
                    inventory = Some(content);
                }
            } else if frame.id == ClientboundSystemChat::ID {
                let chat = ClientboundSystemChat::decode(&mut frame.body)
                    .expect("decode catalog result chat");
                if literal_text_component_text(&chat.content_nbt) == expected {
                    return inventory;
                }
            }
        }
    })
    .await
    .expect("catalog result timeout")
}

async fn send_command(client: &mut Client, command: &str) {
    client
        .write_packet(&ServerboundChatCommand {
            command: command.to_owned(),
        })
        .await
        .unwrap_or_else(|error| panic!("send /{command}: {error}"));
}

async fn claim_current_chunk(client: &mut Client) {
    send_command(client, "claim create").await;
    loop {
        let outcome = client
            .wait_for_frame_id_with_timeout_and_limits(
                ClientboundSystemChat::ID,
                Duration::from_secs(5),
                FRAME_LIMITS,
            )
            .await
            .expect("claim command result");
        let chat = ClientboundSystemChat::decode(&mut outcome.frame.body.clone())
            .expect("decode claim command result");
        match literal_text_component_text(&chat.content_nbt).as_str() {
            "Claims are still loading." => send_command(client, "claim create").await,
            "Chunk claimed. Breaking and placing are now protected." => return,
            message => panic!("unexpected claim result: {message}"),
        }
    }
}

async fn wait_for_block_ack(client: &mut Client, sequence: i32) {
    loop {
        let outcome = client
            .wait_for_frame_id_with_timeout_and_limits(
                BlockChangedAck::ID,
                Duration::from_secs(5),
                FRAME_LIMITS,
            )
            .await
            .expect("block acknowledgement");
        let ack = BlockChangedAck::decode(&mut outcome.frame.body.clone())
            .expect("decode block acknowledgement");
        if ack.sequence == sequence {
            return;
        }
    }
}

async fn wait_for_system_chat(client: &mut Client, expected: &str) {
    let mut seen = Vec::new();
    loop {
        let outcome = client
            .wait_for_frame_id_with_timeout_and_limits(
                ClientboundSystemChat::ID,
                Duration::from_secs(5),
                FRAME_LIMITS,
            )
            .await
            .unwrap_or_else(|error| panic!("wait for {expected:?}: {error}; seen {seen:?}"));
        let chat = ClientboundSystemChat::decode(&mut outcome.frame.body.clone())
            .expect("decode system chat");
        let message = literal_text_component_text(&chat.content_nbt);
        if message == expected {
            return;
        }
        seen.push(message);
    }
}

async fn wait_for_colony_startup(
    client: &mut Client,
    villager_type_id: i32,
    expected_position: (f64, f64, f64),
) -> i32 {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut fixture_ready = false;
        let mut colony_ready = false;
        let mut villager_entity_id = None;
        loop {
            let mut frame = client.read_frame().await.expect("colony startup frame");
            if frame.id == AddEntity::ID {
                let entity = AddEntity::decode(&mut frame.body).expect("decode villager spawn");
                if entity.entity_type_id == villager_type_id
                    && (entity.x - expected_position.0).abs() < 0.01
                    && (entity.y - expected_position.1).abs() < 0.01
                    && (entity.z - expected_position.2).abs() < 0.01
                {
                    villager_entity_id = Some(entity.entity_id);
                }
            } else if frame.id == ClientboundSystemChat::ID {
                let chat = ClientboundSystemChat::decode(&mut frame.body)
                    .expect("decode colony startup chat");
                match literal_text_component_text(&chat.content_nbt).as_str() {
                    "fixture-villager-ready" => fixture_ready = true,
                    "Colony plugin ready." => colony_ready = true,
                    _ => {}
                }
            }
            if fixture_ready
                && colony_ready
                && let Some(entity_id) = villager_entity_id
            {
                return entity_id;
            }
        }
    })
    .await
    .expect("colony startup readiness timeout")
}

async fn wait_for_entity_removal(client: &mut Client, entity_id: i32) {
    loop {
        let outcome = client
            .wait_for_frame_id_with_timeout_and_limits(
                RemoveEntities::ID,
                Duration::from_secs(5),
                FRAME_LIMITS,
            )
            .await
            .expect("bound villager removal");
        let removed = RemoveEntities::decode(&mut outcome.frame.body.clone())
            .expect("decode bound villager removal");
        if removed.entity_ids.contains(&entity_id) {
            return;
        }
    }
}

fn assert_inventory(
    inventory: &ClientboundContainerSetContent,
    currency_id: u32,
    currency_count: i32,
    product_id: u32,
    product_count: i32,
) {
    assert_eq!(inventory.container_id, 0);
    assert_eq!(total_count(inventory, currency_id), currency_count);
    assert_eq!(total_count(inventory, product_id), product_count);
}

fn total_count(inventory: &ClientboundContainerSetContent, item_id: u32) -> i32 {
    inventory
        .items
        .iter()
        .filter(|item| item.item_id == item_id)
        .map(|item| item.count)
        .sum()
}

fn literal_text_component_text(component: &[u8]) -> String {
    let mut bytes = Bytes::copy_from_slice(component);
    let tag = mc_nbt::read_network(&mut bytes).expect("read text component nbt");
    let mc_nbt::Tag::Compound(fields) = tag else {
        panic!("text component root must be a compound");
    };
    fields
        .into_iter()
        .find_map(|(name, value)| match (name.as_str(), value) {
            ("text", mc_nbt::Tag::String(text)) => Some(text),
            _ => None,
        })
        .expect("literal text component")
}
