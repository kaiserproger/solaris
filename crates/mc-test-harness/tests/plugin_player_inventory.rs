use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundCommands, ClientboundContainerSetContent, ClientboundSystemChat,
    ConfirmTeleportation, ServerboundChatCommand, SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;

#[tokio::test]
async fn lua_player_inventory_transactions_are_atomic_authoritative_and_targeted() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    write_owner_plugin(plugins.path());
    write_observer_plugin(plugins.path());
    let (boundary, host) = mc_script::start_lua_host(mc_script::LuaHostConfig::new(plugins.path()))
        .expect("start Lua host");
    assert_eq!(host.loaded_plugins(), 2);

    let world_dir = tempfile::tempdir().expect("world tempdir");
    std::fs::create_dir_all(world_dir.path().join("region")).expect("create world region");
    let block_report = mc_data::blocks::solaris_required_blocks_report();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&block_report).expect("embedded block registry"),
    );
    let items = Arc::new(mc_data::items::solaris_required_items());
    let emerald = item_id(&items, "minecraft:emerald");
    let apple = item_id(&items, "minecraft:apple");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let world =
        mc_world::WorldStorage::open_with_capacity(world_dir.path(), Arc::clone(&blocks), 49)
            .expect("open world")
            .with_item_registry(Arc::clone(&items))
            .with_generator(generator);
    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Lua player inventory wire test".into(),
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
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), false),
        shutdown: shutdown.clone(),
    };
    let bound = mc_net::bind_with_scripts(cfg, boundary)
        .await
        .expect("bind scripted server");
    let addr = bound.local_addr().expect("local address");
    let server = tokio::spawn(async move { bound.serve().await });

    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client
        .drive_login(addr, "InventoryPlayer")
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

    send_command(&mut client, "kitgrant").await;
    wait_for_result_and_inventory(
        &mut client,
        "inventory:grant:true:nil",
        emerald,
        3,
        apple,
        0,
    )
    .await;

    send_command(&mut client, "kitexchange").await;
    wait_for_result_and_inventory(
        &mut client,
        "inventory:exchange:true:nil",
        emerald,
        1,
        apple,
        4,
    )
    .await;

    send_command(&mut client, "kitoverdraw").await;
    wait_for_result_without_inventory(
        &mut client,
        "inventory:overdraw:false:insufficient_resource",
    )
    .await;

    send_command(&mut client, "kitunknown").await;
    wait_for_result_without_inventory(&mut client, "inventory:unknown:false:unknown_resource")
        .await;

    send_command(&mut client, "kitclear").await;
    wait_for_result_and_inventory(
        &mut client,
        "inventory:clear:true:nil",
        emerald,
        0,
        apple,
        0,
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

#[tokio::test]
async fn worldless_runtime_rejects_player_inventory_before_mutation() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    write_owner_plugin(plugins.path());
    let (boundary, host) = mc_script::start_lua_host(mc_script::LuaHostConfig::new(plugins.path()))
        .expect("start Lua host");
    assert_eq!(host.loaded_plugins(), 1);

    let items = Arc::new(mc_data::items::solaris_required_items());
    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Worldless inventory rejection test".into(),
        max_players: 1,
        view_distance: 1,
        data: Arc::new(mc_data::solaris_required_data()),
        blocks: Arc::new(mc_world::BlockRegistry::from_report(&[]).unwrap()),
        world: None,
        tags: Arc::new(mc_data::tags::solaris_required_item_tags(&items)),
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::solaris_required_item_facts()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), false),
        shutdown: shutdown.clone(),
    };
    let bound = mc_net::bind_with_scripts(cfg, boundary)
        .await
        .expect("bind worldless scripted server");
    let addr = bound.local_addr().expect("local address");
    let server = tokio::spawn(async move { bound.serve().await });

    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client
        .drive_login(addr, "WorldlessInv")
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
        .wait_for_frame_id_with_timeout(ClientboundContainerSetContent::ID, Duration::from_secs(5))
        .await
        .expect("initial empty inventory");

    send_command(&mut client, "kitgrant").await;
    wait_for_result_without_inventory(&mut client, "inventory:grant:false:runtime_unavailable")
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

