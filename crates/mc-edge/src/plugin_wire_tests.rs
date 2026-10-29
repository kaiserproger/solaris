//! Explicit sibling gate, not a graphical-client or siege/war receipt.
//! SOLARIS_DEFAULT_PLUGINS_ROOT must name the unchanged packages-matched-sdk deployment.
//! Run: cargo test -p mc-edge --lib plugin_wire_tests::real_six_town_claim_protects_tcp_builds -- --exact --ignored --nocapture
//!
//! Wire evidence is local mc-protocol: login/configuration codecs; play
//! interaction.rs (dig, use-item-on, teleport confirmation), login_flow.rs
//! (absolute position), chunks.rs (BlockUpdate/BlockChangedAck), inventory.rs,
//! and chat_commands.rs. Loader artifact framing is mc-loader::encode_artifact_chunk.
//! No packet IDs/layouts are copied from memory or the harness.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bytes::{Buf, Bytes};
use mc_core::Core;
use mc_core::boundary::{BlockPos, TickOutput};
use mc_protocol::frame::Compression;
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundCustomPayload, ClientboundKnownPacks,
    ConfigurationDisconnect, FinishConfiguration, ServerboundCustomPayload, ServerboundKnownPacks,
};
use mc_protocol::packets::handshake::{Handshake, NextState};
use mc_protocol::packets::login::{
    LoginAcknowledged, LoginDisconnect, LoginStart, LoginSuccess, SetCompression,
};
use mc_protocol::packets::play::{
    BlockChangedAck, BlockUpdate, ClientboundCommandSuggestions, ClientboundContainerSetContent,
    ClientboundContainerSetSlot, ClientboundKeepAlive, ClientboundOpenScreen,
    ClientboundSystemChat, ConfirmTeleportation, Direction, InteractionHand, ItemStack,
    PlayDisconnect, PlayerActionKind, ServerboundChat, ServerboundChatCommand,
    ServerboundCommandSuggestion, ServerboundContainerClose, ServerboundKeepAlive,
    ServerboundPlayerAction, ServerboundSetCarriedItem, ServerboundUseItemOn,
    SynchronizePlayerPosition, pack_block_pos,
};
use mc_protocol::packets::{CustomPayload, Packet};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, mpsc};

use crate::config::EdgeConfig;
use crate::connection::{Connection, PlayWriter};
use crate::server::{EdgeServer, core_config, load_tags, load_vanilla_data, serve};

