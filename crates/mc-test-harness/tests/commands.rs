use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundCommands, ClientboundSetTime, ClientboundSystemChat, ConfirmTeleportation,
    GameEvent, GameMode, ServerboundChatCommand, SetCenterChunk, SynchronizePlayerPosition,
};
use mc_test_harness::client::{Client, FrameWaitLimits};

const LATENCY_SENSITIVE_FRAME_WAIT_LIMITS: FrameWaitLimits = FrameWaitLimits {
    max_skipped_frames: Some(128),
    max_skipped_bytes: Some(128 * 1024),
};

async fn start_server() -> SocketAddr {
    start_server_with_shutdown(mc_net::ShutdownHandle::default()).await
}

async fn start_server_with_shutdown(shutdown: mc_net::ShutdownHandle) -> SocketAddr {
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M35 commands".into(),
        max_players: 4,
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
        entity_types: Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown,
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });
    addr
}

#[tokio::test]
async fn command_tree_gamemode_and_feedback_round_trip() {
    let addr = start_server().await;
    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client.drive_login(addr, "M35Command").await.expect("login");
    client.drive_configuration().await.expect("configuration");

    let _ = client.read_play_login().await.expect("play entry");
    let commands: ClientboundCommands = client.read_typed().await.expect("Commands");
    let root = &commands.nodes[commands.root_index as usize];
    assert!(
        !root.children.is_empty(),
        "command tree must advertise commands"
    );

    client
        .write_packet(&ServerboundChatCommand {
            command: "gamemode creative".to_string(),
        })
        .await
        .expect("send gamemode command");

    loop {
        let outcome = client
            .wait_for_frame_id_with_timeout_and_limits(
                GameEvent::ID,
                Duration::from_secs(5),
                LATENCY_SENSITIVE_FRAME_WAIT_LIMITS,
            )
            .await
            .expect("gamemode event frame");
        let frame = outcome.frame;
        let event = GameEvent::decode(&mut frame.body.clone()).expect("decode GameEvent");
        if event.event == GameEvent::EVENT_CHANGE_GAME_MODE
            && event.value == GameMode::Creative.id() as f32
        {
            break;
        }
    }
    let outcome = client
        .wait_for_frame_id_with_timeout_and_limits(
            ClientboundSystemChat::ID,
            Duration::from_secs(5),
            LATENCY_SENSITIVE_FRAME_WAIT_LIMITS,
        )
        .await
        .expect("command feedback frame");
    assert!(
        outcome.skipped.frames <= 16,
        "command feedback skipped too many frames: {:?}",
        outcome.skipped
    );
    let frame = outcome.frame;
    let feedback =
        ClientboundSystemChat::decode(&mut frame.body.clone()).expect("decode SystemChat");
    assert!(!feedback.content_nbt.is_empty());
}

#[tokio::test]
async fn save_all_and_stop_commands_report_feedback_and_signal_shutdown() {
    let shutdown = mc_net::ShutdownHandle::default();
    let addr = start_server_with_shutdown(shutdown.clone()).await;
    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client.drive_login(addr, "M35Ops").await.expect("login");
    client.drive_configuration().await.expect("configuration");

    let _ = client.read_play_login().await.expect("play entry");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");

    client
        .write_packet(&ServerboundChatCommand {
            command: "save-all".to_string(),
        })
        .await
        .expect("send save-all command");
    assert_eq!(
        next_system_chat_text(&mut client).await,
        "Saved 0 players, 0 entities, 0 chunks"
    );
    assert!(!shutdown.is_requested());

    client
        .write_packet(&ServerboundChatCommand {
            command: "stop".to_string(),
        })
        .await
        .expect("send stop command");
    assert_eq!(
        next_system_chat_text(&mut client).await,
        "Saved all state; stopping server"
    );
    assert!(shutdown.is_requested());
}

#[tokio::test]
async fn client_receives_continuing_world_time_updates() {
    let addr = start_server().await;
    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client.drive_login(addr, "M38Time").await.expect("login");
    client.drive_configuration().await.expect("configuration");

    let _ = client.read_play_login().await.expect("play entry");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    let _: mc_protocol::packets::play::ClientboundInitializeBorder =
        client.read_typed().await.expect("InitializeBorder");
    let _: ClientboundSetTime = client.read_typed().await.expect("SetTime");
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

    let first = next_time_update(&mut client).await;
    let second = next_time_update(&mut client).await;

    assert!(
        second.game_time > first.game_time,
        "world time updates must advance"
    );
}

async fn next_system_chat_text(client: &mut Client) -> String {
    let outcome = client
        .wait_for_frame_id_with_timeout_and_limits(
            ClientboundSystemChat::ID,
            Duration::from_secs(5),
            LATENCY_SENSITIVE_FRAME_WAIT_LIMITS,
        )
        .await
        .expect("system chat frame");
    let packet =
        ClientboundSystemChat::decode(&mut outcome.frame.body.clone()).expect("decode SystemChat");
    text_component_text(&packet)
}

fn text_component_text(packet: &ClientboundSystemChat) -> String {
    let mut bytes = Bytes::copy_from_slice(&packet.content_nbt);
    let tag = mc_nbt::read_network(&mut bytes).expect("read text component nbt");
    let mc_nbt::Tag::Compound(fields) = tag else {
        panic!("system chat component root must be a compound");
    };
    fields
        .into_iter()
        .find_map(|(name, tag)| match (name.as_str(), tag) {
            ("text", mc_nbt::Tag::String(text)) => Some(text),
            _ => None,
        })
        .expect("system chat component must contain text")
}

async fn next_time_update(client: &mut Client) -> ClientboundSetTime {
    let outcome = client
        .wait_for_frame_id_with_timeout_and_limits(
            ClientboundSetTime::ID,
            Duration::from_secs(5),
            LATENCY_SENSITIVE_FRAME_WAIT_LIMITS,
        )
        .await
        .expect("world time frame");
    assert!(
        outcome.skipped.bytes <= 32 * 1024,
        "time update skipped too many bytes: {:?}",
        outcome.skipped
    );
    let frame = outcome.frame;
    ClientboundSetTime::decode(&mut frame.body.clone()).expect("decode SetTime")
}
