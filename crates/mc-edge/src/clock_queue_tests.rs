//! Clock ordering across the real Play queues, routing and socket writer.

use super::*;
use mc_core::boundary::{ChangeSet, EntityId, TickOutput, WorldSnapshot};

fn clock_output(tick: u64, day_time: i64) -> TickOutput {
    TickOutput {
        tick,
        snapshot: Arc::new(WorldSnapshot {
            tick,
            day_time,
            day_time_rate: 1.0,
            ..Default::default()
        }),
        changes: Arc::new(ChangeSet::default()),
        times: Default::default(),
    }
}

#[tokio::test]
async fn queued_heartbeat_then_dirty_jump_is_fifo_at_the_play_writer() {
    let (server, dir) = fixture();
    let server = Arc::new(server);
    let id = server.connections.joining_id();
    let (reliable_tx, mut reliable_rx) = mpsc::channel(32);
    let (world_tx, mut world_rx) = mpsc::channel(32);
    server
        .connections
        .register(
            id,
            ConnectionHandles {
                reliable_tx,
                world_tx: world_tx.clone(),
            },
        )
        .unwrap();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let mut viewers = HashMap::default();
    viewers.insert(
        id,
        crate::plan::ViewerState::new(id, EntityId(1), [8.0, 64.0, 8.0]),
    );
    let mut cache = crate::plan::ChunkPacketCache::new();
    let (mut connections, _) = server.connections.routing_snapshot();
    publish_and_route(
        &server,
        &pool,
        &mut viewers,
        &clock_output(10, 1010),
        &mut cache,
        &mut connections,
    )
    .await;
    // The initial view was consumed before this race; only the queued heartbeat
    // and its subsequent command jump must be ordered against the world queue.
    while reliable_rx.try_recv().is_ok() {}
    while world_rx.try_recv().is_ok() {}

    let mut body = Vec::new();
    ForgetLevelChunk {
        chunk_x: 77,
        chunk_z: 88,
    }
    .encode(&mut body)
    .unwrap();
    let mut frame = bytes::BytesMut::new();
    mc_protocol::frame::append_frame(
        &mut frame,
        ForgetLevelChunk::ID,
        &body,
        mc_protocol::frame::Compression::Threshold(256),
    )
    .unwrap();
    world_tx.try_send(frame.freeze()).unwrap();
    publish_and_route(
        &server,
        &pool,
        &mut viewers,
        &clock_output(30, 1030),
        &mut cache,
        &mut connections,
    )
    .await;
    publish_and_route(
        &server,
        &pool,
        &mut viewers,
        &clock_output(31, 6000),
        &mut cache,
        &mut connections,
    )
    .await;
    let queued = reliable_rx.len() + world_rx.len();
    assert_eq!(queued, 3, "two clocks plus an unrelated world frame");

    // Start the actual writer only after both queues are populated. With a
    // best-effort heartbeat it would drain the dirty reliable clock first,
    // then the old heartbeat from the world queue, rolling client time back.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client_socket = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (server_socket, _) = listener.accept().await.unwrap();
    let mut server_connection = crate::connection::Connection::new(server_socket);
    server_connection.set_compression(mc_protocol::frame::Compression::Threshold(256));
    let (_, writer) = server_connection.into_play_halves();
    let writer_task = tokio::spawn(play_writer(
        writer,
        Arc::clone(&server.net_counters),
        Arc::clone(&server.connections),
        id,
        PlayReceivers {
            reliable: reliable_rx,
            world: world_rx,
            dh: mpsc::channel(1).1,
            keepalive: mpsc::channel(1).1,
        },
    ));
    let mut client = crate::connection::Connection::new(client_socket);
    client.set_compression(mc_protocol::frame::Compression::Threshold(256));
    let mut clocks = Vec::new();
    let mut world_frames = 0;
    for _ in 0..queued {
        let frame =
            tokio::time::timeout(std::time::Duration::from_secs(5), client.read_frame_play())
                .await
                .unwrap()
                .unwrap();
        if frame.id == ClientboundSetTime::ID {
            clocks.push(ClientboundSetTime::decode(&mut &frame.body[..]).unwrap());
        } else {
            assert_eq!(frame.id, ForgetLevelChunk::ID);
            let forget = ForgetLevelChunk::decode(&mut &frame.body[..]).unwrap();
            assert_eq!((forget.chunk_x, forget.chunk_z), (77, 88));
            world_frames += 1;
        }
    }
    writer_task.abort();
    assert!(writer_task.await.unwrap_err().is_cancelled());
    server.connections.remove(id);
    server.hub.shutdown();
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(world_frames, 1, "both real outbound queues were drained");
    assert_eq!(
        clocks,
        vec![
            ClientboundSetTime::overworld(30, 1030, 1.0),
            ClientboundSetTime::overworld(31, 6000, 1.0),
        ],
        "the client's final clock must be the command jump, never the older heartbeat"
    );
}

#[tokio::test]
async fn routing_snapshot_drops_overflow_victim_before_any_later_packets() {
    let (server, dir) = fixture();
    let server = Arc::new(server);
    let id = server.connections.joining_id();
    let (reliable_tx, mut reliable_rx) = mpsc::channel(1);
    let (world_tx, mut world_rx) = mpsc::channel(8);
    reliable_tx
        .try_send(bytes::Bytes::from_static(b"already queued"))
        .unwrap();
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
    let (mut connections, _) = server.connections.routing_snapshot();
    let mut viewers = HashMap::from([(
        id,
        crate::plan::ViewerState::new(id, EntityId(1), [8.0, 64.0, 8.0]),
    )]);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    publish_and_route(
        &server,
        &pool,
        &mut viewers,
        &clock_output(30, 1030),
        &mut crate::plan::ChunkPacketCache::new(),
        &mut connections,
    )
    .await;
    assert!(!connections.contains_key(&id));
    assert!(server.connections.get(id).is_none());
    assert!(!viewers.contains_key(&id));
    assert_eq!(
        server.net_counters.reliable_drops.load(Ordering::Relaxed),
        1
    );
    assert_eq!(
        reliable_rx.recv().await.unwrap(),
        b"already queued".as_slice()
    );
    assert!(reliable_rx.recv().await.is_none());
    assert!(world_rx.recv().await.is_none());
    server.hub.shutdown();
    std::fs::remove_dir_all(dir).unwrap();
}
