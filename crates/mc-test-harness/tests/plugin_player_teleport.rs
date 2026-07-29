use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundCommands, ClientboundSystemChat, ConfirmTeleportation, ServerboundChatCommand,
    SetCenterChunk, SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;

#[tokio::test]
async fn lua_player_teleport_commits_through_the_session_owner_and_reports_exact_failures() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let owner = plugins.path().join("warps");
    std::fs::create_dir(&owner).expect("create owner plugin directory");
    std::fs::write(
        owner.join("plugin.toml"),
        r#"
            id = "warps"
            name = "Warps"
            version = "0.1.0"
            api = "0.6.0"
            events = ["player.joined", "player.teleport_result", "player.zone_entered"]
            capabilities = ["player_teleport", "zones"]
            player_commands = ["warp", "where"]
        "#,
    )
    .expect("write owner manifest");
    std::fs::write(
        owner.join("main.lua"),
        r#"
            --!strict

            function on_player_joined(event: any)
                solaris.upsert_zone("destination", "minecraft:alpha", 39, -60, 0, 41, -58, 2)
                solaris.teleport_player("initial", event.player_id, 40, -59, 1)
            end

            function on_player_command(event: any)
                if event.root == "warp" then
                    solaris.teleport_player("warp", event.player_id, 40, -59, 1)
                elseif event.root == "where" then
                    solaris.send_message(
                        event.player_id,
                        "where:" .. tostring(event.x) .. ":" .. tostring(event.y) .. ":" .. tostring(event.z)
                    )
                end
            end

            function on_player_zone_entered(event: any)
                solaris.send_message(event.player_id, "zone:" .. event.zone_id)
            end

            function on_player_teleport_result(event: any)
                solaris.send_message(
                    event.player_id,
                    "teleport:" .. event.request_id .. ":" .. tostring(event.committed) .. ":" .. tostring(event.failure)
                        .. ":" .. tostring(event.x) .. ":" .. tostring(event.y) .. ":" .. tostring(event.z)
                )
            end
        "#,
    )
    .expect("write owner source");
    let observer = plugins.path().join("teleport-observer");
    std::fs::create_dir(&observer).expect("create observer plugin directory");
    std::fs::write(
        observer.join("plugin.toml"),
        r#"
            id = "teleport-observer"
            name = "Teleport Observer"
            version = "0.1.0"
            api = "0.6.0"
            events = ["player.teleport_result"]
        "#,
    )
    .expect("write observer manifest");
    std::fs::write(
        observer.join("main.lua"),
        r#"
            function on_player_teleport_result(event: any)
                solaris.send_message(event.player_id, "leaked-teleport-result")
            end
        "#,
    )
    .expect("write observer source");

    let (boundary, host) = mc_script::start_lua_host(mc_script::LuaHostConfig::new(plugins.path()))
        .expect("start Lua host");
    assert_eq!(
        host.loaded_plugins(),
        2,
        "loaded command roots: {:?}",
        boundary.player_command_roots()
    );
    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Lua teleport wire test".into(),
        max_players: 1,
        view_distance: 2,
        data: Arc::new(mc_data::testing::stub()),
        blocks: Arc::new(mc_world::BlockRegistry::from_report(&[]).unwrap()),
        world: None,
        tags: Arc::new(mc_data::tags::TagsData::default()),
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items: Arc::new(mc_data::items::ItemRegistry::default()),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), false),
        loader_manifest: None,
        shutdown: shutdown.clone(),
    };
    let bound = mc_net::bind_with_scripts(cfg, boundary)
        .await
        .expect("bind scripted server");
    let addr = bound.local_addr().expect("local_addr");
    let server = tokio::spawn(async move { bound.serve().await });

    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client.drive_login(addr, "WarpPlayer").await.expect("login");
    client.drive_configuration().await.expect("configuration");
    let _ = client.read_play_login().await.expect("play entry");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");
    let initial_sync: SynchronizePlayerPosition =
        client.read_typed().await.expect("initial SyncPlayerPos");
    assert_eq!(
        next_system_chat_text(&mut client).await,
        "teleport:initial:false:teleport_pending:40:-59:1"
    );
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: initial_sync.teleport_id,
        })
        .await
        .expect("ack initial teleport");

    send_plugin_command(&mut client, "warp").await;
    let (warp_sync, center, mut messages) =
        wait_for_teleport_center_and_messages(&mut client, (2, 0), 2).await;
    assert_eq!((warp_sync.x, warp_sync.y, warp_sync.z), (40.0, -59.0, 1.0));
    assert_eq!((center.chunk_x, center.chunk_z), (2, 0));
    messages.sort();
    assert_eq!(
        messages,
        ["teleport:warp:true:nil:40:-59:1", "zone:destination"],
        "zone transition and owner result must be exact and non-leaking"
    );

    send_plugin_command(&mut client, "warp").await;
    assert_eq!(
        next_system_chat_text(&mut client).await,
        "teleport:warp:false:teleport_pending:40:-59:1"
    );
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: warp_sync.teleport_id,
        })
        .await
        .expect("ack plugin teleport");
    send_plugin_command(&mut client, "where").await;
    assert_eq!(next_system_chat_text(&mut client).await, "where:40:-59:1");

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

async fn send_plugin_command(client: &mut Client, command: &str) {
    client
        .write_packet(&ServerboundChatCommand {
            command: command.to_owned(),
        })
        .await
        .expect("send plugin command");
}

async fn wait_for_teleport_center_and_messages(
    client: &mut Client,
    expected_center: (i32, i32),
    message_count: usize,
) -> (SynchronizePlayerPosition, SetCenterChunk, Vec<String>) {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut teleport = None;
        let mut center = None;
        let mut messages = Vec::new();
        loop {
            let mut frame = client.read_frame().await.expect("teleport wire frame");
            if frame.id == SynchronizePlayerPosition::ID {
                teleport = Some(
                    SynchronizePlayerPosition::decode(&mut frame.body)
                        .expect("decode teleport position"),
                );
            } else if frame.id == SetCenterChunk::ID {
                let packet =
                    SetCenterChunk::decode(&mut frame.body).expect("decode teleport center chunk");
                if (packet.chunk_x, packet.chunk_z) == expected_center {
                    center = Some(packet);
                }
            } else if frame.id == ClientboundSystemChat::ID {
                let packet = ClientboundSystemChat::decode(&mut frame.body)
                    .expect("decode teleport system chat");
                messages.push(text_component_text(&packet));
            }
            if messages.len() == message_count
                && let Some(teleport) = teleport
                && let Some(center) = center
            {
                return (teleport, center, messages);
            }
        }
    })
    .await
    .expect("teleport packet/result timeout")
}

async fn next_system_chat_text(client: &mut Client) -> String {
    let outcome = client
        .wait_for_frame_id_with_timeout(ClientboundSystemChat::ID, Duration::from_secs(5))
        .await
        .expect("system chat frame");
    let packet =
        ClientboundSystemChat::decode(&mut outcome.body.clone()).expect("decode SystemChat");
    text_component_text(&packet)
}

fn text_component_text(packet: &ClientboundSystemChat) -> String {
    let mut bytes = Bytes::copy_from_slice(&packet.content_nbt);
    let mc_nbt::Tag::Compound(fields) =
        mc_nbt::read_network(&mut bytes).expect("read text component nbt")
    else {
        panic!("system chat component root must be a compound");
    };
    fields
        .into_iter()
        .find_map(|(name, tag)| match (name.as_str(), tag) {
            ("text", mc_nbt::Tag::String(text)) => Some(text),
            _ => None,
        })
        .expect("literal system chat text")
}
