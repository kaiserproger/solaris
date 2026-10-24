//! `/transfer` command tests: the clientbound Transfer packet, target
//! resolution, default port and operator gating.

use super::*;
use mc_protocol::packets::{Packet, play::*};

fn fixture() -> (EdgeServer, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("mc-edge-transfer-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut config = mc_core::config::CoreConfig::new(dir.join("world"), 42);
    config.vanilla_data_dir =
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"));
    config.checkpoint_every_ticks = u64::MAX;
    let hub = Arc::new(Core::open(config).unwrap());
    let report = mc_data::blocks::solaris_required_blocks_report();
    let data = Arc::new(mc_data::solaris_required_data());
    let cfg = EdgeConfig::default();
    let server = EdgeServer {
        recipes: crate::recipes::RecipePackets::new(hub.registry()).unwrap(),
        loader_manifest: None,
        wire: WireRegistries {
            dimension_type_ids: crate::dimensions::registry_ids(&data).unwrap(),
            biomes: data.registry("worldgen/biome").unwrap().clone(),
            blocks: mc_world::BlockRegistry::from_report(&report).unwrap(),
            items: mc_data::items::solaris_required_items(),
            block_entity_types: mc_data::block_entity_types::solaris_required_block_entity_types(),
        },
        hub,
        data,
        tags: Arc::new(Default::default()),
        dashboard: dashboard_state(&cfg),
        cfg,
        connections: Arc::new(ConnectionRegistry::new()),
        tick_stats: Arc::new(TickStats::new()),
        net_counters: Arc::new(NetCounters::new()),
        admin: crate::admin::Admin::open(&dir).unwrap(),
        console_state: Default::default(),
        stop: tokio::sync::watch::channel(false).0,
        suggestion_items: vec!["minecraft:stone".into()],
        suggestion_blocks: vec!["minecraft:stone".into()],
        suggestion_entities: vec!["minecraft:pig".into()],
        suggestion_structures: vec![],
        online_authentication: None,
        velocity: None,
        loader_required: false,
        lod: None,
        dh: None,
    };
    (server, dir)
}

fn register(
    server: &EdgeServer,
    name: &str,
    op: bool,
) -> (ConnectionId, mpsc::Receiver<bytes::Bytes>) {
    let id = server.connections.joining_id();
    let uuid = crate::crypto::offline_uuid(name);
    let (reliable_tx, reliable_rx) = mpsc::channel(64);
    let (world_tx, _) = mpsc::channel(64);
    server
        .connections
        .register(
            id,
            ConnectionHandles {
                reliable_tx,
                world_tx,
            },
        )
        .unwrap();
    server.connections.set_name(id, name.into());
    server
        .admin
        .register(id, uuid, name.into(), "127.0.0.1".parse().unwrap());
    if op {
        server.admin.set_op(uuid, name, true).unwrap();
    }
    (id, reliable_rx)
}

async fn receive(rx: &mut mpsc::Receiver<bytes::Bytes>) -> mc_protocol::frame::RawFrame {
    use tokio::io::AsyncWriteExt;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut writer = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (reader, _) = listener.accept().await.unwrap();
    writer.write_all(&rx.recv().await.unwrap()).await.unwrap();
    let mut connection = crate::connection::Connection::new(reader);
    connection.set_compression(mc_protocol::frame::Compression::Threshold(256));
    connection.read_frame_play().await.unwrap()
}

fn chat_text(frame: &mc_protocol::frame::RawFrame) -> String {
    assert_eq!(frame.id, ClientboundSystemChat::ID);
    let message = ClientboundSystemChat::decode(&mut &frame.body[..]).unwrap();
    let content = mc_nbt::read_network(&mut message.content_nbt.as_slice()).unwrap();
    match content {
        mc_nbt::Tag::Compound(fields) => fields
            .into_iter()
            .find_map(|(key, value)| match (key.as_str(), value) {
                ("text", mc_nbt::Tag::String(text)) => Some(text),
                _ => None,
            })
            .unwrap_or_default(),
        other => panic!("expected chat compound, got {other:?}"),
    }
}

#[tokio::test]
async fn transfer_sends_the_packet_to_named_targets() {
    let (server, dir) = fixture();
    let (operator, mut operator_rx) = register(&server, "Operator", true);
    let (_alex, mut alex_rx) = register(&server, "Alex", false);
    assert!(
        server
            .handle_admin_command(operator, "transfer lobby.example.com 25566 Alex")
            .await
            .unwrap()
    );
    let frame = receive(&mut alex_rx).await;
    assert_eq!(frame.id, ClientboundTransfer::ID);
    let transfer = ClientboundTransfer::decode(&mut &frame.body[..]).unwrap();
    assert_eq!(transfer.host, "lobby.example.com");
    assert_eq!(transfer.port, 25_566);

    let feedback = chat_text(&receive(&mut operator_rx).await);
    assert_eq!(feedback, "Transferring Alex to lobby.example.com:25566");
    server.hub.shutdown();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn transfer_without_players_moves_the_executor_and_defaults_to_25565() {
    let (server, dir) = fixture();
    let (operator, mut operator_rx) = register(&server, "Operator", true);
    assert!(
        server
            .handle_admin_command(operator, "transfer second.example.com")
            .await
            .unwrap()
    );
    let frame = receive(&mut operator_rx).await;
    assert_eq!(frame.id, ClientboundTransfer::ID);
    let transfer = ClientboundTransfer::decode(&mut &frame.body[..]).unwrap();
    assert_eq!(transfer.host, "second.example.com");
    assert_eq!(transfer.port, 25_565);
    let feedback = chat_text(&receive(&mut operator_rx).await);
    assert_eq!(
        feedback,
        "Transferring Operator to second.example.com:25565"
    );
    server.hub.shutdown();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn transfer_from_console_requires_explicit_players() {
    let (server, dir) = fixture();
    let mut rx = {
        // The console has no queue; capture its stdout-independent feedback by
        // asserting the command's other effect: no target moved.
        let (_operator, rx) = register(&server, "Operator", true);
        rx
    };
    assert!(
        server
            .handle_admin_command(
                mc_core::command_console::CONNECTION,
                "transfer host.example"
            )
            .await
            .unwrap()
    );
    // No transfer frame reached the registered player.
    assert!(rx.try_recv().is_err());
    server.hub.shutdown();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn transfer_rejects_unknown_players_and_bad_ports() {
    let (server, dir) = fixture();
    let (operator, mut operator_rx) = register(&server, "Operator", true);
    for (command, expected) in [
        (
            "transfer host.example 70000",
            "Incorrect argument for command",
        ),
        ("transfer host.example 0", "Incorrect argument for command"),
        ("transfer host.example 25566 Missing", "No player was found"),
    ] {
        assert!(
            server
                .handle_admin_command(operator, command)
                .await
                .unwrap()
        );
        assert_eq!(chat_text(&receive(&mut operator_rx).await), expected);
    }
    server.hub.shutdown();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn transfer_requires_operator_permission() {
    let (server, dir) = fixture();
    let (alex, mut alex_rx) = register(&server, "Alex", false);
    assert!(
        server
            .handle_admin_command(alex, "transfer host.example")
            .await
            .unwrap()
    );
    assert_eq!(
        chat_text(&receive(&mut alex_rx).await),
        "Unknown or incomplete command"
    );
    server.hub.shutdown();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn transfer_is_advertised_at_operator_level_three() {
    assert!(crate::command_tree::permitted(
        "transfer host.example",
        3,
        &[]
    ));
    assert!(!crate::command_tree::permitted(
        "transfer host.example",
        2,
        &[]
    ));
    let packet = crate::command_tree::tree(3, &[]);
    let root = packet
        .nodes
        .iter()
        .find(|node| matches!(&node.kind, CommandNodeKind::Literal(name) if name == "transfer"))
        .expect("transfer root");
    assert!(!root.executable, "hostname is required");
    let hostname = &packet.nodes[root.children[0] as usize];
    let CommandNodeKind::Argument { name, parser, .. } = &hostname.kind else {
        panic!("hostname argument");
    };
    assert_eq!(name, "hostname");
    assert_eq!(
        *parser,
        CommandArgumentParser::String(CommandStringKind::QuotablePhrase)
    );
    let port = &packet.nodes[hostname.children[0] as usize];
    let CommandNodeKind::Argument { name, parser, .. } = &port.kind else {
        panic!("port argument");
    };
    assert_eq!(name, "port");
    assert_eq!(
        *parser,
        CommandArgumentParser::Integer {
            min: Some(1),
            max: Some(65_535)
        }
    );
    assert!(packet.nodes[port.children[0] as usize].executable);
}
