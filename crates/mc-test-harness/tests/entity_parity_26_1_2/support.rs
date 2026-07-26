use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use bytes::Bytes;
use mc_protocol::RawFrame;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, ClientboundChangeDifficulty, ClientboundCommands, ClientboundContainerSetSlot,
    ClientboundInitializeBorder, ClientboundKeepAlive, ClientboundPlayerAbilities,
    ClientboundSetHeldSlot, ClientboundSetTime, ClientboundSystemChat, ConfirmTeleportation,
    EntityEvent, GameEvent, InteractionHand, LevelChunkWithLight, LoginPlay, MovePlayerFlags,
    ServerboundAttack, ServerboundChatCommand, ServerboundInteract, ServerboundKeepAlive,
    ServerboundMovePlayerPosRot, ServerboundPlayerLoaded, ServerboundSetCarriedItem,
    SetCenterChunk, SetDefaultSpawnPosition, SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;
use mc_test_harness::parity::{
    OracleAvailability, ServerKind, read_packet_id_skipping_startup_noise,
    read_typed_skipping_startup_noise, vanilla_oracle_availability,
};
use mc_world::{BlockPos, BlockStateId, WorldStorage};
use tokio::task::JoinHandle;

use super::model::{EntityAliases, EntityFact, ScenarioObservation};
use super::protocol::normalize_tracked_frame;

