use super::*;
use std::time::Duration;

#[test]
fn pending_login_reserves_capacity_until_retirement_finishes() {
    let registry = ConnectionRegistry::new();
    let a = registry.alloc_id();
    let b = registry.alloc_id();
    let policy = |full: bool| full.then(|| serde_json::json!({"text":"full"}));
    assert!(registry.admit(a, 1, policy).is_none());
    assert_eq!(registry.len(), 0, "Configuration has no Play handles yet");
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
    let (_, writer) = crate::connection::Connection::new(socket).into_play_halves();
    let (reply_tx, reply_rx) = mpsc::channel(1);
    let (reliable_tx, reliable_rx) = mpsc::channel(1);
    let (world_tx, world_rx) = mpsc::channel(1);
    let task = tokio::spawn(play_writer(
        writer,
        Arc::new(NetCounters::new()),
        Arc::new(ConnectionRegistry::new()),
        ConnectionId(1),
        PlayReceivers {
            reliable: reliable_rx,
            world: world_rx,
            dh: mpsc::channel(1).1,
            keepalive: reply_rx,
        },
    ));
    (
        crate::connection::Connection::new(client),
        task,
        reply_tx,
        reliable_tx,
        world_tx,
    )
}

#[tokio::test]
async fn keepalive_requires_matching_echo_and_times_out_quiet_readers() {
    let (mut client, task, replies, _reliable, _world) = writer_pair().await;
    tokio::time::pause();
    let frame = client.read_frame_play().await.unwrap();
    assert_eq!(
        frame.id,
        mc_protocol::packets::play::ClientboundKeepAlive::ID
    );
    let mut body = frame.body;
    let first = mc_protocol::packets::play::ClientboundKeepAlive::decode(&mut body).unwrap();
    replies.send(first.id).await.unwrap();
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
    let (_client, task, replies, _reliable, _world) = writer_pair().await;
    replies.send(99).await.unwrap();
    assert_eq!(task.await.unwrap().0, "keepalive-invalid-reply");
}
