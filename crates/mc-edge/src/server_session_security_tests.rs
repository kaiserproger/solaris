use super::*;
use std::time::Duration;

#[test]
fn pending_join_reserves_capacity_until_retirement_finishes() {
    let registry = ConnectionRegistry::new();
    let a = registry.alloc_id();
    let b = registry.alloc_id();
    let policy = |full: bool| full.then(|| serde_json::json!({"text":"full"}));
    assert!(registry.admit(a, 1, policy).is_none());
    assert_eq!(registry.len(), 0, "pending Join has no Play handles yet");
    assert!(registry.admit(b, 1, policy).is_some());
    registry.remove(a);
    assert!(
        registry.admit(b, 1, policy).is_some(),
        "queued Leave retains capacity"
    );
    registry.finish(a);
    assert!(registry.admit(b, 1, policy).is_none());
}

#[test]
fn retired_connections_never_recreate_join_or_reliable_stashes() {
    let registry = ConnectionRegistry::new();
    let id = registry.joining_id();
    registry.remove(id);
    let joined = (
        mc_core::boundary::EntityId(1),
        [0.0; 3],
        0,
        Default::default(),
    );
    registry.stash_join(id, joined);
    registry.stash_disconnect_reason(id, "late kick".into());
    let mut changes = vec![mc_core::boundary::PlayerChange::Chat {
        message: "late".into(),
    }];
    assert!(registry.handles_or_stash(id, &mut changes).is_none());
    assert!(changes.is_empty());
    assert!(registry.take_join(id).is_none());
    assert!(registry.take_disconnect_reason(id).is_none());
    assert!(registry.inner.lock().pending_reliable.is_empty());
}

