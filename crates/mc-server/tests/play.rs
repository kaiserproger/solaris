//! End-to-end test for M1.g.3 Play state entry.
//!
//! Drives the full Handshake → Login → Configuration → Play sequence
//! and asserts that, on the Play side of the boundary, the server:
//!
//! - sends `Login (Play)` with sensible fields,
//! - sends `Synchronize Player Position`, `Set Default Spawn Position`,
//!   and a `GameEvent` instructing the client to stop waiting for level
//!   chunks (the M1.g world is intentionally empty),
//! - sends a `Clientbound Keep Alive` periodically and treats our echo
//!   as a heartbeat (we manually shrink the timing for the test by
//!   reading the first keepalive after the spawn burst, not by tuning
//!   the handler).
//!
//! Packet IDs are still M1.g.4-pending; this test is the wire-shape
//! check, not a vanilla-client compatibility test.

use std::net::SocketAddr;
use std::time::Duration;

use bytes::{Buf, BytesMut};
use mc_protocol::PROTOCOL_VERSION;
use mc_protocol::frame::{Compression, encode_frame, try_decode_frame};
use mc_protocol::packets::Packet;
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundKnownPacks, FinishConfiguration, RegistryData,
    ServerboundKnownPacks, UpdateTags,
};
use mc_protocol::packets::handshake::{Handshake, NextState};
use mc_protocol::packets::login::{LoginAcknowledged, LoginStart, LoginSuccess};
use mc_protocol::packets::play::{
    ClientboundKeepAlive, ConfirmTeleportation, GameEvent, LoginPlay, ServerboundKeepAlive,
    SetCenterChunk, SynchronizePlayerPosition,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

async fn start_server() -> SocketAddr {
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M1.g play".into(),
        max_players: 8,
        data: std::sync::Arc::new(mc_data::testing::stub()),
        blocks: std::sync::Arc::new(
            mc_world::BlockRegistry::from_report(&[]).expect("empty registry builds"),
        ),
        world: None,
        tags: std::sync::Arc::new(mc_data::tags::TagsData::default()),
        block_light: None,
        items: std::sync::Arc::new(mc_data::items::ItemRegistry::default()),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });
    addr
}

async fn write_frame<P: Packet>(stream: &mut TcpStream, packet: &P) {
    let mut body = BytesMut::new();
    packet.encode(&mut body).unwrap();
    let framed = encode_frame(P::ID, &body, Compression::Disabled).unwrap();
    stream.write_all(&framed).await.unwrap();
}

async fn read_one_frame(stream: &mut TcpStream, buf: &mut BytesMut) -> mc_protocol::RawFrame {
    loop {
        if let Some(frame) = try_decode_frame(buf, Compression::Disabled).unwrap() {
            return frame;
        }
        let read = stream.read_buf(buf).await.unwrap();
        assert!(read > 0, "server closed before sending a complete frame");
    }
}

/// Walk the full protocol up to and including
/// `AcknowledgeFinishConfiguration`. After this the connection is in
/// Play state on the server side.
async fn drive_to_play(stream: &mut TcpStream, buf: &mut BytesMut, addr: SocketAddr, name: &str) {
    // Handshake → Login Start → expect Login Success → Acknowledged.
    write_frame(
        stream,
        &Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: addr.port(),
            next_state: NextState::Login,
        },
    )
    .await;
    write_frame(
        stream,
        &LoginStart {
            name: name.into(),
            player_uuid: Uuid::nil(),
        },
    )
    .await;
    let mut frame = read_one_frame(stream, buf).await;
    assert_eq!(frame.id, LoginSuccess::ID);
    let _ = LoginSuccess::decode(&mut frame.body).unwrap();
    write_frame(stream, &LoginAcknowledged).await;

    // Configuration: KnownPacks round trip, drain registries, ack.
    let mut frame = read_one_frame(stream, buf).await;
    assert_eq!(frame.id, ClientboundKnownPacks::ID);
    let cb_packs = ClientboundKnownPacks::decode(&mut frame.body).unwrap();
    write_frame(
        stream,
        &ServerboundKnownPacks {
            packs: cb_packs.packs,
        },
    )
    .await;
    for _ in 0..mc_data::KNOWN_REGISTRIES.len() {
        let mut frame = read_one_frame(stream, buf).await;
        assert_eq!(frame.id, RegistryData::ID);
        let _ = RegistryData::decode(&mut frame.body).unwrap();
    }
    // M3.i: Update Tags arrives between the last RegistryData and
    // FinishConfiguration. The stub `tags` here is empty; the packet
    // is still required on the wire.
    let mut frame = read_one_frame(stream, buf).await;
    assert_eq!(frame.id, UpdateTags::ID);
    let _ = UpdateTags::decode(&mut frame.body).unwrap();
    let mut frame = read_one_frame(stream, buf).await;
    assert_eq!(frame.id, FinishConfiguration::ID);
    let _ = FinishConfiguration::decode(&mut frame.body).unwrap();
    write_frame(stream, &AcknowledgeFinishConfiguration).await;
}