const FIXTURE_GROUND_Y: i32 = 199;
const FULL_BLOCK_POSITION: BlockPos = BlockPos {
    x: 4,
    y: 202,
    z: -3,
};
const HALF_STEP_POSITION: BlockPos = BlockPos {
    x: 6,
    y: 200,
    z: -3,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OracleGate {
    Available { jar: PathBuf },
    Skipped { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FeedbackExpectation {
    ExactText(String),
    TextPrefix(String),
    TranslationKey(String),
}

enum RootChatComponent<'a> {
    Text(&'a str),
    Translation(&'a str),
    Unsupported,
}

fn command_feedback_matches(mut body: Bytes, expected: &FeedbackExpectation) -> Result<bool> {
    // Local protocol dump index 121 (0x79) and javap establish trusted
    // component network NBT followed by the overlay bool. The translation keys
    // used by callers were read from the local 26.1.2 command implementations;
    // these structural checks are command fences, not oracle evidence.
    let packet = ClientboundSystemChat::decode(&mut body)?;
    ensure!(body.is_empty(), "system chat packet has trailing bytes");
    ensure!(!packet.overlay, "command feedback used overlay system chat");

    let mut content = Bytes::from(packet.content_nbt);
    let component = match mc_nbt::read_network(&mut content) {
        Ok(component) => component,
        Err(_) => return Ok(false),
    };
    ensure!(
        content.is_empty(),
        "system chat component has trailing NBT bytes"
    );

    Ok(match (root_chat_component(&component), expected) {
        (RootChatComponent::Text(actual), FeedbackExpectation::ExactText(expected)) => {
            actual == expected
        }
        (RootChatComponent::Text(actual), FeedbackExpectation::TextPrefix(expected)) => {
            actual.starts_with(expected)
        }
        (RootChatComponent::Translation(actual), FeedbackExpectation::TranslationKey(expected)) => {
            actual == expected
        }
        _ => false,
    })
}

fn root_chat_component(component: &mc_nbt::Tag) -> RootChatComponent<'_> {
    let mc_nbt::Tag::Compound(fields) = component else {
        return RootChatComponent::Unsupported;
    };
    let mut text = None;
    let mut translation = None;
    for (name, value) in fields {
        let target = match name.as_str() {
            "text" => &mut text,
            "translate" => &mut translation,
            _ => continue,
        };
        let mc_nbt::Tag::String(value) = value else {
            return RootChatComponent::Unsupported;
        };
        if target.replace(value.as_str()).is_some() {
            return RootChatComponent::Unsupported;
        }
    }
    match (text, translation) {
        (Some(text), None) => RootChatComponent::Text(text),
        (None, Some(translation)) => RootChatComponent::Translation(translation),
        _ => RootChatComponent::Unsupported,
    }
}

fn decode_evidence_container_set_slot(mut body: Bytes) -> Result<ClientboundContainerSetSlot> {
    let packet = ClientboundContainerSetSlot::decode(&mut body)?;
    ensure!(
        body.is_empty(),
        "container set slot evidence packet has trailing bytes"
    );
    Ok(packet)
}

fn decode_evidence_add_entity(mut body: Bytes) -> Result<AddEntity> {
    let packet = AddEntity::decode(&mut body)?;
    ensure!(
        body.is_empty(),
        "AddEntity evidence packet has trailing bytes"
    );
    Ok(packet)
}

pub(crate) fn probe_oracle(repo_root: &Path) -> OracleGate {
    let availability = vanilla_oracle_availability(repo_root);
    match availability {
        OracleAvailability::Available { jar } => OracleGate::Available { jar },
        unavailable => OracleGate::Skipped {
            reason: unavailable
                .skip_message()
                .expect("unavailable oracle has a skip reason"),
        },
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EntityEndpoint {
    pub(crate) kind: ServerKind,
    pub(crate) addr: SocketAddr,
    pub(crate) collision_fixture: bool,
}

pub(crate) struct SolarisServer {
    endpoint: EntityEndpoint,
    task: JoinHandle<()>,
}

impl SolarisServer {
    pub(crate) async fn spawn() -> Result<Self> {
        let data = Arc::new(mc_data::solaris_required_data());
        let blocks_report = mc_data::blocks::solaris_required_blocks_report();
        let blocks = Arc::new(mc_world::BlockRegistry::from_report(&blocks_report)?);
        let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
        let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&blocks), 128)
            .with_generator(generator);
        let stone = default_block_state(&blocks, "minecraft:stone")?;
        let air = default_block_state(&blocks, "minecraft:air")?;
        seed_flat_fixture(&mut storage, stone, air)?;
        let collision_fixture =
            if let Ok(slab) = default_block_state(&blocks, "minecraft:stone_slab") {
                storage.set_block_at(HALF_STEP_POSITION, slab)?;
                true
            } else {
                false
            };
        storage.set_block_at(FULL_BLOCK_POSITION, stone)?;
        let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
        let items = Arc::new(mc_data::items::solaris_required_items());
        let cfg = mc_net::ServerConfig {
            bind_address: "127.0.0.1:0".parse()?,
            motd: "W07 entity differential harness".into(),
            max_players: 4,
            view_distance: 2,
            data,
            blocks,
            world,
            tags: Arc::new(mc_data::tags::solaris_required_item_tags(&items)),
            recipes: Arc::new(mc_data::recipes::solaris_required_recipes()),
            loot: Arc::new(mc_data::loot::builtin().clone()),
            block_light: Some(Arc::new(
                mc_data::block_light::BlockLightTable::conservative_from_blocks_report(
                    &blocks_report,
                ),
            )),
            items,
            item_facts: Arc::new(mc_data::item_components::solaris_required_item_facts()),
            block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
                &blocks_report,
            )),
            entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            biome_spawns: Arc::new(mc_data::biomes::solaris_required_biome_spawn_rules()),
            chunk_pipeline: mc_net::ChunkPipelinePolicy {
                chunk_send_rate: 8,
                chunk_load_rate: 8,
                chunk_generate_rate: 8,
                chunk_prepare_budget_ms: 5,
                chunk_prepare_batch_size: 8,
                chunk_io_threads: 1,
                chunk_worker_threads: 2,
                chunk_result_queue_size: 64,
                region_cache_size: 4,
                compression_threshold: 256,
                compression_level: None,
                runtime_control: None,
            },
            random_tick: mc_net::RandomTickPolicy::default(),
            command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
            loader_manifest: None,
            shutdown: mc_net::ShutdownHandle::default(),
        };
        let bound = mc_net::bind(cfg).await?;
        let addr = bound.local_addr()?;
        let task = tokio::spawn(async move {
            let _ = bound.serve().await;
        });
        Ok(Self {
            endpoint: EntityEndpoint {
                kind: ServerKind::Solaris,
                addr,
                collision_fixture,
            },
            task,
        })
    }

    pub(crate) fn endpoint(&self) -> EntityEndpoint {
        self.endpoint
    }
}

impl Drop for SolarisServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn seed_flat_fixture(
    storage: &mut WorldStorage,
    stone: BlockStateId,
    air: BlockStateId,
) -> Result<()> {
    for x in -8..=12 {
        for z in -4..=4 {
            storage.set_block_at(
                BlockPos {
                    x,
                    y: FIXTURE_GROUND_Y,
                    z,
                },
                stone,
            )?;
            for y in 200..=203 {
                storage.set_block_at(BlockPos { x, y, z }, air)?;
            }
        }
    }
    Ok(())
}

fn default_block_state(blocks: &mc_world::BlockRegistry, name: &str) -> Result<BlockStateId> {
    let identifier = mc_data::Identifier::parse(name)
        .with_context(|| format!("parse fixture block identifier {name}"))?;
    blocks
        .block(&identifier)
        .map(|block| block.default)
        .with_context(|| format!("fixture block {name} is unavailable"))
}

pub(crate) struct SummonObservation {
    pub(crate) runtime_entity_id: i32,
    pub(crate) intervening_frames: Vec<RawFrame>,
}

pub(crate) struct EntityProtocolHarness {
    endpoint: EntityEndpoint,
    client_name: String,
    client: Client,
    login: LoginPlay,
    anchor: [f64; 3],
    failure_timeout: Duration,
    entity_types: mc_data::entity_types::EntityTypeRegistry,
    items: mc_data::items::ItemRegistry,
}

impl EntityProtocolHarness {
    pub(crate) async fn connect(
        endpoint: EntityEndpoint,
        client_name: &str,
        failure_timeout: Duration,
    ) -> Result<Self> {
        tokio::time::timeout(
            failure_timeout,
            Self::connect_inner(endpoint, client_name, failure_timeout),
        )
        .await
        .with_context(|| {
            format!(
                "timed out after {failure_timeout:?} entering play state on {}",
                server_label(endpoint.kind)
            )
        })?
    }

    async fn connect_inner(
        endpoint: EntityEndpoint,
        client_name: &str,
        failure_timeout: Duration,
    ) -> Result<Self> {
        let mut client = Client::connect(endpoint.addr).await?;
        let _ = client.drive_login(endpoint.addr, client_name).await?;
        client.drive_configuration().await?;

        let login: LoginPlay = read_typed_skipping_startup_noise(&mut client)
            .await
            .context("read play Login")?;
        let _: ClientboundChangeDifficulty = read_typed_skipping_startup_noise(&mut client)
            .await
            .context("read play difficulty")?;
        let _: ClientboundPlayerAbilities = read_typed_skipping_startup_noise(&mut client)
            .await
            .context("read play abilities")?;
        let _: ClientboundSetHeldSlot = read_typed_skipping_startup_noise(&mut client)
            .await
            .context("read selected hotbar slot")?;
        let _: EntityEvent = read_typed_skipping_startup_noise(&mut client)
            .await
            .context("read initial permission event")?;
        read_packet_id_skipping_startup_noise(&mut client, ClientboundCommands::ID)
            .await
            .context("read command tree")?;
        let sync: SynchronizePlayerPosition = read_typed_skipping_startup_noise(&mut client)
            .await
            .context("read initial position synchronization")?;
        let _: ClientboundInitializeBorder = read_typed_skipping_startup_noise(&mut client)
            .await
            .context("read world border")?;
        let _: ClientboundSetTime = read_typed_skipping_startup_noise(&mut client)
            .await
            .context("read initial world time")?;
        let _: SetDefaultSpawnPosition = read_typed_skipping_startup_noise(&mut client)
            .await
            .context("read default spawn position")?;
        let _: GameEvent = read_typed_skipping_startup_noise(&mut client)
            .await
            .context("read waiting-for-chunks event")?;
        let center: SetCenterChunk = read_typed_skipping_startup_noise(&mut client)
            .await
            .context("read initial chunk center")?;
        client
            .write_packet(&ConfirmTeleportation {
                teleport_id: sync.teleport_id,
            })
            .await?;
        client.write_packet(&ServerboundPlayerLoaded).await?;

        let mut harness = Self {
            endpoint,
            client_name: client_name.to_owned(),
            client,
            login,
            anchor: [sync.x, sync.y, sync.z],
            failure_timeout,
            entity_types: mc_data::entity_types::solaris_required_entity_types(),
            items: mc_data::items::solaris_required_items(),
        };
        harness
            .wait_for_chunk((center.chunk_x, center.chunk_z))
            .await?;
        Ok(harness)
    }

    pub(crate) fn kind(&self) -> ServerKind {
        self.endpoint.kind
    }

    pub(crate) fn collision_fixture_available(&self) -> bool {
        self.endpoint.collision_fixture
    }

    pub(crate) fn anchor(&self) -> [f64; 3] {
        self.anchor
    }

    pub(crate) fn aliases(&self) -> Result<EntityAliases> {
        let mut aliases = EntityAliases::new(self.anchor);
        aliases.bind_existing("player", self.login.entity_id)?;
        Ok(aliases)
    }

    pub(crate) fn entity_type_id(&self, name: &str) -> Result<i32> {
        let identifier = mc_data::Identifier::parse(name)
            .with_context(|| format!("parse entity identifier {name}"))?;
        let id = self
            .entity_types
            .id_of(&identifier)
            .with_context(|| format!("entity type {name} is unavailable"))?;
        i32::try_from(id).context("entity type id exceeds i32")
    }

    pub(crate) fn item_id(&self, name: &str) -> Result<u32> {
        let identifier = mc_data::Identifier::parse(name)
            .with_context(|| format!("parse item identifier {name}"))?;
        self.items
            .id_of(&identifier)
            .with_context(|| format!("item {name} is unavailable"))
    }

    pub(crate) async fn give_hotbar_zero(&mut self, item: &str) -> Result<()> {
        let item_id = self.item_id(item)?;
        self.client
            .write_packet(&ServerboundSetCarriedItem { slot: 0 })
            .await?;
        let command = match self.endpoint.kind {
            ServerKind::Solaris => format!("debug give {item} 1 0"),
            ServerKind::Vanilla => format!(
                "item replace entity {} hotbar.0 with {item}",
                self.client_name
            ),
        };
        let expected_feedback = match self.endpoint.kind {
            ServerKind::Solaris => FeedbackExpectation::ExactText("Debug command executed".into()),
            ServerKind::Vanilla => FeedbackExpectation::TranslationKey(
                "commands.item.entity.set.success.single".into(),
            ),
        };
        self.client
            .write_packet(&ServerboundChatCommand { command })
            .await?;

        let deadline = tokio::time::Instant::now() + self.failure_timeout;
        let mut saw_item = false;
        let mut saw_feedback = false;
        while !saw_item || !saw_feedback {
            let frame = self
                .next_non_keepalive(deadline, "hotbar item command")
                .await?;
            if frame.id == ClientboundContainerSetSlot::ID {
                let slot = decode_evidence_container_set_slot(frame.body)?;
                saw_item |= slot.container_id == 0
                    && slot.slot == 36
                    && slot.item_stack.item_id == item_id
                    && slot.item_stack.count == 1;
            } else if frame.id == ClientboundSystemChat::ID {
                saw_feedback |= command_feedback_matches(frame.body, &expected_feedback)?;
            }
        }
        Ok(())
    }

    pub(crate) async fn summon(
        &mut self,
        aliases: &mut EntityAliases,
        observation: &mut ScenarioObservation,
        alias: &str,
        entity_name: &str,
        position: [f64; 3],
    ) -> Result<SummonObservation> {
        self.summon_with_suffix(aliases, observation, alias, entity_name, position, "")
            .await
    }

    pub(crate) async fn summon_vanilla_nbt(
        &mut self,
        aliases: &mut EntityAliases,
        observation: &mut ScenarioObservation,
        alias: &str,
        entity_name: &str,
        position: [f64; 3],
        nbt: &str,
    ) -> Result<SummonObservation> {
        ensure!(
            self.endpoint.kind == ServerKind::Vanilla,
            "vanilla NBT summon used against {}",
            server_label(self.endpoint.kind)
        );
        let suffix = format!(" {nbt}");
        self.summon_with_suffix(aliases, observation, alias, entity_name, position, &suffix)
            .await
    }

    async fn summon_with_suffix(
        &mut self,
        aliases: &mut EntityAliases,
        observation: &mut ScenarioObservation,
        alias: &str,
        entity_name: &str,
        position: [f64; 3],
        command_suffix: &str,
    ) -> Result<SummonObservation> {
        let entity_type_id = self.entity_type_id(entity_name)?;
        let expected_feedback = match self.endpoint.kind {
            ServerKind::Solaris => {
                FeedbackExpectation::ExactText(format!("Summoned {entity_name}"))
            }
            ServerKind::Vanilla => {
                FeedbackExpectation::TranslationKey("commands.summon.success".into())
            }
        };
        self.client
            .write_packet(&ServerboundChatCommand {
                command: format!(
                    "summon {entity_name} {} {} {}{command_suffix}",
                    position[0], position[1], position[2]
                ),
            })
            .await?;

        let deadline = tokio::time::Instant::now() + self.failure_timeout;
        let mut runtime_entity_id = None;
        let mut saw_feedback = false;
        let mut intervening_frames = Vec::new();
        while runtime_entity_id.is_none() || !saw_feedback {
            let frame = self
                .next_non_keepalive(deadline, "summoned entity and command feedback")
                .await?;
            if frame.id == AddEntity::ID {
                let packet = decode_evidence_add_entity(frame.body.clone())?;
                if runtime_entity_id.is_none()
                    && packet.entity_type_id == entity_type_id
                    && position_near([packet.x, packet.y, packet.z], position)
                {
                    observation.push(aliases.bind_spawn(
                        alias,
                        packet.entity_id,
                        entity_name,
                        [packet.x, packet.y, packet.z],
                    )?);
                    runtime_entity_id = Some(packet.entity_id);
                    continue;
                }
            } else if frame.id == ClientboundSystemChat::ID
                && command_feedback_matches(frame.body.clone(), &expected_feedback)?
            {
                saw_feedback = true;
                continue;
            }
            intervening_frames.push(frame);
        }
        Ok(SummonObservation {
            runtime_entity_id: runtime_entity_id.expect("loop exits only after entity spawn"),
            intervening_frames,
        })
    }

    pub(crate) fn normalize_frames(
        &self,
        frames: &[RawFrame],
        aliases: &EntityAliases,
        phase: &str,
    ) -> Result<Vec<EntityFact>> {
        let mut facts = Vec::new();
        for frame in frames {
            facts.extend(normalize_tracked_frame(
                frame.id,
                frame.body.clone(),
                aliases,
                phase,
            )?);
        }
        Ok(facts)
    }

    pub(crate) async fn observe_until(
        &mut self,
        aliases: &EntityAliases,
        phase: &str,
        reason: &str,
        complete: impl FnMut(&[EntityFact]) -> bool,
    ) -> Result<Vec<EntityFact>> {
        self.observe_until_matching(aliases, phase, reason, |_| true, complete)
            .await
    }

    pub(crate) async fn observe_until_matching(
        &mut self,
        aliases: &EntityAliases,
        phase: &str,
        reason: &str,
        mut include_packet: impl FnMut(i32) -> bool,
        mut complete: impl FnMut(&[EntityFact]) -> bool,
    ) -> Result<Vec<EntityFact>> {
        let deadline = tokio::time::Instant::now() + self.failure_timeout;
        let mut facts = Vec::new();
        while !complete(&facts) {
            let frame = self.next_non_keepalive(deadline, reason).await?;
            if !include_packet(frame.id) {
                continue;
            }
            facts.extend(normalize_tracked_frame(
                frame.id, frame.body, aliases, phase,
            )?);
        }
        Ok(facts)
    }

    pub(crate) async fn teleport(&mut self, position: [f64; 3]) -> Result<Vec<RawFrame>> {
        let expected_feedback = match self.endpoint.kind {
            ServerKind::Solaris => FeedbackExpectation::ExactText(format!(
                "Teleported to {} {} {}",
                position[0], position[1], position[2]
            )),
            ServerKind::Vanilla => FeedbackExpectation::TranslationKey(
                "commands.teleport.success.location.single".into(),
            ),
        };
        self.client
            .write_packet(&ServerboundChatCommand {
                command: format!("tp {} {} {}", position[0], position[1], position[2]),
            })
            .await?;
        let deadline = tokio::time::Instant::now() + self.failure_timeout;
        let mut saw_sync = false;
        let mut saw_feedback = false;
        let mut frames = Vec::new();
        while !saw_sync || !saw_feedback {
            let frame = self
                .next_non_keepalive(deadline, "teleport synchronization and feedback")
                .await?;
            if frame.id == SynchronizePlayerPosition::ID {
                let mut body = frame.body.clone();
                let sync = SynchronizePlayerPosition::decode(&mut body)?;
                self.client
                    .write_packet(&ConfirmTeleportation {
                        teleport_id: sync.teleport_id,
                    })
                    .await?;
                saw_sync = true;
                frames.push(frame);
            } else if frame.id == ClientboundSystemChat::ID {
                if command_feedback_matches(frame.body.clone(), &expected_feedback)? {
                    saw_feedback = true;
                } else {
                    frames.push(frame);
                }
            } else {
                frames.push(frame);
            }
        }
        Ok(frames)
    }

    pub(crate) async fn interact(&mut self, entity_id: i32) -> Result<()> {
        self.client
            .write_packet(&ServerboundInteract {
                entity_id,
                hand: InteractionHand::MainHand,
                location: mc_protocol::packets::play::EntityVec3::ZERO,
                using_secondary_action: false,
            })
            .await
    }

    pub(crate) async fn attack(&mut self, entity_id: i32) -> Result<()> {
        self.client
            .write_packet(&ServerboundAttack { entity_id })
            .await
    }

    pub(crate) async fn move_and_fence(
        &mut self,
        position: [f64; 3],
        flags: MovePlayerFlags,
    ) -> Result<Vec<RawFrame>> {
        self.client
            .write_packet(&ServerboundMovePlayerPosRot {
                x: position[0],
                y: position[1],
                z: position[2],
                yaw: 90.0,
                pitch: 0.0,
                flags,
            })
            .await?;
        let frames = self.protocol_fence("movement command fence").await?;
        for frame in &frames {
            if frame.id == SynchronizePlayerPosition::ID {
                let mut body = frame.body.clone();
                let sync = SynchronizePlayerPosition::decode(&mut body)?;
                self.client
                    .write_packet(&ConfirmTeleportation {
                        teleport_id: sync.teleport_id,
                    })
                    .await?;
            }
        }
        Ok(frames)
    }

    pub(crate) async fn wait_for_position_correction(&mut self, reason: &str) -> Result<RawFrame> {
        let deadline = tokio::time::Instant::now() + self.failure_timeout;
        loop {
            let frame = self.next_non_keepalive(deadline, reason).await?;
            if frame.id != SynchronizePlayerPosition::ID {
                continue;
            }
            let mut body = frame.body.clone();
            let sync = SynchronizePlayerPosition::decode(&mut body)?;
            ensure!(
                body.is_empty(),
                "position correction packet has trailing bytes"
            );
            self.client
                .write_packet(&ConfirmTeleportation {
                    teleport_id: sync.teleport_id,
                })
                .await?;
            return Ok(frame);
        }
    }

    pub(crate) async fn protocol_fence(&mut self, reason: &str) -> Result<Vec<RawFrame>> {
        match self.endpoint.kind {
            ServerKind::Solaris => {
                self.command_window(
                    "status",
                    FeedbackExpectation::TextPrefix("Runtime control:".into()),
                    reason,
                )
                .await
            }
            ServerKind::Vanilla => {
                self.command_window(
                    "list",
                    FeedbackExpectation::TranslationKey("commands.list.players".into()),
                    reason,
                )
                .await
            }
        }
    }

    pub(crate) async fn vanilla_command_fence(
        &mut self,
        command: &str,
        translation_key: &str,
        reason: &str,
    ) -> Result<Vec<RawFrame>> {
        ensure!(
            self.endpoint.kind == ServerKind::Vanilla,
            "vanilla command fence used against {}",
            server_label(self.endpoint.kind)
        );
        self.command_window(
            command,
            FeedbackExpectation::TranslationKey(translation_key.into()),
            reason,
        )
        .await
    }

    async fn command_window(
        &mut self,
        command: &str,
        expected_feedback: FeedbackExpectation,
        reason: &str,
    ) -> Result<Vec<RawFrame>> {
        self.client
            .write_packet(&ServerboundChatCommand {
                command: command.to_owned(),
            })
            .await?;
        let deadline = tokio::time::Instant::now() + self.failure_timeout;
        let mut frames = Vec::new();
        loop {
            let frame = self.next_non_keepalive(deadline, reason).await?;
            if frame.id == ClientboundSystemChat::ID
                && command_feedback_matches(frame.body.clone(), &expected_feedback)?
            {
                return Ok(frames);
            }
            frames.push(frame);
        }
    }

    async fn wait_for_chunk(&mut self, target: (i32, i32)) -> Result<()> {
        let deadline = tokio::time::Instant::now() + self.failure_timeout;
        loop {
            let frame = self
                .next_non_keepalive(deadline, "spawn chunk readiness")
                .await?;
            if frame.id != LevelChunkWithLight::ID {
                continue;
            }
            let mut body = frame.body;
            let chunk = LevelChunkWithLight::decode(&mut body)?;
            ensure!(body.is_empty(), "chunk readiness packet has trailing bytes");
            if (chunk.chunk_x, chunk.chunk_z) == target {
                return Ok(());
            }
        }
    }

    async fn next_non_keepalive(
        &mut self,
        deadline: tokio::time::Instant,
        reason: &str,
    ) -> Result<RawFrame> {
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                bail!("{reason} timed out after {:?}", self.failure_timeout);
            }
            let frame = self
                .client
                .read_frame_with_timeout(remaining)
                .await
                .with_context(|| reason.to_owned())?;
            if frame.id != ClientboundKeepAlive::ID {
                return Ok(frame);
            }
            let mut body = frame.body;
            let keepalive = ClientboundKeepAlive::decode(&mut body)?;
            self.client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await?;
        }
    }
}

