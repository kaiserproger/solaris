use super::*;

fn server(root: &std::path::Path) -> EdgeServer {
    let mut config = mc_core::CoreConfig::new(root.join("world"), 42);
    config.vanilla_data_dir =
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"));
    config.view_distance = 0;
    config.simulation_distance = 0;
    let core = Arc::new(mc_core::Core::open(config).unwrap());
    let mut cfg = crate::config::EdgeConfig {
        server_dir: root.join("server"),
        ..Default::default()
    };
    std::fs::create_dir_all(&cfg.server_dir).unwrap();
    cfg.data.world_dir = root.join("world");
    cfg.data.vanilla_data_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    cfg.auth.online_mode = false;
    let data = crate::server::load_vanilla_data(&cfg).unwrap();
    let tags = crate::server::load_tags(&cfg, &data).unwrap();
    EdgeServer::new(core, data, tags, cfg).unwrap()
}

/// A core that halts itself (journal append failure, fail closed) queues a
/// terminal `Disconnect{reason}` per connection in its reliable outbox. The
/// pump must still route it when the tick broadcast closes: a bare socket
/// close left clients (and the log) without the cause.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn core_halt_routes_terminal_disconnect_reason() {
    let dir = tempfile::tempdir().unwrap();
    let server = server(dir.path());
    let journal = server.cfg.data.world_dir.join("journal");
    let core = Arc::clone(&server.hub);
    let id = server.connections.joining_id();
    let (reliable_tx, mut reliable_rx) = mpsc::channel(256);
    let (world_tx, _world_rx) = mpsc::channel(256);
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
    assert!(core.submit(mc_core::boundary::PlayerInput::Join {
        connection: id,
        uuid: uuid::Uuid::new_v4(),
        name: "halt".to_owned(),
        permission_level: 0,
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let serve = tokio::spawn(crate::server::serve(listener, server));
    // The player is live once its join burst is routed.
    tokio::time::timeout(std::time::Duration::from_secs(60), reliable_rx.recv())
        .await
        .expect("join burst")
        .expect("reliable queue open");
    // Real failure, no hook: the next WAL append cannot create its batch file.
    std::fs::rename(&journal, journal.with_file_name("journal-gone")).unwrap();
    let needle = b"journal failed: world halted";
    let found = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        while let Some(packet) = reliable_rx.recv().await {
            if packet.windows(needle.len()).any(|window| window == needle) {
                return true;
            }
        }
        false
    })
    .await
    .expect("halt reaches the pump");
    assert!(found, "terminal Disconnect carries the halt reason");
    let ended = tokio::time::timeout(std::time::Duration::from_secs(60), serve)
        .await
        .expect("serve returns after the core halts")
        .unwrap();
    assert!(ended.is_err(), "a dead core ends the server with an error");
    core.shutdown();
}
