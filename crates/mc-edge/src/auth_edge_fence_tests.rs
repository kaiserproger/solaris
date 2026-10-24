use super::*;
use mc_core::boundary::{ConnectionId, PlayerInput};

fn fixture(require_plugin_login: bool) -> (EdgeServer, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "mc-edge-auth-fence-{}-{}",
        require_plugin_login,
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let mut config = mc_core::config::CoreConfig::new(dir.join("world"), 42);
    config.view_distance = 1;
    config.simulation_distance = 0;
    config.vanilla_data_dir =
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"));
    config.checkpoint_every_ticks = u64::MAX;
    config.require_plugin_login = require_plugin_login;
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

fn join_native(server: &EdgeServer, id: ConnectionId, name: &str) {
    assert!(server.hub.submit(PlayerInput::Join {
        connection: id,
        uuid: crate::crypto::offline_uuid(name),
        name: name.into(),
        permission_level: server.admin.session_level(id),
    }));
}

async fn wait_ticks(server: &EdgeServer, ticks: usize) {
    let mut rx = server.hub.subscribe();
    for _ in 0..ticks {
        match rx.recv().await {
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                panic!("tick thread died")
            }
        }
        let _ = server.hub.try_drain_outbox();
    }
}

#[tokio::test]
async fn locked_session_runs_no_edge_admin_effect() {
    let (server, dir) = fixture(true);
    let (id, mut rx) = register(&server, "LockedOp", true);
    assert!(
        server.hub.session_locked(id),
        "pre-Join edge fence must fail closed"
    );
    assert!(!server.handle_admin_command(id, "stop").await.unwrap());
    assert!(!*server.stop.borrow());
    join_native(&server, id, "LockedOp");
    wait_ticks(&server, 3).await;
    assert!(
        server.hub.session_locked(id),
        "fixture must project the native lock"
    );
    // Every mutating edge root is fenced: `Ok(false)` so the input still
    // reaches the native gate, which refuses locked gameplay commands.
    assert!(!server.handle_admin_command(id, "stop").await.unwrap());
    assert!(
        !server
            .handle_admin_command(id, "save-all flush")
            .await
            .unwrap()
    );
    assert!(
        !server
            .handle_admin_command(id, "op LockedOp")
            .await
            .unwrap()
    );
    assert!(
        !server
            .handle_admin_command(id, "kick LockedOp")
            .await
            .unwrap()
    );
    assert!(rx.try_recv().is_err(), "locked fence sends no feedback");
    assert!(
        !*server.stop.borrow(),
        "locked stop must not halt the server"
    );
    server.hub.shutdown();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn locked_session_cannot_use_op_level_or_admin_roots() {
    let (server, dir) = fixture(true);
    let (id, _rx) = register(&server, "LockedOp", true);
    join_native(&server, id, "LockedOp");
    wait_ticks(&server, 3).await;
    assert!(server.hub.session_locked(id));
    // A locked operator's edge permission is fenced on every mutating root:
    // `Ok(false)` lets the input reach the native gate, which refuses locked
    // gameplay commands. No bypass through op level or username targeting.
    assert!(
        !server
            .handle_admin_command(id, "op LockedOp")
            .await
            .unwrap()
    );
    assert!(
        !server
            .handle_admin_command(id, "ban LockedOp")
            .await
            .unwrap()
    );
    server.hub.shutdown();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn unlocked_session_keeps_normal_admin_behavior() {
    let (server, dir) = fixture(false);
    let (id, mut rx) = register(&server, "Operator", true);
    join_native(&server, id, "Operator");
    wait_ticks(&server, 3).await;
    assert!(!server.hub.session_locked(id));
    assert!(
        server
            .handle_admin_command(id, "save-all flush")
            .await
            .unwrap()
    );
    // Two frames: the "saving" notice plus the success report.
    rx.recv().await.unwrap();
    rx.recv().await.unwrap();
    server.hub.shutdown();
    std::fs::remove_dir_all(dir).unwrap();
}