fn position_near(actual: [f64; 3], expected: [f64; 3]) -> bool {
    actual
        .into_iter()
        .zip(expected)
        .all(|(actual, expected)| (actual - expected).abs() <= 0.01)
}

pub(crate) fn server_label(kind: ServerKind) -> &'static str {
    match kind {
        ServerKind::Solaris => "Solaris",
        ServerKind::Vanilla => "vanilla",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn literal_text_chat_body(text: &str, trailing_nbt: bool) -> Bytes {
        let mut body = vec![0x0A, 0x08, 0x00, 0x04, b't', b'e', b'x', b't'];
        body.extend_from_slice(&(text.len() as u16).to_be_bytes());
        body.extend_from_slice(text.as_bytes());
        body.push(0x00);
        if trailing_nbt {
            body.push(0x00);
        }
        body.push(0x00);
        Bytes::from(body)
    }

    fn literal_translation_chat_body(key: &str) -> Bytes {
        let mut body = vec![
            0x0A, 0x08, 0x00, 0x09, b't', b'r', b'a', b'n', b's', b'l', b'a', b't', b'e',
        ];
        body.extend_from_slice(&(key.len() as u16).to_be_bytes());
        body.extend_from_slice(key.as_bytes());
        body.extend_from_slice(&[0x00, 0x00]);
        Bytes::from(body)
    }

    fn structured_chat_body(component: mc_nbt::Tag) -> Bytes {
        let mut body = Vec::new();
        mc_nbt::write_network(&mut body, &component).expect("encode chat component fixture");
        body.push(0);
        Bytes::from(body)
    }

    fn trailing_container_set_slot_body() -> Bytes {
        let mut body = Vec::new();
        ClientboundContainerSetSlot {
            container_id: 0,
            state_id: 1,
            slot: 36,
            item_stack: mc_protocol::packets::play::ItemStack::EMPTY,
        }
        .encode(&mut body)
        .expect("encode slot fixture");
        body.push(0x7F);
        Bytes::from(body)
    }

    fn trailing_add_entity_body() -> Bytes {
        let mut body = Vec::new();
        AddEntity {
            entity_id: 17,
            uuid: uuid::Uuid::nil(),
            entity_type_id: 3,
            x: 1.0,
            y: 64.0,
            z: -2.0,
            movement: mc_protocol::packets::play::EntityVec3::ZERO,
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: 0,
        }
        .encode(&mut body)
        .expect("encode AddEntity fixture");
        body.push(0x7F);
        Bytes::from(body)
    }

    #[test]
    fn missing_oracle_reports_the_exact_ignored_gate_prerequisite_path() {
        let repo = tempfile::tempdir().expect("temporary repo root");

        let gate = probe_oracle(repo.path());

        let OracleGate::Skipped { reason } = gate else {
            panic!("missing oracle must skip");
        };
        assert!(
            reason.contains(
                &repo
                    .path()
                    .join(".analysis/server.jar")
                    .display()
                    .to_string()
            )
        );
        assert!(reason.contains("skipping vanilla-backed parity test"));
    }

    #[test]
    fn protocol_position_matching_uses_a_tight_wire_tolerance() {
        assert!(position_near([1.0, 2.0, 3.0], [1.005, 2.0, 3.0]));
        assert!(!position_near([1.0, 2.0, 3.0], [1.02, 2.0, 3.0]));
    }

    #[test]
    fn unrelated_system_chat_cannot_complete_a_command_fence() {
        let matched = command_feedback_matches(
            literal_text_chat_body("Unrelated operator message", false),
            &FeedbackExpectation::ExactText("Summoned minecraft:sheep".into()),
        )
        .expect("decode unrelated chat");

        assert!(!matched);
    }

    #[test]
    fn root_text_matches_only_text_expectations() {
        let body = literal_text_chat_body("Summoned minecraft:sheep", false);

        assert!(
            command_feedback_matches(
                body.clone(),
                &FeedbackExpectation::ExactText("Summoned minecraft:sheep".into()),
            )
            .expect("decode root text fixture")
        );
        assert!(
            command_feedback_matches(
                body.clone(),
                &FeedbackExpectation::TextPrefix("Summoned ".into()),
            )
            .expect("decode root text prefix fixture")
        );
        assert!(
            !command_feedback_matches(
                body,
                &FeedbackExpectation::TranslationKey("Summoned minecraft:sheep".into()),
            )
            .expect("root text is not a translation")
        );
    }

    #[test]
    fn nested_extra_text_cannot_complete_a_root_text_fence() {
        let body = structured_chat_body(mc_nbt::Tag::Compound(vec![
            ("text".into(), mc_nbt::Tag::String("unrelated root".into())),
            (
                "extra".into(),
                mc_nbt::Tag::List(mc_nbt::ListTag {
                    element_type: mc_nbt::tag_type::COMPOUND,
                    elements: vec![mc_nbt::Tag::Compound(vec![(
                        "text".into(),
                        mc_nbt::Tag::String("Summoned minecraft:sheep".into()),
                    )])],
                }),
            ),
        ]));

        assert!(
            !command_feedback_matches(
                body,
                &FeedbackExpectation::ExactText("Summoned minecraft:sheep".into()),
            )
            .expect("decode nested extra fixture")
        );
    }

    #[test]
    fn nested_translation_argument_cannot_complete_a_root_translation_fence() {
        let body = structured_chat_body(mc_nbt::Tag::Compound(vec![
            (
                "translate".into(),
                mc_nbt::Tag::String("chat.type.text".into()),
            ),
            (
                "with".into(),
                mc_nbt::Tag::List(mc_nbt::ListTag {
                    element_type: mc_nbt::tag_type::COMPOUND,
                    elements: vec![mc_nbt::Tag::Compound(vec![(
                        "translate".into(),
                        mc_nbt::Tag::String("commands.summon.success".into()),
                    )])],
                }),
            ),
        ]));

        assert!(
            !command_feedback_matches(
                body,
                &FeedbackExpectation::TranslationKey("commands.summon.success".into()),
            )
            .expect("decode nested translation argument fixture")
        );
    }

    #[test]
    fn evidence_container_set_slot_rejects_trailing_bytes() {
        let error = decode_evidence_container_set_slot(trailing_container_set_slot_body())
            .expect_err("trailing slot bytes must reject evidence");

        assert!(error.to_string().contains("trailing bytes"));
    }

    #[test]
    fn evidence_add_entity_rejects_trailing_bytes() {
        let error = decode_evidence_add_entity(trailing_add_entity_body())
            .expect_err("trailing AddEntity bytes must reject evidence");

        assert!(error.to_string().contains("trailing bytes"));
    }

    #[test]
    fn command_feedback_rejects_trailing_nbt_bytes() {
        let error = command_feedback_matches(
            literal_text_chat_body("Summoned minecraft:sheep", true),
            &FeedbackExpectation::ExactText("Summoned minecraft:sheep".into()),
        )
        .expect_err("trailing NBT must not fence a command");

        assert!(error.to_string().contains("trailing"));
    }

    #[test]
    fn fixture_success_requires_the_exact_translation_key() {
        let body = literal_translation_chat_body("commands.setblock.success");

        assert!(
            command_feedback_matches(
                body.clone(),
                &FeedbackExpectation::TranslationKey("commands.setblock.success".into()),
            )
            .expect("decode exact fixture feedback")
        );
        assert!(
            !command_feedback_matches(
                body,
                &FeedbackExpectation::TranslationKey("commands.fill.success".into()),
            )
            .expect("decode unrelated fixture feedback")
        );
        assert!(
            !command_feedback_matches(
                literal_translation_chat_body("commands.setblock.success"),
                &FeedbackExpectation::ExactText("commands.setblock.success".into()),
            )
            .expect("root translation is not literal text")
        );
    }

    #[test]
    fn unsupported_chat_component_shape_cannot_complete_a_command_fence() {
        assert!(
            !command_feedback_matches(
                Bytes::from_static(&[0x00, 0x00]),
                &FeedbackExpectation::TranslationKey("commands.list.players".into()),
            )
            .expect("unsupported unrelated component is a non-match")
        );
    }
}
