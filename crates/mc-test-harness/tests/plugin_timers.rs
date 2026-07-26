use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundCommands, ClientboundSystemChat, ConfirmTeleportation, ServerboundChatCommand,
};
use mc_test_harness::client::Client;

#[tokio::test]
async fn lua_timer_is_pushed_by_simulation_ticks_without_tick_subscription() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let plugin = plugins.path().join("timer-wire");
    std::fs::create_dir(&plugin).expect("create timer plugin directory");
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"
            id = "timer-wire"
            name = "Timer Wire"
            version = "0.1.0"
            api = "0.6.0"
            player_commands = ["timerwire"]
        "#,
    )
    .expect("write timer manifest");
    std::fs::write(
        plugin.join("main.lua"),
        r#"
            local waiting_player = nil

            function on_player_command(event)
                waiting_player = event.player_id
                local scheduled = solaris.schedule_timer("wire", 2)
                assert(scheduled >= 2)
            end

            function on_plugin_timer(event)
                assert(event.timer_id == "wire")
                assert(event.fired_tick >= event.scheduled_tick)
                solaris.send_message(waiting_player, "timer-fired")
                waiting_player = nil
            end
        "#,
    )
    .expect("write timer source");

    let (boundary, host) = mc_script::start_lua_host(mc_script::LuaHostConfig::new(plugins.path()))
        .expect("start Lua host");
    assert_eq!(host.loaded_plugins(), 1);
    let shutdown = mc_net::ShutdownHandle::default();
    let config = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Lua timer wire test".into(),
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
    let bound = mc_net::bind_with_scripts(config, boundary)
        .await
        .expect("bind scripted server");
    let address = bound.local_addr().expect("server address");
    let server = tokio::spawn(async move { bound.serve().await });

    let mut client = Client::connect(address).await.expect("client connect");
    let _ = client
        .drive_login(address, "TimerPlayer")
        .await
        .expect("login");
    client.drive_configuration().await.expect("configuration");
    let _ = client.read_play_login().await.expect("play entry");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");
    let initial_sync: mc_protocol::packets::play::SynchronizePlayerPosition =
        client.read_typed().await.expect("initial position");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: initial_sync.teleport_id,
        })
        .await
        .expect("confirm initial position");
    client
        .write_packet(&ServerboundChatCommand {
            command: "timerwire".to_owned(),
        })
        .await
        .expect("send timer command");

    assert_eq!(next_system_chat_text(&mut client).await, "timer-fired");

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

async fn next_system_chat_text(client: &mut Client) -> String {
    let outcome = client
        .wait_for_frame_id_with_timeout(ClientboundSystemChat::ID, Duration::from_secs(5))
        .await
        .expect("system chat frame");
    let packet =
        ClientboundSystemChat::decode(&mut outcome.body.clone()).expect("decode SystemChat");
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
