use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, ClientboundCommands, ClientboundContainerSetContent, ClientboundContainerSetSlot,
    ClientboundOpenScreen, ClientboundSystemChat, CommandNodeKind, ConfirmTeleportation,
    ContainerInput, HashedStack, HashedStackComponentHashes, MovePlayerFlags, RemoveEntities,
    ServerboundAttack, ServerboundChatCommand, ServerboundContainerClick, ServerboundMovePlayerPos,
    ServerboundMovePlayerStatusOnly, SynchronizePlayerPosition,
};
use mc_test_harness::client::{Client, FrameWaitLimits};

const FRAME_LIMITS: FrameWaitLimits = FrameWaitLimits {
    max_skipped_frames: Some(4096),
    max_skipped_bytes: Some(32 * 1024 * 1024),
};

struct CatalogMenu {
    container_id: i32,
    state_id: i32,
    apple_id: u32,
    apple_count: i32,
}

#[tokio::test]
async fn shipped_currency_catalog_completes_buy_reject_and_refund_over_wire() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    copy_example_plugin("currency-catalog", plugins.path());
    let (boundary, host) = mc_script::start_lua_host(mc_script::LuaHostConfig::new(plugins.path()))
        .expect("start shipped currency catalog");
    assert_eq!(host.loaded_plugins(), 1);

    let world_dir = tempfile::tempdir().expect("disk-backed world tempdir");
    std::fs::create_dir_all(world_dir.path().join("region")).expect("create world region");
    let block_report = mc_data::blocks::solaris_required_blocks_report();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&block_report).expect("embedded block registry"),
    );
    let items = Arc::new(mc_data::items::solaris_required_items());
    let emerald_id = item_id(&items, "minecraft:emerald");
    let apple_id = item_id(&items, "minecraft:apple");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let world =
        mc_world::WorldStorage::open_with_capacity(world_dir.path(), Arc::clone(&blocks), 49)
            .expect("open disk-backed world")
            .with_item_registry(Arc::clone(&items))
            .with_generator(generator);
    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Shipped currency catalog wire test".into(),
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
    wait_for_system_chat(&mut client, "Currency Catalog ready.").await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "give minecraft:emerald 3".to_owned(),
        })
        .await
        .expect("give configured catalog currency");
    wait_for_slot(&mut client, emerald_id, 3).await;

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
            x: 0.0,
            y: sync.y,
            z: 0.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("enter catalog zone");

    let initial_menu = wait_for_catalog_menu(&mut client, apple_id, 0).await;
    click_catalog_apple(&mut client, &initial_menu, 0).await;
    let purchased = wait_for_message_and_inventory(&mut client, "Purchased Apples.").await;
    assert_inventory(&purchased, emerald_id, 0, apple_id, 2);

    let owned_menu = wait_for_catalog_menu(&mut client, apple_id, 1).await;
    click_catalog_apple(&mut client, &owned_menu, 0).await;
    let rejected = wait_for_message_and_optional_inventory(
        &mut client,
        "Transaction rejected: inventory or storage precondition changed.",
    )
    .await;
    if let Some(inventory) = rejected {
        assert_inventory(&inventory, emerald_id, 0, apple_id, 2);
    }

    let unchanged_menu = wait_for_catalog_menu(&mut client, apple_id, 1).await;
    click_catalog_apple(&mut client, &unchanged_menu, 1).await;
    let refunded = wait_for_message_and_inventory(&mut client, "Refunded Apples.").await;
    assert_inventory(&refunded, emerald_id, 3, apple_id, 0);

    let final_menu = wait_for_catalog_menu(&mut client, apple_id, 0).await;
    assert_eq!(final_menu.apple_count, 2);

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

async fn wait_for_catalog_menu(client: &mut Client, apple_id: u32, owned: usize) -> CatalogMenu {
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
        "Market - Emeralds"
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
    assert_eq!(content.items[0].item_id, apple_id);
    assert_eq!(content.items[0].count, 2);
    assert_eq!(
        content.items[0].custom_name.as_deref(),
        Some(format!("Apples | buy 3 Emeralds | refund | owned {owned}").as_str())
    );
    CatalogMenu {
        container_id: content.container_id,
        state_id: content.state_id,
        apple_id,
        apple_count: content.items[0].count,
    }
}

async fn click_catalog_apple(client: &mut Client, menu: &CatalogMenu, button_num: i8) {
    client
        .write_packet(&ServerboundContainerClick {
            container_id: menu.container_id,
            state_id: menu.state_id,
            slot_num: 0,
            button_num,
            container_input: ContainerInput::Pickup,
            changed_slots: vec![(0, HashedStack::empty())],
            carried_item: HashedStack::Actual {
                item_id: menu.apple_id,
                count: menu.apple_count,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("click catalog apple");
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
    emerald_id: u32,
    emerald_count: i32,
    apple_id: u32,
    apple_count: i32,
) {
    assert_eq!(inventory.container_id, 0);
    assert_eq!(total_count(inventory, emerald_id), emerald_count);
    assert_eq!(total_count(inventory, apple_id), apple_count);
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
