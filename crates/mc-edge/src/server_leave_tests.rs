//! Player retiral on every post-join exit path. A client that dies while the
//! edge is still writing the initial join burst used to return before the
//! tail teardown, leaving a ghost core player (and its edge/admin entries)
//! behind.

use std::time::Duration;

use super::*;
use mc_vanilla::wire_client::Client;

fn server(root: &std::path::Path) -> EdgeServer {
    let content = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let mut config = mc_core::CoreConfig::new(root.join("world"), 81);
    config.vanilla_data_dir = Some(content.clone());
    // A real client view radius: the join burst spans hundreds of chunks, so
    // the socket can die while it is still being written.
    config.view_distance = 8;
    config.simulation_distance = 8;
    let core = Arc::new(mc_core::Core::open(config).unwrap());
    let mut cfg = crate::config::EdgeConfig {
        server_dir: root.join("server"),
        ..Default::default()
    };
    std::fs::create_dir_all(&cfg.server_dir).unwrap();
    cfg.server.view_distance = 8;
    cfg.server.simulation_distance = 8;
    cfg.data.world_dir = root.join("world");
    cfg.data.vanilla_data_dir = content;
    cfg.auth.online_mode = false;
    let data = crate::server::load_vanilla_data(&cfg).unwrap();
    let tags = crate::server::load_tags(&cfg, &data).unwrap();
    EdgeServer::new(core, data, tags, cfg).unwrap()
}

async fn wait_snapshot(
    ticks: &mut tokio::sync::broadcast::Receiver<mc_core::boundary::TickOutput>,
    timeout: Duration,
    mut predicate: impl FnMut(&mc_core::boundary::WorldSnapshot) -> bool,
) -> Arc<mc_core::boundary::WorldSnapshot> {
    tokio::time::timeout(timeout, async {
        loop {
            match ticks.recv().await {
                Ok(output) => {
                    if predicate(&output.snapshot) {
                        return output.snapshot;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("core tick broadcast closed")
                }
            }
        }
    })
    .await
    .expect("core publication did not satisfy the predicate")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_dying_during_join_burst_leaves_core() {
    let dir = tempfile::tempdir().unwrap();
    let server = Arc::new(server(dir.path()));
    let core = Arc::clone(&server.hub);
    let mut ticks = core.subscribe();
    let pump = crate::server::spawn_pump(Arc::clone(&server)).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // Accept one connection and run the real connection pipeline; the pump
    // routes the join publication the burst waits for.
    let connection_server = Arc::clone(&server);
    let accepted = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        super::handle_connection(stream, connection_server).await
    });

    let mut client = Client::connect(addr).await.unwrap();
    let login = client.drive_login(addr, "BurstVictim").await.unwrap();
    client.drive_configuration().await.unwrap();
    // `Joined` is published before the edge starts the multi-second burst, so
    // dropping the socket after this observation kills the connection while
    // `send_join_burst` is still writing.
    let uuid = login.uuid;
    wait_snapshot(&mut ticks, Duration::from_secs(60), |snapshot| {
        snapshot.entities.iter().any(|row| row.uuid == uuid)
    })
    .await;
    drop(client);

    let left = wait_snapshot(&mut ticks, Duration::from_secs(60), |snapshot| {
        !snapshot.entities.iter().any(|row| row.uuid == uuid)
    })
    .await;
    assert!(
        !left.entities.iter().any(|row| row.uuid == uuid),
        "a client that died during the join burst still retires its core player"
    );
    // The single teardown point must also drop the edge connection entry and
    // any admin registration (the burst died before registration happened, so
    // neither may linger).
    let outcome = tokio::time::timeout(Duration::from_secs(10), accepted)
        .await
        .expect("connection task returned")
        .expect("connection task joined");
    assert!(
        outcome.is_err(),
        "the socket must die inside the join burst (send_join_burst), not after it"
    );
    assert_eq!(server.connections.len(), 0, "edge connection entry retired");
    assert!(
        !server
            .admin
            .roster(false)
            .iter()
            .any(|name| name == "BurstVictim"),
        "no admin registration lingers"
    );

    core.shutdown();
    pump.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_while_waiting_for_join_retires_native_player() {
    let dir = tempfile::tempdir().unwrap();
    let server = Arc::new(server(dir.path()));
    let core = Arc::clone(&server.hub);
    let mut ticks = core.subscribe();
    // Deliberately no pump: native Join commits, but its notification cannot
    // reach the edge's waiter before cancellation.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let connection_server = Arc::clone(&server);
    let accepted = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        handle_connection(socket, connection_server).await
    });
    let mut client = Client::connect(address).await.unwrap();
    let login = client.drive_login(address, "WaitVictim").await.unwrap();
    client.drive_configuration().await.unwrap();
    wait_snapshot(&mut ticks, Duration::from_secs(60), |snapshot| {
        snapshot.entities.iter().any(|row| row.uuid == login.uuid)
    })
    .await;
    accepted.abort();
    assert!(accepted.await.unwrap_err().is_cancelled());
    wait_snapshot(&mut ticks, Duration::from_secs(60), |snapshot| {
        !snapshot.entities.iter().any(|row| row.uuid == login.uuid)
    })
    .await;
    assert!(server.connections.inner.lock().admitted.is_empty());
    assert!(server.connections.inner.lock().joining.is_empty());
    core.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pre_play_deadline_is_absolute_across_repeated_configuration_packets() {
    use mc_protocol::packets::configuration::{
        ClientboundKnownPacks, ServerboundClientInformation,
    };
    use mc_protocol::packets::{ChatVisibility, ClientInformation, MainHand, ParticleStatus};
    let dir = tempfile::tempdir().unwrap();
    let server = Arc::new(server(dir.path()));
    let core = Arc::clone(&server.hub);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let accepted = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        handle_connection(socket, server).await
    });
    let mut client = Client::connect(address).await.unwrap();
    client.drive_login(address, "Deadline").await.unwrap();
    loop {
        if client.read_frame().await.unwrap().id == ClientboundKnownPacks::ID {
            break;
        }
    }
    let packet = ServerboundClientInformation {
        information: ClientInformation {
            language: "en_us".into(),
            view_distance: 2,
            chat_visibility: ChatVisibility::Full,
            chat_colors: true,
            model_customisation: 0,
            main_hand: MainHand::Right,
            text_filtering_enabled: false,
            allows_listing: true,
            particle_status: ParticleStatus::All,
        },
    };
    let mut repeated = tokio::time::interval(Duration::from_secs(8));
    let mut accepted = accepted;
    let result = tokio::time::timeout(Duration::from_secs(55), async {
        loop {
            tokio::select! {
                result = &mut accepted => break result.unwrap(),
                _ = repeated.tick() => {
                    if client.write_packet(&packet).await.is_err() {
                        break accepted.await.unwrap();
                    }
                }
            }
        }
    })
    .await
    .expect("absolute pre-play timeout must terminate repeated valid packets");
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("total pre-play deadline")
    );
    core.shutdown();
}
