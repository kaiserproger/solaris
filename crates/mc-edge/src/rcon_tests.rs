use super::*;

async fn pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (server, _) = listener.accept().await.unwrap();
    (client, server)
}

#[tokio::test]
async fn serverops_rcon_fragmented_frames_and_lengths() {
    let (mut client, mut server) = pair().await;
    let task = tokio::spawn(async move { read(&mut server).await });
    for byte in [
        14i32.to_le_bytes().as_slice(),
        23i32.to_le_bytes().as_slice(),
        2i32.to_le_bytes().as_slice(),
        b"list\0\0",
    ]
    .concat()
    {
        client.write_all(&[byte]).await.unwrap();
    }
    let request = task.await.unwrap().unwrap();
    assert_eq!(
        (request.id, request.kind, request.body.as_str()),
        (23, 2, "list")
    );
    let (mut client, mut server) = pair().await;
    client.write_i32_le(i32::MAX).await.unwrap();
    assert!(read(&mut server).await.is_err());
    let (mut client, mut server) = pair().await;
    client
        .write_all(&[10i32.to_le_bytes().as_slice(), &[0; 8], &[1, 0]].concat())
        .await
        .unwrap();
    assert!(read(&mut server).await.is_err());
}

#[tokio::test]
async fn serverops_rcon_password_admin_and_native_console_feedback() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = crate::console::tests::server(dir.path());
    server.cfg.rcon.password = "secret".into();
    let server = Arc::new(server);
    // A bounded event-driven consumer exercises the same console outbox owner
    // without starting graphical or public network services.
    let mut ticks = server.hub.subscribe();
    let pump_server = Arc::clone(&server);
    let pump = tokio::spawn(async move {
        while let Ok(tick) = ticks.recv().await {
            for (connection, changes) in pump_server.hub.try_drain_outbox() {
                if connection == mc_core::command_console::CONNECTION {
                    crate::console::output(&pump_server.console_state, &changes);
                }
            }
            pump_server.console_state.drained(tick.tick);
        }
    });
    let (mut client, socket) = pair().await;
    let connection = tokio::spawn(session(socket, Arc::clone(&server)));
    send(&mut client, 1, 2, "op Intruder").await.unwrap();
    let denied = read(&mut client).await.unwrap();
    assert_eq!((denied.id, denied.kind), (-1, 2));
    assert!(!server.cfg.server_dir.join("ops.json").exists());
    connection.await.unwrap().unwrap();
    let (mut client, socket) = pair().await;
    let connection = tokio::spawn(session(socket, Arc::clone(&server)));
    send(&mut client, 2, 3, "wrong").await.unwrap();
    assert_eq!(read(&mut client).await.unwrap().id, -1);
    connection.await.unwrap().unwrap();
    let (mut client, socket) = pair().await;
    let connection = tokio::spawn(session(socket, Arc::clone(&server)));
    send(&mut client, 3, 3, "secret").await.unwrap();
    assert_eq!(read(&mut client).await.unwrap().id, 3);
    send(&mut client, 4, 2, "op Alice").await.unwrap();
    let reply = read(&mut client).await.unwrap();
    assert_eq!((reply.id, reply.kind), (4, 0));
    assert!(reply.body.contains("Alice"), "{}", reply.body);
    assert!(server.cfg.server_dir.join("ops.json").is_file());
    send(&mut client, 5, 2, "time set 4321").await.unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(30), read(&mut client))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.id, 5);
    assert_eq!(reply.body.trim(), "Set minecraft:overworld to 4321 tick(s)");
    assert!(!reply.body.contains("Alice"));
    connection.abort();
    pump.abort();
    server.hub.shutdown();
}
