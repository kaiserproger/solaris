//! End-to-end test for the M1.d Login state.
//!
//! Boots a real `mc_net::run` on an ephemeral port, drives the
//! Handshake → LoginStart → LoginSuccess → LoginAcknowledged sequence,
//! and asserts:
//!
//! - the server replies with the expected offline UUID (Java-compatible
//!   `nameUUIDFromBytes` derivation),
//! - the connection is closed after acknowledgement (Configuration state
//!   is M1.e, not implemented),
//! - the listener stays up for parallel clients.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::{Buf, BytesMut};
use mc_protocol::PROTOCOL_VERSION;
use mc_protocol::codec::read_varint_partial;
use mc_protocol::frame::{Compression, encode_frame, try_decode_frame};
use mc_protocol::packets::Packet;
use mc_protocol::packets::configuration::{ClientboundKnownPacks, UpdateEnabledFeatures};
use mc_protocol::packets::handshake::{Handshake, NextState};
use mc_protocol::packets::login::{
    EncryptionRequest, EncryptionResponse, GameProfileProperty, LoginAcknowledged, LoginDisconnect,
    LoginStart, LoginSuccess, SetCompression,
};
use mc_protocol::packets::status::{StatusRequest, StatusResponse};
use rsa::pkcs8::DecodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

async fn start_server() -> SocketAddr {
    start_server_with_policy(mc_net::ChunkPipelinePolicy::default()).await
}

async fn start_server_with_policy(chunk_pipeline: mc_net::ChunkPipelinePolicy) -> SocketAddr {
    start_server_with_policy_and_permissions(
        chunk_pipeline,
        mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
    )
    .await
}

async fn start_server_with_policy_and_permissions(
    chunk_pipeline: mc_net::ChunkPipelinePolicy,
    command_permissions: mc_net::CommandPermissionConfig,
) -> SocketAddr {
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M1.d login".into(),
        max_players: 8,
        view_distance: 10,
        data: std::sync::Arc::new(mc_data::testing::stub()),
        blocks: std::sync::Arc::new(
            mc_world::BlockRegistry::from_report(&[]).expect("empty registry builds"),
        ),
        world: None,
        tags: std::sync::Arc::new(mc_data::tags::TagsData::default()),
        recipes: std::sync::Arc::new(Vec::new()),
        loot: std::sync::Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items: std::sync::Arc::new(mc_data::items::ItemRegistry::default()),
        item_facts: std::sync::Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: std::sync::Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: std::sync::Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline,
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions,
        loader_manifest: None,
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });
    addr
}

async fn start_server_with_login_access(login_access: mc_net::LoginAccessConfig) -> SocketAddr {
    let permissions = mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true)
        .with_login_access(login_access);
    start_server_with_policy_and_permissions(mc_net::ChunkPipelinePolicy::default(), permissions)
        .await
}

fn network_config_from_server_config(cfg: mc_server::ServerConfig) -> mc_net::ServerConfig {
    cfg.to_network(
        std::sync::Arc::new(mc_data::testing::stub()),
        std::sync::Arc::new(
            mc_world::BlockRegistry::from_report(&[]).expect("empty registry builds"),
        ),
        None,
        std::sync::Arc::new(mc_data::tags::TagsData::default()),
        std::sync::Arc::new(Vec::new()),
        std::sync::Arc::new(mc_data::loot::LootTables::default()),
        None,
        std::sync::Arc::new(mc_data::items::ItemRegistry::default()),
        std::sync::Arc::new(mc_data::item_components::ItemFactsTable::default()),
        std::sync::Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        std::sync::Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
    )
    .expect("network config")
}

fn network_config_from_toml(toml_src: &str) -> mc_net::ServerConfig {
    let cfg: mc_server::ServerConfig = toml::from_str(toml_src).expect("parse config");
    network_config_from_server_config(cfg)
}

fn network_config_from_path(path: &Path) -> mc_net::ServerConfig {
    let raw = std::fs::read_to_string(path).expect("read server config");
    let mut cfg: mc_server::ServerConfig = toml::from_str(&raw).expect("parse server config");
    cfg.load_access_control_files(path)
        .expect("load file-backed access control");
    network_config_from_server_config(cfg)
}

async fn start_server_from_toml(toml_src: &str) -> SocketAddr {
    let bound = mc_net::bind(network_config_from_toml(toml_src))
        .await
        .expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });
    addr
}