const DEADLINE: Duration = Duration::from_secs(90);
const LEADER: &str = "ClaimWireLeader";
const STRANGER: &str = "ClaimWireGuest";
const CLEANUP: &str = "ClaimWireCleanup";
const PACKAGES: [&str; 6] = [
    "basic-economy",
    "solaris-audit",
    "solaris-economy",
    "solaris-essentials",
    "solaris-permissions",
    "solaris-towns",
];

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("solaris-plugin-wire-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("create isolated wire gate directory");
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// Panic cleanup also stops the native owners. The dedicated runtime below owns
// all socket/pump tasks, so even a failed assertion cannot leave a live server.
struct RunningCore(Arc<Core>);

impl Drop for RunningCore {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

fn exact<P: Packet>(mut body: Bytes) -> P {
    let packet = P::decode(&mut body).expect("decode local typed packet");
    assert!(body.is_empty(), "trailing bytes in packet {:#x}", P::ID);
    packet
}

#[derive(Debug)]
enum WireEvent {
    Position(SynchronizePlayerPosition),
    Contents(ClientboundContainerSetContent),
    Slot(ClientboundContainerSetSlot),
    Chat(String),
    Suggestions(ClientboundCommandSuggestions),
    Block(BlockUpdate),
    Ack(BlockChangedAck),
    Disconnect(String),
}

struct WireClient {
    writer: Arc<Mutex<PlayWriter>>,
    events: mpsc::Receiver<anyhow::Result<WireEvent>>,
    pending: VecDeque<WireEvent>,
    inventory: Vec<ItemStack>,
    carried: ItemStack,
    reader_task: tokio::task::JoinHandle<()>,
    // These are actually downloaded, checked and cached before the wire ACK.
    artifacts: BTreeMap<String, Vec<u8>>,
}

impl Drop for WireClient {
    fn drop(&mut self) {
        self.reader_task.abort();
    }
}

impl WireClient {
    async fn connect(
        address: SocketAddr,
        name: &str,
        expected: &mc_loader::LoaderManifest,
    ) -> Self {
        tokio::time::timeout(DEADLINE, Self::connect_inner(address, name, expected))
            .await
            .expect("bounded TCP/login/configuration readiness")
    }

    async fn connect_inner(
        address: SocketAddr,
        name: &str,
        expected: &mc_loader::LoaderManifest,
    ) -> Self {
        let mut connection =
            Connection::new(TcpStream::connect(address).await.expect("real TCP connect"));
        connection
            .send_packet(&Handshake {
                protocol_version: mc_protocol::PROTOCOL_VERSION,
                server_address: address.ip().to_string(),
                server_port: address.port(),
                next_state: NextState::Login,
            })
            .await
            .unwrap();
        connection
            .send_packet(&LoginStart {
                name: name.to_owned(),
                player_uuid: crate::crypto::offline_uuid(name),
            })
            .await
            .unwrap();
        let compression = connection
            .read_packet::<SetCompression>(mc_protocol::State::Login)
            .await
            .expect("production compression negotiation");
        assert!(compression.threshold >= 0);
        connection.set_compression(Compression::Threshold(
            usize::try_from(compression.threshold).unwrap(),
        ));
        let profile = connection
            .read_packet::<LoginSuccess>(mc_protocol::State::Login)
            .await
            .expect("offline profile admitted");
        assert_eq!(profile.uuid, crate::crypto::offline_uuid(name));
        connection.send_packet(&LoginAcknowledged).await.unwrap();

        let mut artifacts = BTreeMap::new();
        let mut acknowledged = false;
        loop {
            let frame = connection
                .read_frame_in(mc_protocol::State::Configuration)
                .await
                .unwrap();
            if frame.id == ConfigurationDisconnect::ID {
                panic!("configuration refused: {:?}", frame.body);
            }
            if frame.id == ClientboundKnownPacks::ID {
                let advertised = exact::<ClientboundKnownPacks>(frame.body);
                assert!(advertised.packs.iter().any(|pack| {
                    pack.namespace == "minecraft"
                        && pack.id == "core"
                        && pack.version == mc_protocol::TARGET_RELEASE
                }));
                connection
                    .send_packet(&ServerboundKnownPacks {
                        packs: advertised.packs,
                    })
                    .await
                    .unwrap();
            } else if frame.id == ClientboundCustomPayload::ID {
                let payload = exact::<ClientboundCustomPayload>(frame.body).payload;
                if let CustomPayload::Unknown { channel, payload } = payload
                    && channel == *mc_loader::loader_manifest_channel()
                {
                    assert!(!acknowledged, "only one Loader negotiation");
                    let manifest: mc_loader::LoaderManifest =
                        serde_json::from_slice(&payload).unwrap();
                    assert_eq!(manifest.protocol, 3, "the production wire-3 contract");
                    assert_eq!(
                        serde_json::to_value(&manifest).unwrap(),
                        serde_json::to_value(expected).unwrap()
                    );
                    let mut permissions = BTreeSet::new();
                    for bundle in &manifest.bundles {
                        assert!(bundle.loaders.contains(&mc_loader::LoaderPlatform::Native));
                        let request = mc_loader::LoaderArtifactRequest {
                            protocol: manifest.protocol,
                            cache_key: bundle.cache_key.clone(),
                        };
                        connection
                            .send_packet(&ServerboundCustomPayload {
                                payload: CustomPayload::Unknown {
                                    channel: mc_loader::loader_request_channel().clone(),
                                    payload: serde_json::to_vec(&request).unwrap(),
                                },
                            })
                            .await
                            .unwrap();
                        let mut downloaded = Vec::new();
                        loop {
                            let frame = connection
                                .read_frame_in(mc_protocol::State::Configuration)
                                .await
                                .unwrap();
                            assert_eq!(
                                frame.id,
                                ClientboundCustomPayload::ID,
                                "artifact data precedes ConfigurationFinish"
                            );
                            let CustomPayload::Unknown { channel, payload } =
                                exact::<ClientboundCustomPayload>(frame.body).payload
                            else {
                                panic!("requested actual Loader artifact data");
                            };
                            assert_eq!(channel, *mc_loader::loader_artifact_channel());
                            // Exactly the big-endian envelope written by the production
                            // mc_loader::encode_artifact_chunk (not a hand-invented ACK).
                            let mut payload = payload.as_slice();
                            assert!(payload.remaining() >= 4);
                            assert_eq!(payload.get_u16(), manifest.protocol);
                            let key_length = usize::from(payload.get_u16());
                            assert!(payload.remaining() >= key_length + 9);
                            let key = std::str::from_utf8(&payload[..key_length]).unwrap();
                            assert_eq!(key, bundle.cache_key);
                            payload.advance(key_length);
                            assert_eq!(payload.get_u64(), downloaded.len() as u64);
                            let last = payload.get_u8();
                            assert!(last <= 1);
                            assert!(
                                !payload.is_empty()
                                    && payload.len() <= mc_loader::LOADER_ARTIFACT_CHUNK_BYTES
                            );
                            downloaded.extend_from_slice(payload);
                            assert!(downloaded.len() as u64 <= bundle.size_bytes);
                            if last == 1 {
                                break;
                            }
                        }
                        assert_eq!(downloaded.len() as u64, bundle.size_bytes);
                        let verified = expected
                            .bundles
                            .iter()
                            .find(|local| local.cache_key == bundle.cache_key)
                            .unwrap();
                        assert_eq!(bundle.sha256, verified.sha256);
                        // Core startup SHA-256 verifies these immutable bytes. Exact
                        // wire-byte equality binds the download to that SHA without
                        // adding a second hash implementation or dependency here.
                        assert_eq!(
                            downloaded.as_slice(),
                            verified
                                .artifact_bytes
                                .as_deref()
                                .expect("production SHA-verified immutable artifact")
                        );
                        assert!(
                            artifacts
                                .insert(bundle.cache_key.clone(), downloaded)
                                .is_none()
                        );
                        permissions.extend(bundle.permissions.iter().copied());
                    }
                    let ack = mc_loader::LoaderClientAck {
                        protocol: manifest.protocol,
                        platform: mc_loader::LoaderPlatform::Native,
                        loader_version: "solaris-wire-integration".into(),
                        accepted_permissions: permissions.into_iter().collect(),
                        cached_bundles: artifacts.keys().cloned().collect(),
                        carrier_block_state_ids: BTreeMap::new(),
                    };
                    // Native catalog declares no Loader blocks (and no guest
                    // bundles at all); never fabricate a carrier-state mapping.
                    expected
                        .bind_ack(&ack)
                        .expect("exact production Loader identities and permissions");
                    connection
                        .send_packet(&ServerboundCustomPayload {
                            payload: CustomPayload::Unknown {
                                channel: mc_loader::loader_ack_channel().clone(),
                                payload: serde_json::to_vec(&ack).unwrap(),
                            },
                        })
                        .await
                        .unwrap();
                    acknowledged = true;
                }
            } else if frame.id == FinishConfiguration::ID {
                let _ = exact::<FinishConfiguration>(frame.body);
                assert!(
                    acknowledged
                        || expected.bundles.is_empty()
                        // Optional bundles are offered to a Loader client and are
                        // not a condition for a vanilla client reaching Play.
                        || expected.bundles.iter().all(|bundle| !bundle.required),
                    "a required client bundle must be negotiated before finish"
                );
                if acknowledged {
                    // A negotiated deployment downloaded every genuine bundle.
                    assert_eq!(artifacts.len(), expected.bundles.len());
                } else {
                    assert!(artifacts.is_empty(), "no bundle was downloaded");
                }
                connection
                    .send_packet(&AcknowledgeFinishConfiguration)
                    .await
                    .unwrap();
                break;
            }
        }

        let (mut reader, writer) = connection.into_play_halves();
        let writer = Arc::new(Mutex::new(writer));
        let background_writer = Arc::clone(&writer);
        let (sender, events) = mpsc::channel(1024);
        let reader_task = tokio::spawn(async move {
            loop {
                let result = async {
                    let frame = reader.read_frame().await?;
                    let event = match frame.id {
                        SynchronizePlayerPosition::ID => {
                            let position = exact::<SynchronizePlayerPosition>(frame.body);
                            background_writer
                                .lock()
                                .await
                                .send_packet(&ConfirmTeleportation::accepting(&position))
                                .await?;
                            Some(WireEvent::Position(position))
                        }
                        ClientboundKeepAlive::ID => {
                            let keep_alive = exact::<ClientboundKeepAlive>(frame.body);
                            background_writer
                                .lock()
                                .await
                                .send_packet(&ServerboundKeepAlive { id: keep_alive.id })
                                .await?;
                            None
                        }
                        ClientboundOpenScreen::ID => {
                            // The unchanged economy package can open its real
                            // market at spawn. Close it over the ordinary wire;
                            // never disable its manifest, zone or guest handler.
                            let menu = exact::<ClientboundOpenScreen>(frame.body);
                            background_writer
                                .lock()
                                .await
                                .send_packet(&ServerboundContainerClose {
                                    container_id: menu.container_id,
                                })
                                .await?;
                            None
                        }
                        ClientboundContainerSetContent::ID => {
                            Some(WireEvent::Contents(exact(frame.body)))
                        }
                        ClientboundContainerSetSlot::ID => Some(WireEvent::Slot(exact(frame.body))),
                        ClientboundCommandSuggestions::ID => {
                            Some(WireEvent::Suggestions(exact(frame.body)))
                        }
                        ClientboundSystemChat::ID => {
                            let chat = exact::<ClientboundSystemChat>(frame.body);
                            let content = mc_nbt::read_network(&mut chat.content_nbt.as_slice())?;
                            Some(WireEvent::Chat(format!("{content:?}")))
                        }
                        BlockUpdate::ID => Some(WireEvent::Block(exact(frame.body))),
                        BlockChangedAck::ID => Some(WireEvent::Ack(exact(frame.body))),
                        PlayDisconnect::ID => {
                            let reason = exact::<PlayDisconnect>(frame.body).reason_nbt;
                            Some(WireEvent::Disconnect(format!("{reason:?}")))
                        }
                        _ => None,
                    };
                    Ok::<_, anyhow::Error>(event)
                }
                .await;
                match result {
                    Ok(Some(event)) => {
                        let disconnected = matches!(event, WireEvent::Disconnect(_));
                        if sender.send(Ok(event)).await.is_err() || disconnected {
                            return;
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let _ = sender.send(Err(error)).await;
                        return;
                    }
                }
            }
        });
        let mut client = Self {
            writer,
            events,
            reader_task,
            artifacts,
            pending: VecDeque::new(),
            inventory: Vec::new(),
            carried: ItemStack::EMPTY,
        };
        client
            .matching("initial absolute spawn", |event| {
                matches!(event, WireEvent::Position(_))
            })
            .await;
        client
            .matching(
                "initial authoritative inventory",
                |event| matches!(event, WireEvent::Contents(packet) if packet.container_id == 0),
            )
            .await;
        client
    }

    async fn send<P: Packet>(&self, packet: &P) {
        tokio::time::timeout(DEADLINE, async {
            self.writer.lock().await.send_packet(packet).await
        })
        .await
        .expect("bounded wire write")
        .expect("real wire write");
    }

    async fn next(&mut self) -> WireEvent {
        let event = self
            .events
            .recv()
            .await
            .expect("wire reader alive")
            .expect("wire reader succeeded");
        match &event {
            WireEvent::Contents(packet) if packet.container_id == 0 => {
                self.inventory.clone_from(&packet.items);
                self.carried.clone_from(&packet.carried_item);
            }
            WireEvent::Slot(packet) if packet.container_id == 0 => {
                let slot = usize::try_from(packet.slot).unwrap();
                self.inventory[slot].clone_from(&packet.item_stack);
            }
            _ => {}
        }
        event
    }

    /// Drop every event that is already queued or arrives within a short
    /// quiet window (login hints, announcements), so the next assertion sees
    /// the answer to the command it just sent.
    async fn drain(&mut self) {
        self.pending.clear();
        loop {
            match tokio::time::timeout(std::time::Duration::from_millis(250), self.events.recv())
                .await
            {
                Ok(Some(Ok(_))) => {}
                Ok(Some(Err(error))) => panic!("wire reader failed while draining: {error}"),
                Ok(None) => break,
                Err(_) => break,
            }
        }
    }

    async fn matching(
        &mut self,
        description: &str,
        predicate: impl Fn(&WireEvent) -> bool,
    ) -> WireEvent {
        tokio::time::timeout(DEADLINE, async {
            loop {
                if let Some(index) = self.pending.iter().position(&predicate) {
                    return self.pending.remove(index).unwrap();
                }
                let event = self.next().await;
                if predicate(&event) {
                    return event;
                }
                if let WireEvent::Disconnect(reason) = &event {
                    panic!("unexpected wire disconnect waiting for {description}: {reason}");
                }
                self.pending.push_back(event);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("missing received wire evidence: {description}"))
    }

    async fn command(&mut self, command: &str, feedback: &[&str]) {
        self.send(&ServerboundChatCommand {
            command: command.to_owned(),
        })
        .await;
        let received = self
            .matching(
                command,
                |event| matches!(event, WireEvent::Chat(text) if feedback.iter().all(|part| text.contains(part))),
            )
            .await;
        eprintln!("plugin-v2 genuine wire command={command:?} received={received:?}");
    }

    async fn item_count(&mut self, item: u32, expected: i32) {
        tokio::time::timeout(DEADLINE, async {
            while self
                .inventory
                .iter()
                .filter(|stack| stack.item_id == item)
                .map(|stack| stack.count)
                .sum::<i32>()
                != expected
            {
                let event = self.next().await;
                assert!(!matches!(event, WireEvent::Disconnect(_)));
                self.pending.push_back(event);
            }
        })
        .await
        .expect("authoritative wire inventory count");
    }

    async fn select_item(&self, item: u32) {
        // Vanilla window-0 hotbar indices are 36..44, per inventory.rs and
        // mc-core::inventory's native menu projection. Do not debit a fake slot.
        let slot = (36..45)
            .find(|slot| self.inventory[*slot].item_id == item && self.inventory[*slot].count > 0)
            .expect("real supplied stone is in the hotbar");
        self.send(&ServerboundSetCarriedItem {
            slot: i16::try_from(slot - 36).unwrap(),
        })
        .await;
    }

    async fn position(&mut self, expected: [f64; 3]) {
        self.matching("native teleport and client confirmation", |event| {
            matches!(event, WireEvent::Position(position) if position.relative_flags == 0 && [position.x, position.y, position.z] == expected)
        }).await;
    }

    async fn action_receipt(
        &mut self,
        sequence: i32,
        position: BlockPos,
        block: u32,
        repair_inventory: bool,
    ) {
        let packed = pack_block_pos(position.x, position.y, position.z);
        self.matching("ordinary BlockUpdate repair or committed edit", |event| {
            matches!(event, WireEvent::Block(update) if update.position == packed && update.state_id == i32::try_from(block).unwrap())
        }).await;
        self.matching(
            "matching real BlockChangedAck sequence",
            |event| matches!(event, WireEvent::Ack(ack) if ack.sequence == sequence),
        )
        .await;
        if repair_inventory {
            self.matching(
                "ordinary authoritative inventory repair",
                |event| matches!(event, WireEvent::Contents(packet) if packet.container_id == 0),
            )
            .await;
        }
    }
}

fn snapshot_block(output: &TickOutput, position: BlockPos) -> Option<u32> {
    output
        .snapshot
        .chunks
        .iter()
        .find(|chunk| {
            chunk.pos.x == position.x.div_euclid(16) && chunk.pos.z == position.z.div_euclid(16)
        })?
        .chunk
        .get_block(
            position.x.rem_euclid(16) as u8,
            position.y,
            position.z.rem_euclid(16) as u8,
        )
        .map(|state| state.0)
}

async fn world_is(core: &Core, expected: &[(BlockPos, u32)]) {
    // Subscribe AFTER the wire receipt: no buffered pre-action snapshot can
    // accidentally satisfy a refusal assertion.
    let mut ticks = core.subscribe();
    let output = tokio::time::timeout(DEADLINE, ticks.recv())
        .await
        .expect("bounded next native tick")
        .expect("unlagged native tick publication");
    for (position, block) in expected {
        assert_eq!(
            snapshot_block(&output, *position),
            Some(*block),
            "actual native state after received wire evidence at {position:?}"
        );
    }
}

fn dig(position: BlockPos, sequence: i32) -> ServerboundPlayerAction {
    ServerboundPlayerAction {
        action: PlayerActionKind::StartDestroyBlock,
        position: pack_block_pos(position.x, position.y, position.z),
        direction: Direction::Up,
        sequence,
    }
}

fn place(support: BlockPos, sequence: i32) -> ServerboundUseItemOn {
    ServerboundUseItemOn {
        hand: InteractionHand::MainHand,
        position: pack_block_pos(support.x, support.y, support.z),
        direction: Direction::Up,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside: false,
        world_border_hit: false,
        sequence,
    }
}

#[test]
#[ignore = "requires all six unchanged genuine first-party packages, matched SDK and real vanilla data"]
fn real_six_town_claim_protects_tcp_builds() {
    // A separately owned runtime gives even panic cleanup an exact task boundary.
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_stack_size(16 * 1024 * 1024)
        .enable_all()
        .build()
        .unwrap()
        .block_on(genuine_claim_gate());
}

async fn genuine_claim_gate() {
    let directory = TestDirectory::new();
    let deployment = PathBuf::from(std::env::var_os("SOLARIS_DEFAULT_PLUGINS_ROOT").expect(
        "SOLARIS_DEFAULT_PLUGINS_ROOT must point to packages-matched-sdk; no substitute deployment",
    ))
    .canonicalize()
    .unwrap();
    for name in PACKAGES {
        for file in ["plugin.toml", "plugin.wasm"] {
            assert!(
                deployment.join(name).join(file).is_file(),
                "missing genuine six-package {name}/{file}"
            );
        }
        let manifest: toml::Value =
            std::fs::read_to_string(deployment.join(name).join("plugin.toml"))
                .unwrap()
                .parse()
                .unwrap();
        assert_eq!(manifest["id"].as_str(), Some(name));
    }
    let towns: toml::Value = std::fs::read_to_string(deployment.join("solaris-towns/config.toml"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(towns["dimension"].as_str(), Some("minecraft:overworld"));
    assert!(
        towns["minimum_y"].as_integer().unwrap() <= 161
            && towns["maximum_y"].as_integer().unwrap() >= 161
    );

    let mut cfg = EdgeConfig {
        server_dir: directory.0.clone(),
        ..EdgeConfig::default()
    };
    cfg.data.world_dir = directory.0.join("world");
    cfg.data.seed = 81;
    cfg.data.vanilla_data_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    cfg.server.view_distance = 1;
    cfg.server.simulation_distance = 1;
    cfg.plugins.directory = Some(deployment);
    let data = load_vanilla_data(&cfg).unwrap();
    let tags = load_tags(&cfg, &data).unwrap();
    let mut native_config = core_config(&cfg);
    native_config.checkpoint_every_ticks = u64::MAX;
    native_config.broadcast_cap = 512;
    let core = Arc::new(
        tokio::time::timeout(
            DEADLINE,
            tokio::task::spawn_blocking(move || Core::open(native_config)),
        )
        .await
        .expect("bounded genuine component startup")
        .unwrap()
        .expect("all six genuine components initialize"),
    );
    let running = RunningCore(Arc::clone(&core));
    let actual = core
        .plugin_packages()
        .iter()
        .map(|package| package.plugin_id())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual,
        PACKAGES.into_iter().collect::<BTreeSet<_>>(),
        "exact six-package deployment, no settlements package"
    );
    // No guest owns the settlement catalog now; the Loader manifest carries
    // only the six deployed packages' bundles.
    let manifest =
        mc_loader::LoaderManifest::from_script_bundles(core.plugin_client_bundles()).unwrap();
    let server = EdgeServer::new(Arc::clone(&core), data, tags, cfg).unwrap();
    // Scoped native fixture privileges only. Both policy actors below are
    // non-operators. Cleanup's separate account has no build/policy role.
    server
        .admin
        .set_op(crate::crypto::offline_uuid(LEADER), LEADER, true)
        .unwrap();
    server
        .admin
        .set_op(crate::crypto::offline_uuid(CLEANUP), CLEANUP, true)
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap(); // binding, not a sleep, is readiness
    let server_task = tokio::spawn(serve(listener, server));
    let mut leader = WireClient::connect(address, LEADER, &manifest).await;
    let mut stranger = WireClient::connect(address, STRANGER, &manifest).await;
    assert_eq!(
        leader.artifacts, stranger.artifacts,
        "both clients actually downloaded the exact genuine bundles"
    );

    let [spawn_x, _, spawn_z] = core.world_spawn();
    let x = spawn_x.div_euclid(16) * 16 + 8;
    let z = spawn_z.div_euclid(16) * 16 + 8;
    let support = BlockPos { x, y: 160, z };
    let break_target = BlockPos { x, y: 161, z };
    let place_support = BlockPos {
        x: x + 1,
        y: 160,
        z,
    };
    let place_target = BlockPos {
        x: x + 1,
        y: 161,
        z,
    };
    let stone = core.registry().states.stone;
    let air = core.registry().states.air;
    let torch = core
        .registry()
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:torch").unwrap())
        .unwrap()
        .default
        .0;
    let stone_item = core
        .registry()
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap();

    // Native gamemode correctly emits no success for an unchanged mode.
    // Make this an actual transition before demanding its wire feedback.
    leader
        .command(
            "gamemode creative @a",
            &["commands.gamemode.success", "gameMode.creative"],
        )
        .await;
    leader
        .command(
            "gamemode survival @a",
            &["commands.gamemode.success", "gameMode.survival"],
        )
        .await;
    leader.command("clear @a", &["clear."]).await;
    leader
        .command(
            &format!(
                "fill {} 160 {} {} 160 {} minecraft:stone",
                x - 3,
                z - 3,
                x + 3,
                z + 3
            ),
            &["Changed"],
        )
        .await;
    // Air above the pad may already be air; both genuine native feedbacks
    // are accepted, but the world snapshot below must prove the actual state.
    leader
        .send(&ServerboundChatCommand {
            command: format!(
                "fill {} 161 {} {} 164 {} minecraft:air",
                x - 3,
                z - 3,
                x + 3,
                z + 3
            ),
        })
        .await;
    leader.matching("native air fixture feedback", |event| matches!(event, WireEvent::Chat(text) if text.contains("Changed") || text.contains("No blocks were changed"))).await;
    leader
        .command(
            &format!("setblock {x} 161 {z} minecraft:torch"),
            &["Changed 1 block(s)"],
        )
        .await;
    let leader_position = [f64::from(x) + 0.5, 161.0, f64::from(z) + 2.5];
    let stranger_position = [f64::from(x) + 1.5, 161.0, f64::from(z) + 2.5];
    leader
        .command(
            &format!(
                "tp {LEADER} {} {} {}",
                leader_position[0], leader_position[1], leader_position[2]
            ),
            &["commands.teleport.success", LEADER],
        )
        .await;
    leader.position(leader_position).await;
    leader
        .command(
            &format!(
                "tp {STRANGER} {} {} {}",
                stranger_position[0], stranger_position[1], stranger_position[2]
            ),
            &["commands.teleport.success", STRANGER],
        )
        .await;
    stranger.position(stranger_position).await;
    leader
        .command(
            &format!("give {LEADER} minecraft:stone 8"),
            &["commands.give.success", LEADER],
        )
        .await;
    leader
        .command(
            &format!("give {STRANGER} minecraft:stone 8"),
            &["commands.give.success", STRANGER],
        )
        .await;
    leader.item_count(stone_item, 8).await;
    stranger.item_count(stone_item, 8).await;
    leader.select_item(stone_item).await;
    stranger.select_item(stone_item).await;
    leader
        .command(
            &format!("deop {LEADER}"),
            &[&format!("Command deop completed for {LEADER}")],
        )
        .await;
    let town = format!("wire{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    leader
        .command(
            &format!("town create {town}"),
            &[&format!("Town {town} created.")],
        )
        .await;
    leader
        .command("town claim", &[&format!("Chunk claimed for {town}.")])
        .await;
    // This response is emitted by the real towns component after native zone
    // commit. The removed settlements package's before-build hook is gone;
    // claim protection is towns/native-zone only.
    world_is(
        &core,
        &[
            (support, stone),
            (break_target, torch),
            (place_support, stone),
            (place_target, air),
        ],
    )
    .await;
    // Consume a real policy command barrier on each socket before collecting
    // per-sequence prediction repairs; no old setup updates qualify as evidence.
    leader
        .command("town info", &[&format!("{town}: 1 members, 1 claims.")])
        .await;
    stranger.command("town info", &["Town not found."]).await;
    leader.pending.clear();
    stranger.pending.clear();
    let stranger_inventory = stranger.inventory.clone();
    let stranger_carried = stranger.carried.clone();

    stranger.send(&dig(break_target, 101)).await;
    stranger
        .action_receipt(101, break_target, torch, true)
        .await;
    assert_eq!(
        stranger.inventory, stranger_inventory,
        "denied break has no inventory effect"
    );
    assert_eq!(stranger.carried, stranger_carried);
    world_is(&core, &[(break_target, torch), (place_target, air)]).await;
    stranger.pending.clear();
    stranger.send(&place(place_support, 102)).await;
    stranger.action_receipt(102, place_target, air, true).await;
    assert_eq!(
        stranger.inventory, stranger_inventory,
        "denied survival placement cannot debit inventory"
    );
    assert_eq!(stranger.carried, stranger_carried);
    world_is(&core, &[(break_target, torch), (place_target, air)]).await;

    // The zero-hardness torch uses the real StartDestroyBlock instant-break
    // path. Native generic zone admits the actual non-op leader; eligible
    // edits run Core::admit_build -> AuthorizedBuild consumption, not a fake
    // approval shortcut. No guest build hook participates.
    leader.pending.clear();
    leader.send(&dig(break_target, 201)).await;
    leader.action_receipt(201, break_target, air, false).await;
    world_is(&core, &[(break_target, air), (place_target, air)]).await;
    leader.pending.clear();
    leader.send(&place(place_support, 202)).await;
    leader.action_receipt(202, place_target, stone, false).await;
    leader.item_count(stone_item, 7).await;
    world_is(&core, &[(break_target, air), (place_target, stone)]).await;
    stranger.item_count(stone_item, 8).await;

    // Only cleanup has operator authority now, solely for the native stop
    // command. Its Loader session is negotiated honestly too.
    let mut cleanup = WireClient::connect(address, CLEANUP, &manifest).await;
    cleanup
        .send(&ServerboundChatCommand {
            command: "stop".into(),
        })
        .await;
    cleanup
        .matching("native graceful shutdown disconnect", |event| {
            matches!(event, WireEvent::Disconnect(_))
        })
        .await;
    leader
        .matching("leader shutdown disconnect", |event| {
            matches!(event, WireEvent::Disconnect(_))
        })
        .await;
    stranger
        .matching("stranger shutdown disconnect", |event| {
            matches!(event, WireEvent::Disconnect(_))
        })
        .await;
    tokio::time::timeout(DEADLINE, server_task)
        .await
        .expect("bounded graceful edge shutdown")
        .unwrap()
        .expect("normal production stop path");
    // Wire resync is not the only inventory oracle: inspect the real native
    // player records captured by production graceful shutdown.
    let mut identity = [0; 32];
    identity[..16].copy_from_slice(b"SolarisV2Native\0");
    identity[16..24].copy_from_slice(&81_i64.to_be_bytes());
    identity[24..].copy_from_slice(&(mc_worldgen::WORLDGEN_REVISION as u64).to_be_bytes());
    let image = mc_world::native::NativeReadOnlyStore::open_with_cache(
        &directory.0.join("world/native/state.redb"),
        identity,
        16 * 1024 * 1024,
    )
    .unwrap()
    .read_checkpoint()
    .unwrap();
    for (name, expected_stone) in [(STRANGER, 8), (LEADER, 7)] {
        let player = image
            .players
            .iter()
            .find(|player| player.uuid == *crate::crypto::offline_uuid(name).as_bytes())
            .expect("real policy player persisted by native checkpoint");
        let mut payload = player.record.payload.as_slice();
        let (_, tag) = mc_nbt::read_named(&mut payload).unwrap();
        assert!(payload.is_empty());
        let mc_nbt::Tag::Compound(fields) = tag else {
            panic!("native player record is a compound");
        };
        let decoded = mc_data::player_nbt::player_from_fields(
            &core.registry().items,
            mc_data::player_nbt::VanillaPlayer::default(),
            &fields,
        )
        .unwrap();
        assert_eq!(
            decoded.game_mode, 0,
            "both policy actors genuinely used survival mode"
        );
        assert_eq!(
            decoded
                .inventory
                .iter()
                .filter(|(_, stack)| stack.item_id == stone_item)
                .map(|(_, stack)| stack.count)
                .sum::<i32>(),
            expected_stone,
            "native persisted stone custody agrees with real wire admission/refusal for {name}",
        );
    }
    eprintln!(
        "plugin-v2 genuine TCP claim receipt: packages={actual:?} town={town:?} \
         denied_sequences=[101,102] admitted_sequences=[201,202] \
         persisted_survival_stone={STRANGER}:8,{LEADER}:7; \
         claim_policy=solaris-towns/native-zone; settlement_catalog=native; \
         not_a_siege_veto_or_graphical_client_receipt"
    );
    drop(cleanup);
    drop(stranger);
    drop(leader);
    drop(running);
}

#[test]
fn native_settlement_list_from_player_session_reaches_tick() {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_stack_size(16 * 1024 * 1024)
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let directory = TestDirectory::new();
            let mut cfg = EdgeConfig {
                server_dir: directory.0.clone(),
                ..EdgeConfig::default()
            };
            cfg.data.world_dir = directory.0.join("world");
            cfg.data.seed = 42;
            cfg.data.vanilla_data_dir =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
            cfg.server.view_distance = 1;
            cfg.server.simulation_distance = 1;
            let data = load_vanilla_data(&cfg).unwrap();
            let tags = load_tags(&cfg, &data).unwrap();
            let mut native = core_config(&cfg);
            native.checkpoint_every_ticks = u64::MAX;
            let core = Arc::new(
                tokio::task::spawn_blocking(move || Core::open(native))
                    .await
                    .unwrap()
                    .unwrap(),
            );
            let running = RunningCore(Arc::clone(&core));
            assert!(core.plugin_commands().is_empty());
            let manifest =
                mc_loader::LoaderManifest::from_script_bundles(core.plugin_client_bundles())
                    .unwrap();
            let server = EdgeServer::new(Arc::clone(&core), data, tags, cfg).unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server_task = tokio::spawn(serve(listener, server));
            // No operator privileges or console injection: real login/config/play.
            let mut player = WireClient::connect(address, "SettlementPlayer", &manifest).await;
            player.matching("settlement join tip", |event|
                matches!(event, WireEvent::Chat(text) if text.contains("Settlements / Поселения: /settlement guide"))).await;
            player.command("settlement guide", &[
                "1. Stand at your town centre", "2. Place beds",
                "2000-8999", "settlement bind Oak", "settlement info Oak",
                "production Oak status", "commerce Oak status",
                "payroll Oak status", "caravan Oak status",
            ]).await;
            player
                .command("settlement list", &["No settlements."])
                .await;
            player
                .command("settlement create Oak", &["Founded Oak"])
                .await;
            // Fixture setup only: spawn one real body at the safe world spawn.
            // Membership itself is changed exclusively by this non-op TCP player.
            let villager_type = core.registry().entity_types
                .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap()).unwrap();
            let mut ticks = core.subscribe();
            assert!(core.debug_spawn(mc_core::core::SpawnRequest {
                type_id: villager_type, count: 1, spread: true, start: 0, total: 1,
            }));
            tokio::time::timeout(DEADLINE, async {
                loop {
                    let output = ticks.recv().await.unwrap();
                    if output.snapshot.entities.iter().any(|entity| entity.type_id == villager_type) {
                        break;
                    }
                }
            }).await.expect("loaded nearby villager fixture");
            player.command("settlement bind Oak", &["Bound ", " to Oak."]).await;
            player.command("settlement info Oak", &["«Oak» · глава:", "жителей 1", "Oak |", "members=1"]).await;
            player.command("settlement unbind", &["Unbound "]).await;
            player.command("settlement info Oak", &["«Oak» · глава:", "жителей 0", "members=0"]).await;
            player.command("settlement bind Oak", &["Bound ", " to Oak."]).await;
            player
                .command("settlement list", &["Settlements: Oak."])
                .await;
            player
                .send(&ServerboundCommandSuggestion {
                    id: 41,
                    command: "/settlement li".into(),
                })
                .await;
            let response = player
                .matching(
                    "live native settlement tab completion",
                    |event| matches!(event, WireEvent::Suggestions(response) if response.id == 41),
                )
                .await;
            let WireEvent::Suggestions(response) = response else {
                unreachable!()
            };
            assert_eq!(
                response
                    .suggestions
                    .iter()
                    .map(|entry| entry.text.as_str())
                    .collect::<Vec<_>>(),
                ["list"]
            );
            player
                .command("help settlement", &["/settlement create", "2000-8999"])
                .await;
            let mut stranger = WireClient::connect(address, "SettleGuest", &manifest).await;
            stranger
                .command("settlement remove Oak", &["Only the owner or an operator"])
                .await;
            player
                .command("settlement rename Oak Birch", &["Renamed Oak to Birch"])
                .await;
            player
                .command("settlement remove Birch", &["Removed Birch"])
                .await;
            drop(stranger);
            drop(player);
            drop(running);
            server_task.abort();
        });
}

// ---------------------------------------------------------------------------
// E4/E5 moderation gate: vanilla whitelist refusal at login plus a cross-plugin
// permission node refusing another package's command.
//
// Run: SOLARIS_DEFAULT_PLUGINS_ROOT=/path/to/solaris-default-plugins \
//      cargo test -p mc-edge --lib -- --ignored real_moderation_and_permission_gate --nocapture
// ---------------------------------------------------------------------------

/// One Login-state attempt that never reaches Configuration.
async fn login_refusal(address: SocketAddr, name: &str) -> String {
    let mut connection =
        Connection::new(TcpStream::connect(address).await.expect("real TCP connect"));
    connection
        .send_packet(&Handshake {
            protocol_version: mc_protocol::PROTOCOL_VERSION,
            server_address: address.ip().to_string(),
            server_port: address.port(),
            next_state: NextState::Login,
        })
        .await
        .unwrap();
    connection
        .send_packet(&LoginStart {
            name: name.to_owned(),
            player_uuid: crate::crypto::offline_uuid(name),
        })
        .await
        .unwrap();
    // Admission refusals are answered before compression negotiation; an
    // admitted login gets SetCompression first.
    let first = tokio::time::timeout(
        DEADLINE,
        connection.read_frame_in(mc_protocol::State::Login),
    )
    .await
    .expect("bounded login answer")
    .expect("login answer is readable");
    if first.id == LoginDisconnect::ID {
        return exact::<LoginDisconnect>(first.body).reason_json;
    }
    assert_eq!(first.id, SetCompression::ID, "unexpected first login frame");
    let compression = exact::<SetCompression>(first.body);
    connection.set_compression(Compression::Threshold(
        usize::try_from(compression.threshold).unwrap(),
    ));
    tokio::time::timeout(
        DEADLINE,
        connection.read_packet::<LoginDisconnect>(mc_protocol::State::Login),
    )
    .await
    .expect("bounded login refusal")
    .expect("whitelist refusal is a Login-state disconnect")
    .reason_json
}

fn chat(message: &str) -> ServerboundChat {
    ServerboundChat {
        message: message.to_owned(),
        timestamp_millis: 0,
        salt: 0,
        signature: None,
        last_seen_offset: 0,
        last_seen_acknowledged: [0; 3],
        last_seen_checksum: 0,
    }
}

#[test]
#[ignore = "requires the genuine first-party packages (permission provider + gated roots) and real vanilla data"]
fn real_moderation_and_permission_gate() {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_stack_size(16 * 1024 * 1024)
        .enable_all()
        .build()
        .unwrap()
        .block_on(genuine_moderation_gate());
}

async fn genuine_moderation_gate() {
    let directory = TestDirectory::new();
    let deployment = PathBuf::from(std::env::var_os("SOLARIS_DEFAULT_PLUGINS_ROOT").expect(
        "SOLARIS_DEFAULT_PLUGINS_ROOT must point to packages-matched-sdk; no substitute deployment",
    ))
    .canonicalize()
    .unwrap();
    let permissions: toml::Value =
        std::fs::read_to_string(deployment.join("solaris-permissions/plugin.toml"))
            .unwrap()
            .parse()
            .unwrap();
    assert_eq!(permissions["permission_provider"].as_bool(), Some(true));
    let essentials: toml::Value =
        std::fs::read_to_string(deployment.join("solaris-essentials/plugin.toml"))
            .unwrap()
            .parse()
            .unwrap();
    assert_eq!(
        essentials["command_permissions"]["delwarp"].as_str(),
        Some("solaris.essentials.manage")
    );
    assert_eq!(
        essentials["command_permissions"]["home"].as_str(),
        Some("solaris.essentials.use")
    );

    let mut cfg = EdgeConfig {
        server_dir: directory.0.clone(),
        ..EdgeConfig::default()
    };
    cfg.data.world_dir = directory.0.join("world");
    cfg.data.seed = 81;
    cfg.data.vanilla_data_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    cfg.server.view_distance = 1;
    cfg.server.simulation_distance = 1;
    cfg.plugins.directory = Some(deployment);
    let data = load_vanilla_data(&cfg).unwrap();
    let tags = load_tags(&cfg, &data).unwrap();
    let mut native_config = core_config(&cfg);
    native_config.checkpoint_every_ticks = u64::MAX;
    native_config.broadcast_cap = 512;
    // The full shipping pack (nine components) starts slower than the shared
    // wire deadline; this bound still fails closed on a wedged deployment.
    const PACK_STARTUP_DEADLINE: Duration = Duration::from_secs(300);
    let core = Arc::new(
        tokio::time::timeout(
            PACK_STARTUP_DEADLINE,
            tokio::task::spawn_blocking(move || Core::open(native_config)),
        )
        .await
        .expect("bounded genuine component startup")
        .unwrap()
        .expect("the deployment initializes"),
    );
    let _running = RunningCore(Arc::clone(&core));
    let manifest =
        mc_loader::LoaderManifest::from_script_bundles(core.plugin_client_bundles()).unwrap();
    let server = EdgeServer::new(Arc::clone(&core), data, tags, cfg).unwrap();
    server
        .admin
        .set_op(crate::crypto::offline_uuid(LEADER), LEADER, true)
        .unwrap();
    // Vanilla whitelist: both fixture accounts are listed, a third is not.
    for name in [LEADER, STRANGER, "WireJoiner"] {
        server
            .admin
            .whitelist_add(crate::crypto::offline_uuid(name), name)
            .unwrap();
    }
    server.admin.set_whitelist(true).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server_task = tokio::spawn(serve(listener, server));

    // The enforced list refuses an unlisted identity in Login, with vanilla's
    // own client-side wording.
    let refusal = login_refusal(address, "WireUnlisted").await;
    assert!(
        refusal.contains("multiplayer.disconnect.not_whitelisted"),
        "vanilla whitelist refusal, got {refusal}"
    );

    let mut leader = WireClient::connect(address, LEADER, &manifest).await;
    let mut stranger = WireClient::connect(address, STRANGER, &manifest).await;

    // `delwarp` requires `solaris.essentials.manage`; the default group grants
    // `solaris.essentials.use` only, so admission refuses before the guest and
    // before the plugin's own operator check.
    stranger
        .command(
            "delwarp nothere",
            &["You do not have permission to use this plugin command."],
        )
        .await;
    // `home` needs `solaris.essentials.use`, which the default row grants: the
    // refusal above is the node, not plugin commands being disabled.
    stranger.command("home", &["Location not found."]).await;
    // The native operator level does not substitute for a granted node.
    leader
        .command(
            "delwarp nothere",
            &["You do not have permission to use this plugin command."],
        )
        .await;

    // The provider answers through its published table: the operator assigns
    // the admin group (node `*`), the plugin publishes, and admission now admits.
    leader
        .command(
            &format!(
                "perm user {} set admin",
                crate::crypto::offline_uuid(LEADER)
            ),
            &["Group set to admin."],
        )
        .await;
    leader.drain().await;
    leader
        .command("delwarp nothere", &["Location not found."])
        .await;

    // Join/leave are vanilla yellow translatable announcements.
    let joiner = WireClient::connect(address, "WireJoiner", &manifest).await;
    leader
        .matching("join announcement", |event| {
            matches!(event, WireEvent::Chat(text)
                if text.contains("multiplayer.player.joined") && text.contains("WireJoiner"))
        })
        .await;
    drop(joiner);
    leader
        .matching("leave announcement", |event| {
            matches!(event, WireEvent::Chat(text)
                if text.contains("multiplayer.player.left") && text.contains("WireJoiner"))
        })
        .await;

    // Mute refuses ordinary chat only; warn is delivered privately.
    leader
        .command(&format!("mute {STRANGER} 10m spam"), &["expires in 600s"])
        .await;
    stranger.send(&chat("am I audible?")).await;
    stranger
        .matching(
            "mute notice",
            |event| matches!(event, WireEvent::Chat(text) if text.contains("You are muted.")),
        )
        .await;
    leader
        .command(&format!("warn {STRANGER} keep it civil"), &["completed"])
        .await;
    stranger
        .matching("warning", |event| {
            matches!(event, WireEvent::Chat(text)
                if text.contains("[Warned]") && text.contains("keep it civil"))
        })
        .await;
    leader
        .command(&format!("unmute {STRANGER}"), &["completed"])
        .await;

    // A temporary ban writes the vanilla `expires` stamp and refuses at login
    // with the expiration line once the target is offline.
    leader
        .command("ban WireTempBanned 10m grief", &["expires in 600s"])
        .await;
    let refusal = login_refusal(address, "WireTempBanned").await;
    assert!(
        refusal.contains("multiplayer.disconnect.banned.reason")
            && refusal.contains("multiplayer.disconnect.banned.expiration"),
        "temporary ban refusal carries the vanilla expiration, got {refusal}"
    );

    server_task.abort();
}

// ---------------------------------------------------------------------------
// E4/G7/G9 live gate: vanilla whitelist refusal, temporary-ban expiration and
// the per-address accept limits, all asserted on the real wire.
//
// Run: cargo nextest run -p mc-edge --lib -E 'test(real_moderation_limits_gate)' --run-ignored ignored-only
// ---------------------------------------------------------------------------

/// What a Login-state attempt actually observed.
#[derive(Debug, PartialEq, Eq)]
enum LoginOutcome {
    Admitted,
    /// The server answered LoginDisconnect with this JSON component.
    Refused(String),
    /// The listener closed the socket before it answered (an accept refusal).
    Closed,
}

async fn login_outcome(address: SocketAddr, name: &str) -> LoginOutcome {
    let Ok(stream) = TcpStream::connect(address).await else {
        return LoginOutcome::Closed;
    };
    let mut connection = Connection::new(stream);
    // An accept refusal closes the socket before the handshake is read, so a
    // dead write is itself the refusal this helper reports.
    if connection
        .send_packet(&Handshake {
            protocol_version: mc_protocol::PROTOCOL_VERSION,
            server_address: address.ip().to_string(),
            server_port: address.port(),
            next_state: NextState::Login,
        })
        .await
        .is_err()
    {
        return LoginOutcome::Closed;
    }
    if connection
        .send_packet(&LoginStart {
            name: name.to_owned(),
            player_uuid: crate::crypto::offline_uuid(name),
        })
        .await
        .is_err()
    {
        return LoginOutcome::Closed;
    }
    // Admission refusals (ban, whitelist, server-full) are answered before
    // compression negotiation; an admitted login gets SetCompression first.
    let Ok(Ok(first)) = tokio::time::timeout(
        DEADLINE,
        connection.read_frame_in(mc_protocol::State::Login),
    )
    .await
    else {
        return LoginOutcome::Closed;
    };
    if first.id == LoginDisconnect::ID {
        return LoginOutcome::Refused(exact::<LoginDisconnect>(first.body).reason_json);
    }
    assert_eq!(first.id, SetCompression::ID, "unexpected first login frame");
    let compression = exact::<SetCompression>(first.body);
    connection.set_compression(Compression::Threshold(
        usize::try_from(compression.threshold).unwrap(),
    ));
    let Ok(Ok(second)) = tokio::time::timeout(
        DEADLINE,
        connection.read_frame_in(mc_protocol::State::Login),
    )
    .await
    else {
        return LoginOutcome::Closed;
    };
    if second.id == LoginDisconnect::ID {
        return LoginOutcome::Refused(exact::<LoginDisconnect>(second.body).reason_json);
    }
    assert_eq!(second.id, LoginSuccess::ID, "unexpected login frame");
    LoginOutcome::Admitted
}

/// One plugin-free server over its own world directory.
async fn start_minimal_server(
    directory: &TestDirectory,
    world: &str,
    configure: impl FnOnce(&mut EdgeConfig),
    configure_admin: impl FnOnce(&crate::admin::Admin),
) -> (
    Arc<Core>,
    SocketAddr,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let mut cfg = EdgeConfig {
        server_dir: directory.0.clone(),
        ..EdgeConfig::default()
    };
    cfg.data.world_dir = directory.0.join(world);
    cfg.data.seed = 81;
    cfg.data.vanilla_data_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    cfg.server.view_distance = 2;
    cfg.server.simulation_distance = 2;
    cfg.plugins.directory = None;
    configure(&mut cfg);
    let data = load_vanilla_data(&cfg).unwrap();
    let tags = load_tags(&cfg, &data).unwrap();
    let mut native_config = core_config(&cfg);
    native_config.checkpoint_every_ticks = u64::MAX;
    let core = Arc::new(
        tokio::time::timeout(
            DEADLINE,
            tokio::task::spawn_blocking(move || Core::open(native_config)),
        )
        .await
        .expect("bounded core startup")
        .unwrap()
        .expect("core opens"),
    );
    let server = EdgeServer::new(Arc::clone(&core), data, tags, cfg).unwrap();
    // The very Admin its connections use: a second handle would only rewrite the
    // files and leave the live lists stale.
    configure_admin(&server.admin);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(serve(listener, server));
    (core, address, task)
}

#[test]
#[ignore = "real TCP server gate; needs the native content cache"]
fn real_moderation_limits_gate() {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_stack_size(16 * 1024 * 1024)
        .enable_all()
        .build()
        .unwrap()
        .block_on(genuine_moderation_limits());
}

async fn genuine_moderation_limits() {
    let directory = TestDirectory::new();

    // --- whitelist + temporary ban -------------------------------------
    let (core, address, task) = start_minimal_server(
        &directory,
        "world-whitelist",
        |_| {},
        |admin| {
            admin
                .whitelist_add(crate::crypto::offline_uuid("WireAllowed"), "WireAllowed")
                .unwrap();
            admin.set_whitelist(true).unwrap();
            admin
                .ban_player_for(
                    crate::crypto::offline_uuid("WireTempBanned"),
                    "WireTempBanned",
                    "Console",
                    "grief",
                    Some(std::time::Duration::from_secs(600)),
                    true,
                )
                .unwrap();
        },
    )
    .await;
    assert_eq!(
        login_outcome(address, "WireAllowed").await,
        LoginOutcome::Admitted
    );
    match login_outcome(address, "WireDenied").await {
        LoginOutcome::Refused(reason) => assert!(
            reason.contains("multiplayer.disconnect.not_whitelisted"),
            "vanilla whitelist refusal, got {reason}"
        ),
        other => panic!("unlisted player must be refused, got {other:?}"),
    }
    match login_outcome(address, "WireTempBanned").await {
        LoginOutcome::Refused(reason) => assert!(
            reason.contains("multiplayer.disconnect.banned.reason")
                && reason.contains("grief")
                && reason.contains("multiplayer.disconnect.banned.expiration"),
            "temporary ban carries reason and expiration, got {reason}"
        ),
        other => panic!("temporary ban must refuse login, got {other:?}"),
    }
    core.shutdown();
    task.abort();

    // The list is the only reason WireDenied was refused: the persisted toggle is
    // re-read by a fresh server over the same directory and login is admitted.
    let (core, address, task) = start_minimal_server(
        &directory,
        "world-whitelist",
        |_| {},
        |admin| admin.set_whitelist(false).unwrap(),
    )
    .await;
    assert_eq!(
        login_outcome(address, "WireDenied").await,
        LoginOutcome::Admitted
    );
    core.shutdown();
    task.abort();

    // --- connection throttle -------------------------------------------
    let (core, address, task) = start_minimal_server(
        &directory,
        "world-throttle",
        |cfg| cfg.network.connection_throttle_ms = 400,
        |_| {},
    )
    .await;
    assert_eq!(
        login_outcome(address, "WireAllowed").await,
        LoginOutcome::Admitted
    );
    assert_eq!(
        login_outcome(address, "WireAllowed").await,
        LoginOutcome::Closed,
        "a second login inside the throttle window is refused before the handshake"
    );
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(
        login_outcome(address, "WireAllowed").await,
        LoginOutcome::Admitted,
        "the throttle window passes"
    );
    core.shutdown();
    task.abort();

    // --- concurrent connections per address ----------------------------
    let (core, address, task) = start_minimal_server(
        &directory,
        "world-cap",
        |cfg| cfg.network.max_connections_per_ip = 1,
        |_| {},
    )
    .await;
    let held = TcpStream::connect(address).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        login_outcome(address, "WireAllowed").await,
        LoginOutcome::Closed,
        "the second concurrent connection is closed at accept"
    );
    drop(held);
    let mut admitted = false;
    for _ in 0..40 {
        if login_outcome(address, "WireAllowed").await == LoginOutcome::Admitted {
            admitted = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(admitted, "the released slot admits the next login");
    core.shutdown();
    task.abort();
}