async fn writer_pair() -> (
    crate::connection::Connection,
    crate::connection::PlayReader,
    tokio::task::JoinHandle<(&'static str, Option<String>)>,
    mpsc::Sender<i64>,
    mpsc::Sender<bytes::Bytes>,
    mpsc::Sender<bytes::Bytes>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (socket, _) = listener.accept().await.unwrap();
    let (reader, writer) = crate::connection::Connection::new(socket).into_play_halves();
    let (reply_tx, reply_rx) = mpsc::channel(1);
    let (reliable_tx, reliable_rx) = mpsc::channel(1);
    let (world_tx, world_rx) = mpsc::channel(1);
    let task = tokio::spawn(play_writer(
        writer,
        Arc::new(NetCounters::new()),
        Arc::new(ConnectionRegistry::new()),
        ConnectionId(1),
        uuid::Uuid::nil(),
        PlayReceivers {
            reliable: reliable_rx,
            world: world_rx,
            dh: mpsc::channel(1).1,
            keepalive: reply_rx,
        },
    ));
    (
        crate::connection::Connection::new(client),
        reader,
        task,
        reply_tx,
        reliable_tx,
        world_tx,
    )
}

#[tokio::test]
async fn keepalive_round_trip_reaches_player_info_wire() {
    use mc_protocol::packets::play::{ClientboundKeepAlive, PlayerInfoActions, PlayerInfoUpdate};
    let (mut client, _reader, _task, replies, _reliable, _world) = writer_pair().await;
    tokio::time::pause();
    let challenge = client.read_frame_play().await.unwrap();
    let challenge = ClientboundKeepAlive::decode(&mut &challenge.body[..]).unwrap();
    tokio::time::advance(Duration::from_millis(160)).await;
    replies.send(challenge.id).await.unwrap();
    tokio::task::yield_now().await;
    let frame = client.read_frame_play().await.unwrap();
    assert_eq!(frame.id, PlayerInfoUpdate::ID);
    let update = PlayerInfoUpdate::decode(&mut &frame.body[..]).unwrap();
    assert_eq!(update.actions, PlayerInfoActions::UPDATE_LATENCY);
    assert_eq!(update.entries.len(), 1);
    assert!(update.entries[0].latency >= 40);
    assert_eq!(update.entries[0].profile_id, uuid::Uuid::nil());
}

#[tokio::test]
async fn measured_latency_is_broadcast_to_other_players() {
    use mc_protocol::frame::{Compression, try_decode_frame};
    use mc_protocol::packets::play::{ClientboundKeepAlive, PlayerInfoActions, PlayerInfoUpdate};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
    let (socket, _) = listener.accept().await.unwrap();
    let (_, writer) = crate::connection::Connection::new(socket).into_play_halves();
    let registry = Arc::new(ConnectionRegistry::new());
    let other = registry.joining_id();
    let (other_tx, mut other_rx) = mpsc::channel(4);
    registry.register(other, ConnectionHandles {
        reliable_tx: other_tx,
        world_tx: mpsc::channel(1).0,
    }).unwrap();
    let (replies, reply_rx) = mpsc::channel(1);
    let (_reliable, reliable_rx) = mpsc::channel(1);
    let (_world, world_rx) = mpsc::channel(1);
    let profile = uuid::Uuid::from_u128(42);
    let _task = tokio::spawn(play_writer(
        writer, Arc::new(NetCounters::new()), registry, ConnectionId(99), profile,
        PlayReceivers { reliable: reliable_rx, world: world_rx, dh: mpsc::channel(1).1, keepalive: reply_rx },
    ));
    let mut client = crate::connection::Connection::new(client);
    tokio::time::pause();
    let frame = client.read_frame_play().await.unwrap();
    let challenge = ClientboundKeepAlive::decode(&mut &frame.body[..]).unwrap();
    tokio::time::advance(Duration::from_millis(160)).await;
    replies.send(challenge.id).await.unwrap();
    tokio::task::yield_now().await;
    let wire = other_rx.recv().await.unwrap();
    let mut wire = bytes::BytesMut::from(&wire[..]);
    let frame = try_decode_frame(&mut wire, Compression::Threshold(256)).unwrap().unwrap();
    assert_eq!(frame.id, PlayerInfoUpdate::ID);
    let update = PlayerInfoUpdate::decode(&mut &frame.body[..]).unwrap();
    assert_eq!(update.actions, PlayerInfoActions::UPDATE_LATENCY);
    assert_eq!(update.entries[0].profile_id, profile);
    assert!(update.entries[0].latency >= 40);
}

#[test]
fn keepalive_latency_uses_weighted_round_trip() {
    assert_eq!(smooth_latency(0, 160), 40);
    assert_eq!(smooth_latency(40, 80), 50);
}

#[tokio::test]
async fn keepalive_requires_matching_echo_and_times_out_quiet_readers() {
    let (mut client, _reader, task, replies, _reliable, _world) = writer_pair().await;
    tokio::time::pause();
    let frame = client.read_frame_play().await.unwrap();
    assert_eq!(
        frame.id,
        mc_protocol::packets::play::ClientboundKeepAlive::ID
    );
    let mut body = frame.body;
    let first = mc_protocol::packets::play::ClientboundKeepAlive::decode(&mut body).unwrap();
    replies.send(first.id).await.unwrap();
    let update = client.read_frame_play().await.unwrap();
    assert_eq!(update.id, mc_protocol::packets::play::PlayerInfoUpdate::ID);
    let frame = client.read_frame_play().await.unwrap();
    let mut body = frame.body;
    let second = mc_protocol::packets::play::ClientboundKeepAlive::decode(&mut body).unwrap();
    assert_ne!(
        first.id, second.id,
        "even a paused core produces fresh challenges"
    );
    // No echo for the second challenge. Ordinary outgoing traffic cannot renew it.
    tokio::time::advance(Duration::from_secs(30)).await;
    assert_eq!(task.await.unwrap().0, "keepalive-timeout");
}

#[tokio::test]
async fn keepalive_rejects_unsolicited_reply() {
    let (_client, _reader, task, replies, _reliable, _world) = writer_pair().await;
    replies.send(99).await.unwrap();
    assert_eq!(task.await.unwrap().0, "keepalive-invalid-reply");
}

#[tokio::test]
async fn checkpoint_wait_keeps_play_echoes_live_and_preserves_queued_input_order() {
    use mc_protocol::packets::play::{
        ClientboundKeepAlive, ServerboundChatCommand, ServerboundKeepAlive,
    };
    let (mut client, mut reader, writer, replies, _reliable, _world) = writer_pair().await;
    tokio::time::pause();
    let (finish, completion) = tokio::sync::oneshot::channel::<()>();
    let waiting = tokio::spawn(async move {
        let mut pending = PendingPlayFrames::default();
        let mut limiter = PacketLimiter::new(&Default::default());
        let result = await_play_operation(
            completion,
            &mut reader,
            &replies,
            &mut pending,
            &mut limiter,
        )
        .await;
        (result, pending)
    });
    for command in ["first", "second"] {
        client
            .send_packet(&ServerboundChatCommand {
                command: command.into(),
            })
            .await
            .unwrap();
    }
    // The checkpoint completion remains withheld for four 15-second heartbeat
    // intervals: longer than both the observed 45s save and the 30s echo deadline.
    for _ in 0..4 {
        tokio::time::advance(Duration::from_secs(15)).await;
        tokio::time::resume();
        let frame = client
            .read_frame_play()
            .await
            .expect("checkpoint wait must not close a responsive peer");
        assert_eq!(frame.id, ClientboundKeepAlive::ID);
        let challenge = ClientboundKeepAlive::decode(&mut &frame.body[..]).unwrap();
        client
            .send_packet(&ServerboundKeepAlive { id: challenge.id })
            .await
            .unwrap();
        let update = client.read_frame_play().await.unwrap();
        assert_eq!(update.id, mc_protocol::packets::play::PlayerInfoUpdate::ID);
        tokio::time::pause();
    }
    finish.send(()).unwrap();
    let (result, mut pending) = waiting.await.unwrap();
    result.unwrap().unwrap();
    for expected in ["first", "second"] {
        let frame = pending.pop_front().expect("queued gameplay frame");
        assert_eq!(frame.id, ServerboundChatCommand::ID);
        let command = ServerboundChatCommand::decode(&mut &frame.body[..]).unwrap();
        assert_eq!(command.command, expected);
    }
    assert!(pending.is_empty());
    writer.abort();
    let _ = writer.await;
}

#[tokio::test]
async fn completed_admin_operation_does_not_prefetch_play_input() {
    use mc_protocol::packets::play::ServerboundChatCommand;
    let (mut client, mut reader, writer, replies, _reliable, _world) = writer_pair().await;
    client
        .send_packet(&ServerboundChatCommand {
            command: "next".into(),
        })
        .await
        .unwrap();
    let mut pending = PendingPlayFrames::default();
    let mut limiter = PacketLimiter::new(&Default::default());
    await_play_operation(
        std::future::ready(()),
        &mut reader,
        &replies,
        &mut pending,
        &mut limiter,
    )
    .await
    .unwrap();
    assert!(
        pending.is_empty(),
        "immediate completion keeps the ordinary path"
    );
    let frame = reader.read_frame().await.unwrap();
    assert_eq!(frame.id, ServerboundChatCommand::ID);
    writer.abort();
    let _ = writer.await;
}

#[tokio::test]
async fn pending_admin_input_budget_fails_explicitly_instead_of_dropping_frames() {
    use mc_protocol::packets::play::ServerboundChatCommand;
    let (mut client, mut reader, writer, replies, _reliable, _world) = writer_pair().await;
    let waiting = tokio::spawn(async move {
        let mut pending = PendingPlayFrames::default();
        let mut limiter = PacketLimiter::new(&Default::default());
        await_play_operation(
            std::future::pending::<()>(),
            &mut reader,
            &replies,
            &mut pending,
            &mut limiter,
        )
        .await
    });
    let command = ServerboundChatCommand {
        command: "x".repeat(32700),
    };
    for _ in 0..40 {
        if client.send_packet(&command).await.is_err() {
            break;
        }
    }
    let error = waiting.await.unwrap().unwrap_err();
    assert_eq!(
        error.to_string(),
        "Too much pending input while an administrative command is running (1 MiB limit)"
    );
    writer.abort();
    let _ = writer.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn edge_kick_flushes_terminal_packet_after_an_earlier_reliable_batch() {
    use mc_protocol::frame::Compression;
    use mc_protocol::packets::play::{ClientboundKeepAlive, PlayDisconnect};
    let root = tempfile::tempdir().unwrap();
    let server = crate::console::tests::server(root.path());
    let text = format!(
        "Pending administrative input exceeded its limit: {}",
        "x".repeat(60000)
    );
    for _ in 0..32 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (peer, _) = listener.accept().await.unwrap();
        let mut peer = crate::connection::Connection::new(peer);
        peer.set_compression(Compression::Threshold(256));
        let (_reader, writer) = peer.into_play_halves();
        let mut client = crate::connection::Connection::new(socket);
        client.set_compression(Compression::Threshold(256));
        let id = server.connections.joining_id();
        let (reliable, reliable_rx) = mpsc::channel(4);
        let (world, world_rx) = mpsc::channel(1);
        server
            .connections
            .register(
                id,
                ConnectionHandles {
                    reliable_tx: reliable.clone(),
                    world_tx: world,
                },
            )
            .unwrap();
        let (_replies, keepalive) = mpsc::channel(1);
        let task = tokio::spawn(play_writer(
            writer,
            Arc::new(NetCounters::new()),
            Arc::clone(&server.connections),
            id,
            uuid::Uuid::nil(),
            PlayReceivers {
                reliable: reliable_rx,
                world: world_rx,
                dh: mpsc::channel(1).1,
                keepalive,
            },
        ));
        let mut body = Vec::new();
        ClientboundKeepAlive { id: 42 }.encode(&mut body).unwrap();
        let mut previous = bytes::BytesMut::new();
        mc_protocol::frame::append_frame(
            &mut previous,
            ClientboundKeepAlive::ID,
            &body,
            Compression::Threshold(256),
        )
        .unwrap();
        reliable.send(previous.freeze()).await.unwrap();
        server.kick(id, serde_json::json!({"text": &text})).unwrap();
        let previous = client.read_frame_play().await.unwrap();
        assert_eq!(previous.id, ClientboundKeepAlive::ID);
        let terminal = client
            .read_frame_play()
            .await
            .expect("terminal packet must precede writer shutdown");
        assert_eq!(terminal.id, PlayDisconnect::ID);
        let terminal = PlayDisconnect::decode(&mut &terminal.body[..]).unwrap();
        let reason = mc_nbt::read_network(&mut &terminal.reason_nbt[..]).unwrap();
        assert_eq!(
            reason,
            mc_nbt::Tag::Compound(vec![("text".into(), mc_nbt::Tag::String(text.clone()))])
        );
        assert_eq!(task.await.unwrap().0, "core-disconnect");
        server.connections.remove(id);
        server.connections.finish(id);
    }
    server.hub.shutdown();
}