async fn start_server_from_path(path: &Path) -> SocketAddr {
    let bound = mc_net::bind(network_config_from_path(path))
        .await
        .expect("bind file-backed access server");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });
    addr
}

async fn send_login_start(addr: SocketAddr, name: &str) -> (TcpStream, BytesMut) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let rbuf = BytesMut::with_capacity(4096);
    write_frame(
        &mut stream,
        &Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: addr.port(),
            next_state: NextState::Login,
        },
        Compression::Disabled,
    )
    .await;
    write_frame(
        &mut stream,
        &LoginStart {
            name: name.into(),
            player_uuid: Uuid::nil(),
        },
        Compression::Disabled,
    )
    .await;
    (stream, rbuf)
}

async fn drive_to_set_compression(addr: SocketAddr, name: &str) -> SetCompression {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(4096);
    write_frame(
        &mut stream,
        &Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: addr.port(),
            next_state: NextState::Login,
        },
        Compression::Disabled,
    )
    .await;
    write_frame(
        &mut stream,
        &LoginStart {
            name: name.into(),
            player_uuid: Uuid::nil(),
        },
        Compression::Disabled,
    )
    .await;

    let mut frame = read_one_frame(&mut stream, &mut rbuf, Compression::Disabled).await;
    assert_eq!(frame.id, SetCompression::ID);
    SetCompression::decode(&mut frame.body).unwrap()
}

async fn write_frame<P: Packet>(stream: &mut TcpStream, packet: &P, compression: Compression) {
    let mut body = BytesMut::new();
    packet.encode(&mut body).unwrap();
    let framed = encode_frame(P::ID, &body, compression).unwrap();
    stream.write_all(&framed).await.unwrap();
}

async fn read_one_frame(
    stream: &mut TcpStream,
    buf: &mut BytesMut,
    compression: Compression,
) -> mc_protocol::RawFrame {
    loop {
        if let Some(frame) = try_decode_frame(buf, compression).unwrap() {
            return frame;
        }
        let read = stream.read_buf(buf).await.unwrap();
        assert!(read > 0, "server closed before sending a complete frame");
    }
}

async fn read_one_encrypted_frame(
    stream: &mut TcpStream,
    buf: &mut BytesMut,
    cipher: &mut mc_net::encryption::MinecraftCipher,
    compression: Compression,
) -> mc_protocol::RawFrame {
    loop {
        if let Some(frame) = try_decode_frame(buf, compression).unwrap() {
            return frame;
        }

        let mut encrypted = [0_u8; 4096];
        let read = stream.read(&mut encrypted).await.unwrap();
        assert!(
            read > 0,
            "server closed before sending a complete encrypted frame"
        );
        cipher.decrypt_in_place(&mut encrypted[..read]);
        buf.extend_from_slice(&encrypted[..read]);
    }
}

async fn write_encrypted_frame<P: Packet>(
    stream: &mut TcpStream,
    packet: &P,
    compression: Compression,
    cipher: &mut mc_net::encryption::MinecraftCipher,
) {
    let mut body = BytesMut::new();
    packet.encode(&mut body).unwrap();
    let mut framed = encode_frame(P::ID, &body, compression).unwrap().to_vec();
    cipher.encrypt_in_place(&mut framed);
    stream.write_all(&framed).await.unwrap();
}

#[derive(Debug)]
struct RecordingSessionVerifier {
    request: Mutex<Option<mc_net::VerifySession>>,
    response: mc_net::VerifiedSession,
}

impl mc_net::SessionVerifier for RecordingSessionVerifier {
    fn verify(&self, request: mc_net::VerifySession) -> mc_net::SessionVerifierFuture<'_> {
        Box::pin(async move {
            *self.request.lock().unwrap() = Some(request);
            Ok(self.response.clone())
        })
    }
}

async fn read_optional_frame(
    stream: &mut TcpStream,
    buf: &mut BytesMut,
    compression: Compression,
) -> Option<mc_protocol::RawFrame> {
    loop {
        if let Some(frame) = try_decode_frame(buf, compression).unwrap() {
            return Some(frame);
        }
        let read = tokio::time::timeout(Duration::from_secs(2), stream.read_buf(buf))
            .await
            .expect("server neither closed nor sent a complete frame")
            .unwrap();
        if read == 0 {
            return None;
        }
    }
}