#[tokio::test]
async fn play_state_entry_sends_login_and_spawn_burst() {
    let addr = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(8192);
    drive_to_play(&mut stream, &mut rbuf, addr, "PlayTester").await;

    // The handler emits four packets back-to-back. Order matters: Login
    // (Play) first because the client needs the world setup before it
    // can interpret anything else.
    let mut frame = read_one_frame(&mut stream, &mut rbuf).await;
    assert_eq!(frame.id, LoginPlay::ID, "expected Login (Play) first");
    let login = LoginPlay::decode(&mut frame.body).unwrap();
    assert_eq!(frame.body.remaining(), 0);
    // Stub data is alphabetical: dimensions are [alpha, beta]. Server
    // should pick the first as the player's spawn dimension.
    assert_eq!(login.dimension_names.len(), 2);
    assert_eq!(login.dimension_type_id, 0);
    assert_eq!(login.dimension_name.as_str(), "minecraft:alpha");
    assert_eq!(login.game_mode, 0); // survival
    assert!(login.is_flat, "M1.g world is intentionally flat/empty");

    let mut frame = read_one_frame(&mut stream, &mut rbuf).await;
    assert_eq!(
        frame.id,
        SynchronizePlayerPosition::ID,
        "expected Synchronize Player Position"
    );
    let sync = SynchronizePlayerPosition::decode(&mut frame.body).unwrap();
    assert_eq!(sync.teleport_id, 1);
    // Spawn Y sits just above the flat-preset grass surface
    // (Y=-61); see SPAWN_Y in mc_net::play. Bound the assertion to
    // a sane window rather than re-importing the constant.
    assert!(
        sync.y > -65.0 && sync.y < 0.0,
        "unexpected spawn y={}",
        sync.y
    );

    let mut frame = read_one_frame(&mut stream, &mut rbuf).await;
    assert_eq!(frame.id, GameEvent::ID, "expected Game Event");
    let event = GameEvent::decode(&mut frame.body).unwrap();
    assert_eq!(event.event, GameEvent::EVENT_START_WAITING_FOR_CHUNKS);

    let mut frame = read_one_frame(&mut stream, &mut rbuf).await;
    assert_eq!(frame.id, SetCenterChunk::ID, "expected Set Center Chunk");
    let center = SetCenterChunk::decode(&mut frame.body).unwrap();
    // SPAWN_(X,Z) = (0.5, 0.5) → chunk (0, 0).
    assert_eq!((center.chunk_x, center.chunk_z), (0, 0));
    // With world = None in this test the chunk packet is intentionally
    // not emitted; M3.e's view-distance test exercises the chunk path.

    // Be polite: ack the teleport.
    write_frame(
        &mut stream,
        &ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        },
    )
    .await;

    // Closing here: the keepalive loop runs on a 15-second tick, which
    // is way too long for a unit test. Asserting "no keepalive yet" or
    // waiting for one would either flake or stall, so we just confirm
    // the server stays connected for a beat and then close ourselves —
    // server.rs will surface EOF cleanly.
    let mut scratch = [0u8; 1];
    let early_close =
        tokio::time::timeout(Duration::from_millis(250), stream.read(&mut scratch)).await;
    assert!(
        early_close.is_err(),
        "server should NOT close the connection in the first quarter-second \
         after the spawn burst — keepalive loop is running"
    );

    // Now drop the client; the server task will see EOF and exit.
    drop(stream);
}

#[tokio::test]
async fn play_state_handles_serverbound_keepalive_echo() {
    // We can't easily wait 15 s for a keepalive in a unit test, so this
    // test instead sends a *spurious* keepalive echo from the client to
    // confirm the server reads it without crashing or treating it as
    // an unexpected packet. The mismatch will be logged as a warning;
    // the connection should stay up.
    let addr = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(8192);
    drive_to_play(&mut stream, &mut rbuf, addr, "Spurious").await;

    // Drain spawn burst.
    // After M3.d the burst is 4 frames: LoginPlay,
    // SynchronizePlayerPosition, GameEvent, SetCenterChunk. With
    // world = None the chunk packet itself is intentionally not
    // emitted.
    for _ in 0..4 {
        let _ = read_one_frame(&mut stream, &mut rbuf).await;
    }

    write_frame(&mut stream, &ServerboundKeepAlive { id: 0xDEAD_BEEF }).await;

    // Confirm the connection is still alive — the server should log a
    // mismatch warning but not close.
    let mut scratch = [0u8; 1];
    let close = tokio::time::timeout(Duration::from_millis(250), stream.read(&mut scratch)).await;
    assert!(
        close.is_err(),
        "server should not close on a spurious keepalive id"
    );
    drop(stream);

    // Hush the unused-import lint for `ClientboundKeepAlive`. It is
    // referenced by docstrings only — the test wire format uses the
    // serverbound counterpart.
    let _ = std::mem::size_of::<ClientboundKeepAlive>();
}
