use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundCommands, ClientboundSetTime, ClientboundSystemChat, ConfirmTeleportation,
    GameEvent, GameMode, LoginPlay, ServerboundChatCommand, SetCenterChunk,
    SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;

async fn start_server() -> SocketAddr {
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
        shutdown: mc_net::ShutdownHandle::default(),
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

    let _: LoginPlay = client.read_typed().await.expect("LoginPlay");
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

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut saw_gamemode = false;
    let mut saw_feedback = false;
    while !(saw_gamemode && saw_feedback) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("command response frame");
        if frame.id == GameEvent::ID {
            let event = GameEvent::decode(&mut frame.body.clone()).expect("decode GameEvent");
            if event.event == GameEvent::EVENT_CHANGE_GAME_MODE
                && event.value == GameMode::Creative.id() as f32
            {
                saw_gamemode = true;
            }
        } else if frame.id == ClientboundSystemChat::ID {
            let feedback =
                ClientboundSystemChat::decode(&mut frame.body.clone()).expect("decode SystemChat");
            assert!(!feedback.content_nbt.is_empty());
            saw_feedback = true;
        }
    }
}

#[tokio::test]
async fn client_receives_continuing_world_time_updates() {
    let addr = start_server().await;
    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client.drive_login(addr, "M38Time").await.expect("login");
    client.drive_configuration().await.expect("configuration");

    let _: LoginPlay = client.read_typed().await.expect("LoginPlay");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
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

async fn next_time_update(client: &mut Client) -> ClientboundSetTime {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("world time frame");
        if frame.id == ClientboundSetTime::ID {
            return ClientboundSetTime::decode(&mut frame.body.clone()).expect("decode SetTime");
        }
    }
}