async fn read_complete_raw_frame(stream: &mut TcpStream, buf: &mut BytesMut) -> BytesMut {
    loop {
        if let Some((frame_len, prefix_len)) = read_varint_partial(buf.as_ref()).unwrap() {
            let frame_len = usize::try_from(frame_len).expect("frame length is non-negative");
            let total = prefix_len + frame_len;
            if buf.len() >= total {
                return buf.split_to(total);
            }
        }
        let read = stream.read_buf(buf).await.unwrap();
        assert!(
            read > 0,
            "server closed before sending a complete raw frame"
        );
    }
}

#[tokio::test]
async fn login_offline_flow_completes() {
    let addr = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(4096);

    // Handshake with next_state = Login.
    write_frame(
        &mut stream,
        &Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: addr.port(),
            next_state: NextState::Login,
        },
        Compression::Disabled,
    )
    .await;

    // Client always sends some UUID alongside its name. We send a
    // throwaway one: the server should ignore it and stamp its own
    // offline UUID derived from the name.
    write_frame(
        &mut stream,
        &LoginStart {
            name: "Notch".into(),
            player_uuid: Uuid::from_u128(0xDEADBEEF),
        },
        Compression::Disabled,
    )
    .await;

    let mut frame = read_one_frame(&mut stream, &mut rbuf, Compression::Disabled).await;
    assert_eq!(
        frame.id,
        SetCompression::ID,
        "should negotiate compression first"
    );
    let set_compression = SetCompression::decode(&mut frame.body).unwrap();
    assert_eq!(set_compression.threshold, 256);
    let compression = Compression::Threshold(set_compression.threshold as usize);

    // Expect LoginSuccess after the compression boundary.
    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(frame.id, LoginSuccess::ID, "should be login success");
    let success = LoginSuccess::decode(&mut frame.body).unwrap();
    assert_eq!(frame.body.remaining(), 0);
    assert_eq!(success.name, "Notch");
    assert_eq!(success.uuid, mc_net::offline_uuid("Notch"));
    assert!(success.properties.is_empty());

    // Acknowledge — this transitions to Configuration state.
    write_frame(&mut stream, &LoginAcknowledged, compression).await;
    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(frame.id, UpdateEnabledFeatures::ID);
    let _ = UpdateEnabledFeatures::decode(&mut frame.body).unwrap();
    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(
        frame.id,
        ClientboundKnownPacks::ID,
        "server should advertise Known Packs once Configuration begins"
    );
    let _ = ClientboundKnownPacks::decode(&mut frame.body).unwrap();
}

#[tokio::test]
async fn login_compresses_post_set_compression_frames_at_configured_threshold() {
    let addr = start_server_with_policy(mc_net::ChunkPipelinePolicy {
        compression_threshold: 1,
        ..mc_net::ChunkPipelinePolicy::default()
    })
    .await;
    let (mut stream, mut rbuf) = send_login_start(addr, "CompressedLogin").await;

    let mut frame = read_one_frame(&mut stream, &mut rbuf, Compression::Disabled).await;
    assert_eq!(frame.id, SetCompression::ID);
    let set_compression = SetCompression::decode(&mut frame.body).unwrap();
    assert_eq!(set_compression.threshold, 1);

    let raw = read_complete_raw_frame(&mut stream, &mut rbuf).await;
    let (frame_len, prefix_len) = read_varint_partial(raw.as_ref())
        .unwrap()
        .expect("raw frame has length prefix");
    let compressed_body = &raw[prefix_len..prefix_len + frame_len as usize];
    let (data_length, _) = read_varint_partial(compressed_body)
        .unwrap()
        .expect("compressed frame has data_length prefix");
    assert!(
        data_length > 0,
        "LoginSuccess after SetCompression must use compressed framing at threshold 1",
    );

    let mut decode_buf = raw.clone();
    let mut frame = try_decode_frame(&mut decode_buf, Compression::Threshold(1))
        .unwrap()
        .expect("compressed LoginSuccess decodes");
    assert_eq!(frame.id, LoginSuccess::ID);
    let success = LoginSuccess::decode(&mut frame.body).unwrap();
    assert_eq!(success.name, "CompressedLogin");
    assert_eq!(success.uuid, mc_net::offline_uuid("CompressedLogin"));
}