fn write_owner_plugin(root: &std::path::Path) {
    let owner = root.join("kits");
    std::fs::create_dir(&owner).expect("create owner plugin directory");
    std::fs::write(
        owner.join("plugin.toml"),
        r#"
            id = "kits"
            name = "Kits"
            version = "0.1.0"
            api = "0.6.0"
            events = ["player.inventory_transaction_result"]
            capabilities = ["player_inventory"]
            player_commands = ["kitgrant", "kitexchange", "kitoverdraw", "kitunknown", "kitclear"]
        "#,
    )
    .expect("write owner manifest");
    std::fs::write(
        owner.join("main.lua"),
        r#"
            function on_player_command(event)
                if event.root == "kitgrant" then
                    solaris.inventory_transaction(event.player_id, "grant", {
                        { resource = "minecraft:emerald", delta = 3 },
                    })
                elseif event.root == "kitexchange" then
                    solaris.inventory_transaction(event.player_id, "exchange", {
                        { resource = "minecraft:emerald", delta = -2 },
                        { resource = "minecraft:apple", delta = 4 },
                    })
                elseif event.root == "kitoverdraw" then
                    solaris.inventory_transaction(event.player_id, "overdraw", {
                        { resource = "minecraft:emerald", delta = -1 },
                        { resource = "minecraft:apple", delta = -5 },
                    })
                elseif event.root == "kitunknown" then
                    solaris.inventory_transaction(event.player_id, "unknown", {
                        { resource = "minecraft:emerald", delta = -1 },
                        { resource = "minecraft:not_an_item", delta = 1 },
                    })
                elseif event.root == "kitclear" then
                    solaris.inventory_transaction(event.player_id, "clear", {
                        { resource = "minecraft:emerald", delta = -1 },
                        { resource = "minecraft:apple", delta = -4 },
                    })
                end
            end

            function on_player_inventory_transaction_result(event)
                solaris.send_message(
                    event.player_id,
                    "inventory:" .. event.request_id .. ":" .. tostring(event.committed)
                        .. ":" .. tostring(event.failure)
                )
            end
        "#,
    )
    .expect("write owner source");
}

fn write_observer_plugin(root: &std::path::Path) {
    let observer = root.join("inventory-observer");
    std::fs::create_dir(&observer).expect("create observer plugin directory");
    std::fs::write(
        observer.join("plugin.toml"),
        r#"
            id = "inventory-observer"
            name = "Inventory Observer"
            version = "0.1.0"
            api = "0.6.0"
            events = ["player.inventory_transaction_result"]
        "#,
    )
    .expect("write observer manifest");
    std::fs::write(
        observer.join("main.lua"),
        r#"
            function on_player_inventory_transaction_result(event)
                solaris.send_message(event.player_id, "leaked-inventory-result")
            end
        "#,
    )
    .expect("write observer source");
}

async fn send_command(client: &mut Client, command: &str) {
    client
        .write_packet(&ServerboundChatCommand {
            command: command.to_owned(),
        })
        .await
        .unwrap_or_else(|error| panic!("send /{command}: {error}"));
}

async fn wait_for_result_and_inventory(
    client: &mut Client,
    expected: &str,
    emerald: u32,
    emerald_count: i32,
    apple: u32,
    apple_count: i32,
) {
    let mut seen = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let mut inventory_matches = false;
        let mut result_seen = false;
        loop {
            let mut frame = client.read_frame().await.expect("plugin result frame");
            if frame.id == ClientboundContainerSetContent::ID {
                let content = ClientboundContainerSetContent::decode(&mut frame.body)
                    .expect("decode authoritative inventory");
                if seen.len() < 16 {
                    seen.push(format!(
                        "inventory:{}:{:?}",
                        content.container_id,
                        content
                            .items
                            .iter()
                            .filter(|item| !item.is_empty())
                            .map(|item| (item.item_id, item.count))
                            .collect::<Vec<_>>()
                    ));
                }
                if content.container_id == 0
                    && total_count(&content, emerald) == emerald_count
                    && total_count(&content, apple) == apple_count
                {
                    inventory_matches = true;
                }
            } else if frame.id == ClientboundSystemChat::ID {
                let chat = ClientboundSystemChat::decode(&mut frame.body)
                    .expect("decode inventory result chat");
                let message = literal_text_component_text(&chat.content_nbt);
                if seen.len() < 16 {
                    seen.push(format!("chat:{message}"));
                }
                assert_ne!(message, "leaked-inventory-result");
                if message == expected {
                    result_seen = true;
                }
            }
            if result_seen && inventory_matches {
                return;
            }
        }
    })
    .await;
    assert!(
        result.is_ok(),
        "inventory result/snapshot {expected:?} timeout; seen {seen:?}"
    );
}

async fn wait_for_result_without_inventory(client: &mut Client, expected: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let mut frame = client.read_frame().await.expect("plugin rejection frame");
            if frame.id == ClientboundContainerSetContent::ID {
                let content = ClientboundContainerSetContent::decode(&mut frame.body)
                    .expect("decode unexpected authoritative inventory");
                if content.container_id == 0 {
                    panic!("rejected transaction published an inventory snapshot");
                }
            } else if frame.id == ClientboundSystemChat::ID {
                let chat = ClientboundSystemChat::decode(&mut frame.body)
                    .expect("decode inventory rejection chat");
                let message = literal_text_component_text(&chat.content_nbt);
                assert_ne!(message, "leaked-inventory-result");
                if message == expected {
                    return;
                }
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("inventory rejection {expected:?} timeout"));
}

fn item_id(items: &mc_data::items::ItemRegistry, resource: &str) -> u32 {
    items
        .id_of(&mc_data::Identifier::parse(resource).expect("checked item identifier"))
        .unwrap_or_else(|| panic!("missing embedded item {resource}"))
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