#[tokio::test]
async fn login_rejects_malformed_compressed_ack_frame() {
    let addr = start_server().await;
    let (mut stream, mut rbuf) = send_login_start(addr, "BadZipAck").await;

    let mut frame = read_one_frame(&mut stream, &mut rbuf, Compression::Disabled).await;
    assert_eq!(frame.id, SetCompression::ID);
    let set_compression = SetCompression::decode(&mut frame.body).unwrap();
    let compression = Compression::Threshold(set_compression.threshold as usize);

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(frame.id, LoginSuccess::ID);
    let success = LoginSuccess::decode(&mut frame.body).unwrap();
    assert_eq!(success.name, "BadZipAck");

    // packet_length = 3, data_length = 256, followed by a single non-zlib byte.
    // This reaches the compressed-frame decoder instead of the below-threshold guard.
    stream.write_all(&[0x03, 0x80, 0x02, 0x00]).await.unwrap();

    assert!(
        read_optional_frame(&mut stream, &mut rbuf, compression)
            .await
            .is_none(),
        "malformed compressed client frame must close before Configuration"
    );
}

#[tokio::test]
async fn login_uses_configured_compression_threshold() {
    let addr = start_server_with_policy(mc_net::ChunkPipelinePolicy {
        compression_threshold: 128,
        ..mc_net::ChunkPipelinePolicy::default()
    })
    .await;

    let set_compression = drive_to_set_compression(addr, "ThresholdProbe").await;

    assert_eq!(set_compression.threshold, 128);
}

#[tokio::test]
async fn login_rejects_invalid_username_syntax_before_compression() {
    let addr = start_server().await;
    for name in ["", "ab", "abc\n", "abc def", "abc-def", "éclair", "玩家123"] {
        let (mut stream, mut rbuf) = send_login_start(addr, name).await;
        let mut frame = read_one_frame(&mut stream, &mut rbuf, Compression::Disabled).await;
        assert_eq!(frame.id, LoginDisconnect::ID, "name {name:?}");
        let disconnect = LoginDisconnect::decode(&mut frame.body).unwrap();
        assert!(disconnect.reason_json.contains("Invalid username"));
        assert_eq!(frame.body.remaining(), 0);
    }
}

#[tokio::test]
async fn login_whitelist_rejects_before_compression() {
    let permissions = mc_net::CommandPermissionConfig::new(Vec::<String>::new(), false)
        .with_login_access(mc_net::LoginAccessConfig::normalized(
            false,
            true,
            ["Allowed"],
            std::iter::empty::<&str>(),
        ));
    let addr = start_server_with_policy_and_permissions(
        mc_net::ChunkPipelinePolicy::default(),
        permissions,
    )
    .await;
    let (mut stream, mut rbuf) = send_login_start(addr, "Blocked").await;

    let mut frame = read_one_frame(&mut stream, &mut rbuf, Compression::Disabled).await;
    assert_eq!(frame.id, LoginDisconnect::ID);
    let disconnect = LoginDisconnect::decode(&mut frame.body).unwrap();
    assert!(disconnect.reason_json.contains("not whitelisted"));
}

#[tokio::test]
async fn file_backed_whitelist_and_ban_reload_across_server_instances() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join("server.toml");
    let whitelist_path = temp.path().join("whitelist.json");
    let banned_path = temp.path().join("banned-players.json");
    std::fs::write(
        &config_path,
        r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 0

            [auth]
            whitelist_enabled = true
            whitelist_file = "whitelist.json"
            banned_players_file = "banned-players.json"
        "#,
    )
    .unwrap();
    std::fs::write(&whitelist_path, br#"[{"name":"Allowed"}]"#).unwrap();
    std::fs::write(&banned_path, b"[]").unwrap();

    let first_addr = start_server_from_path(&config_path).await;
    let compression = drive_to_set_compression(first_addr, "Allowed").await;
    assert_eq!(compression.threshold, 256);
    let (mut blocked, mut blocked_buf) = send_login_start(first_addr, "Blocked").await;
    let mut frame = read_one_frame(&mut blocked, &mut blocked_buf, Compression::Disabled).await;
    assert_eq!(frame.id, LoginDisconnect::ID);
    let disconnect = LoginDisconnect::decode(&mut frame.body).unwrap();
    assert!(disconnect.reason_json.contains("not whitelisted"));

    std::fs::write(
        &whitelist_path,
        br#"[{"name":"Allowed"},{"name":"SecondUser"}]"#,
    )
    .unwrap();
    std::fs::write(
        &banned_path,
        br#"[{"name":"Allowed","reason":"restart-ban"}]"#,
    )
    .unwrap();

    let restarted_addr = start_server_from_path(&config_path).await;
    let (mut banned, mut banned_buf) = send_login_start(restarted_addr, "Allowed").await;
    let mut frame = read_one_frame(&mut banned, &mut banned_buf, Compression::Disabled).await;
    assert_eq!(frame.id, LoginDisconnect::ID);
    let disconnect = LoginDisconnect::decode(&mut frame.body).unwrap();
    assert!(disconnect.reason_json.contains("banned"));

    let compression = drive_to_set_compression(restarted_addr, "SecondUser").await;
    assert_eq!(compression.threshold, 256);
}

#[tokio::test]
async fn login_toml_online_mode_starts_encryption_before_compression() {
    let addr = start_server_from_toml(
        r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 0

            [auth]
            online_mode = true
        "#,
    )
    .await;
    let (mut stream, mut rbuf) = send_login_start(addr, "Notch").await;

    let mut frame = read_one_frame(&mut stream, &mut rbuf, Compression::Disabled).await;
    assert_eq!(frame.id, EncryptionRequest::ID);
    let request = EncryptionRequest::decode(&mut frame.body).unwrap();
    assert_eq!(request.server_id, "");
    assert!(!request.public_key.is_empty());
    assert_eq!(request.verify_token.len(), 4);
    assert!(request.should_authenticate);
}

#[tokio::test]
async fn online_mode_completes_encrypted_login_with_fake_session_verifier() {
    const SHARED_SECRET: [u8; 16] = *b"0123456789abcdef";
    let verified_uuid = Uuid::parse_str("12345678-1234-5678-9abc-def012345678").unwrap();
    let properties = vec![GameProfileProperty {
        name: "textures".to_owned(),
        value: "signed-texture-value".to_owned(),
        signature: Some("texture-signature".to_owned()),
    }];
    let verifier = Arc::new(RecordingSessionVerifier {
        request: Mutex::new(None),
        response: mc_net::VerifiedSession {
            uuid: verified_uuid,
            name: "OnlinePlayer".to_owned(),
            properties: properties.clone(),
        },
    });
    let login_access = mc_net::LoginAccessConfig::normalized(
        true,
        false,
        std::iter::empty::<&str>(),
        std::iter::empty::<&str>(),
    )
    .with_session_verifier(verifier.clone());
    let addr = start_server_with_login_access(login_access).await;
    let (mut stream, mut rbuf) = send_login_start(addr, "onlineplayer").await;

    let mut frame = read_one_frame(&mut stream, &mut rbuf, Compression::Disabled).await;
    assert_eq!(frame.id, EncryptionRequest::ID);
    let request = EncryptionRequest::decode(&mut frame.body).unwrap();
    assert_eq!(frame.body.remaining(), 0);
    assert_eq!(request.server_id, "");
    assert_eq!(request.verify_token.len(), 4);
    assert!(request.should_authenticate);

    let public_key = RsaPublicKey::from_public_key_der(&request.public_key).unwrap();
    let mut rng = rsa::rand_core::OsRng;
    let encrypted_shared_secret = public_key
        .encrypt(&mut rng, Pkcs1v15Encrypt, &SHARED_SECRET)
        .unwrap();
    let encrypted_verify_token = public_key
        .encrypt(&mut rng, Pkcs1v15Encrypt, &request.verify_token)
        .unwrap();
    write_frame(
        &mut stream,
        &EncryptionResponse {
            encrypted_shared_secret,
            encrypted_verify_token,
        },
        Compression::Disabled,
    )
    .await;

    let mut clientbound_cipher = mc_net::encryption::MinecraftCipher::new(&SHARED_SECRET);
    let mut serverbound_cipher = mc_net::encryption::MinecraftCipher::new(&SHARED_SECRET);

    let mut frame = read_one_encrypted_frame(
        &mut stream,
        &mut rbuf,
        &mut clientbound_cipher,
        Compression::Disabled,
    )
    .await;
    assert_eq!(frame.id, SetCompression::ID);
    let set_compression = SetCompression::decode(&mut frame.body).unwrap();
    assert_eq!(frame.body.remaining(), 0);
    let compression = Compression::Threshold(set_compression.threshold as usize);

    let mut frame =
        read_one_encrypted_frame(&mut stream, &mut rbuf, &mut clientbound_cipher, compression)
            .await;
    assert_eq!(frame.id, LoginSuccess::ID);
    let success = LoginSuccess::decode(&mut frame.body).unwrap();
    assert_eq!(frame.body.remaining(), 0);
    assert_eq!(success.name, "OnlinePlayer");
    assert_eq!(success.uuid, verified_uuid);
    assert_eq!(success.properties, properties);

    write_encrypted_frame(
        &mut stream,
        &LoginAcknowledged,
        compression,
        &mut serverbound_cipher,
    )
    .await;
    let mut frame =
        read_one_encrypted_frame(&mut stream, &mut rbuf, &mut clientbound_cipher, compression)
            .await;
    assert_eq!(frame.id, UpdateEnabledFeatures::ID);
    UpdateEnabledFeatures::decode(&mut frame.body).unwrap();
    assert_eq!(frame.body.remaining(), 0);
    let mut frame =
        read_one_encrypted_frame(&mut stream, &mut rbuf, &mut clientbound_cipher, compression)
            .await;
    assert_eq!(frame.id, ClientboundKnownPacks::ID);
    ClientboundKnownPacks::decode(&mut frame.body).unwrap();
    assert_eq!(frame.body.remaining(), 0);

    let verification = verifier
        .request
        .lock()
        .unwrap()
        .clone()
        .expect("login success proves that the verifier was called");
    assert_eq!(verification.username, "onlineplayer");
    assert_eq!(
        verification.server_id_hash,
        mc_net::minecraft_server_hash(b"", &SHARED_SECRET, &request.public_key)
    );
    assert_eq!(verification.client_ip, None);
}

#[tokio::test]
async fn login_toml_ban_rejects_before_compression() {
    let addr = start_server_from_toml(
        r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 0

            [auth]
            online_mode = false
            banned_players = ["Notch"]
        "#,
    )
    .await;
    let (mut stream, mut rbuf) = send_login_start(addr, "Notch").await;

    let mut frame = read_one_frame(&mut stream, &mut rbuf, Compression::Disabled).await;
    assert_eq!(frame.id, LoginDisconnect::ID);
    let disconnect = LoginDisconnect::decode(&mut frame.body).unwrap();
    assert!(disconnect.reason_json.contains("banned"));
}

#[tokio::test]
async fn login_toml_whitelist_allows_normalized_offline_profile() {
    let addr = start_server_from_toml(
        r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 0

            [auth]
            online_mode = false
            whitelist_enabled = true
            whitelist = [" notch "]
        "#,
    )
    .await;
    let (mut stream, mut rbuf) = send_login_start(addr, "Notch").await;

    let mut frame = read_one_frame(&mut stream, &mut rbuf, Compression::Disabled).await;
    assert_eq!(frame.id, SetCompression::ID);
    let set_compression = SetCompression::decode(&mut frame.body).unwrap();
    let compression = Compression::Threshold(set_compression.threshold as usize);

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(frame.id, LoginSuccess::ID);
    let success = LoginSuccess::decode(&mut frame.body).unwrap();
    assert_eq!(success.name, "Notch");
    assert_eq!(success.uuid, mc_net::offline_uuid("Notch"));

    write_frame(&mut stream, &LoginAcknowledged, compression).await;
    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(frame.id, UpdateEnabledFeatures::ID);
    UpdateEnabledFeatures::decode(&mut frame.body).unwrap();
    let frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(frame.id, ClientboundKnownPacks::ID);
}

#[tokio::test]
async fn login_does_not_break_concurrent_status() {
    let addr = start_server().await;

    // Start a slow login: just send the handshake, don't follow up. The
    // server is sitting in read_packet waiting for LoginStart.
    let mut slow = TcpStream::connect(addr).await.unwrap();
    write_frame(
        &mut slow,
        &Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: addr.port(),
            next_state: NextState::Login,
        },
        Compression::Disabled,
    )
    .await;

    // A second client should still be able to ping concurrently.
    let mut pinger = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(4096);
    write_frame(
        &mut pinger,
        &Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: addr.port(),
            next_state: NextState::Status,
        },
        Compression::Disabled,
    )
    .await;
    write_frame(&mut pinger, &StatusRequest, Compression::Disabled).await;
    let mut frame = read_one_frame(&mut pinger, &mut rbuf, Compression::Disabled).await;
    assert_eq!(frame.id, StatusResponse::ID);
    let _ = StatusResponse::decode(&mut frame.body).unwrap();

    // Tidy: close the slow connection.
    drop(slow);
}
