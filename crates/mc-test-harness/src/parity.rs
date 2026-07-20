//! Autonomous parity-test primitives for comparing Solaris with a local vanilla
//! oracle.
//!
//! This module deliberately keeps Mojang-owned artifacts outside the repo. The
//! vanilla launcher only looks for a developer-supplied `.analysis/server.jar`
//! and creates all runtime files in a temporary directory.

use std::fmt;
use std::future::Future;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundChangeDifficulty, ClientboundCommands, ClientboundContainerSetContent,
    ClientboundContainerSetSlot, ClientboundInitializeBorder, ClientboundKeepAlive,
    ClientboundPlayerAbilities, ClientboundSetHealth, ClientboundSetHeldSlot, ClientboundSetTime,
    ConfirmTeleportation, EntityEvent, GameEvent, LoginPlay, MovePlayerFlags, ServerboundKeepAlive,
    ServerboundMovePlayerPos, ServerboundMovePlayerRot, ServerboundMovePlayerStatusOnly,
    ServerboundPlayerLoaded, SetCenterChunk, SetDefaultSpawnPosition, SynchronizePlayerPosition,
};
use serde::{Deserialize, Serialize};

use crate::client::Client;

/// Default local path for the vanilla oracle jar. The jar itself is never
/// tracked by git.
pub const DEFAULT_VANILLA_JAR: &str = ".analysis/server.jar";

const CLIENTBOUND_RECIPE_BOOK_ADD_ID: i32 = 0x4A;
const CLIENTBOUND_RECIPE_BOOK_REMOVE_ID: i32 = 0x4B;
const CLIENTBOUND_RECIPE_BOOK_SETTINGS_ID: i32 = 0x4C;
const CLIENTBOUND_PLAYER_INFO_UPDATE_ID: i32 = 0x46;
const CLIENTBOUND_SERVER_DATA_ID: i32 = 0x56;
const CLIENTBOUND_TICKING_STATE_ID: i32 = 0x7F;
const CLIENTBOUND_TICKING_STEP_ID: i32 = 0x80;
const CLIENTBOUND_UPDATE_RECIPES_ID: i32 = 0x85;

/// Result of probing for a local vanilla oracle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OracleAvailability {
    Available {
        jar: PathBuf,
    },
    Missing {
        expected: PathBuf,
    },
    JavaTooOld {
        jar: PathBuf,
        found_major: Option<u32>,
    },
}

impl OracleAvailability {
    #[must_use]
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }

    #[must_use]
    pub fn skip_message(&self) -> Option<String> {
        match self {
            Self::Available { .. } => None,
            Self::Missing { expected } => Some(format!(
                "skipping vanilla-backed parity test: {} missing; put a Mojang 26.1.x server jar there",
                expected.display()
            )),
            Self::JavaTooOld { jar, found_major } => Some(format!(
                "skipping vanilla-backed parity test: {} requires Java 25+; found {}",
                jar.display(),
                found_major.map_or_else(
                    || "unknown Java".to_string(),
                    |major| format!("Java {major}")
                ),
            )),
        }
    }
}

/// Locate the developer-supplied vanilla oracle jar under a repository root and
/// verify that `java` is new enough for 26.1.x class files.
#[must_use]
pub fn vanilla_oracle_availability(repo_root: impl AsRef<Path>) -> OracleAvailability {
    let expected = repo_root.as_ref().join(DEFAULT_VANILLA_JAR);
    if !expected.is_file() {
        return OracleAvailability::Missing { expected };
    }
    let found_major = java_major_version();
    if found_major.is_some_and(|major| major >= 25) {
        OracleAvailability::Available { jar: expected }
    } else {
        OracleAvailability::JavaTooOld {
            jar: expected,
            found_major,
        }
    }
}

#[must_use]
pub fn java_major_version() -> Option<u32> {
    let output = Command::new("java").arg("-version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stderr);
    parse_java_major_version(&text)
}

#[must_use]
pub fn parse_java_major_version(version_output: &str) -> Option<u32> {
    let marker = "version \"";
    let start = version_output.find(marker)? + marker.len();
    let rest = &version_output[start..];
    let version = rest.split('"').next()?;
    let first = version.split('.').next()?;
    first.parse().ok()
}

/// A fact captured from either Solaris or vanilla.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObservationFact {
    PacketSeen {
        id: i32,
    },
    SpawnPosition {
        x: i64,
        y: i64,
        z: i64,
    },
    BlockState {
        x: i32,
        y: i32,
        z: i32,
        state_id: u32,
    },
    InventoryContent {
        container_id: i32,
        state_id: i32,
        slots: u16,
        non_empty_slots: u16,
        carried_count: i32,
    },
    Health {
        half_hearts_milli: i32,
        food: i32,
    },
    /// A single container slot's observed content (from ClientboundContainerSetSlot).
    ContainerSlotContent {
        container_id: i32,
        state_id: i32,
        slot: i16,
        item_id: u32,
        count: i32,
    },
    /// An entity was added to the world (AddEntity).
    EntitySpawned {
        entity_id: i32,
        entity_type_id: i32,
        x: i64,
        y: i64,
        z: i64,
    },
    /// An entity was removed from the world (RemoveEntities); one fact per removed entity.
    EntityRemoved {
        entity_id: i32,
    },
    /// A projectile lifecycle event (EntityEvent), e.g. arrow critical hit, snowball land.
    ProjectileEvent {
        entity_id: i32,
        event_id: i8,
    },
    /// An item-entity drop or pickup (ClientboundTakeItemEntity).
    DropEvent {
        item_entity_id: i32,
        player_entity_id: Option<i32>,
        amount: i32,
    },
    /// Server instructed the client to select a hotbar slot (ClientboundSetHeldSlot).
    HeldSlotChanged {
        slot: i32,
    },
    Note {
        key: String,
        value: String,
    },
}

/// Ordered observations for one scenario phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationSet {
    pub subject: String,
    pub phase: String,
    facts: Vec<ObservationFact>,
}

impl ObservationSet {
    #[must_use]
    pub fn new(subject: impl Into<String>, phase: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
            phase: phase.into(),
            facts: Vec::new(),
        }
    }

    pub fn push(&mut self, fact: ObservationFact) {
        self.facts.push(fact);
    }

    #[must_use]
    pub fn normalize_sequence(self) -> Self {
        self
    }

    /// Deliberately discard order and duplicate facts for a set-like observation.
    ///
    /// Protocol and gameplay observations must use [`Self::normalize_sequence`]
    /// instead so repeated packets and their order remain observable.
    #[must_use]
    pub fn normalize_set(mut self) -> Self {
        self.facts.sort();
        self.facts.dedup();
        self
    }

    #[must_use]
    pub fn facts(&self) -> &[ObservationFact] {
        &self.facts
    }
}

/// Human-readable diff between two ordered observation sequences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationDiff {
    pub phase: String,
    pub missing_from_actual: Vec<ObservationFact>,
    pub unexpected_in_actual: Vec<ObservationFact>,
}

impl ObservationDiff {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.missing_from_actual.is_empty() && self.unexpected_in_actual.is_empty()
    }
}

impl fmt::Display for ObservationDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return write!(f, "{}: observations match", self.phase);
        }
        writeln!(f, "{}: observation mismatch", self.phase)?;
        for fact in &self.missing_from_actual {
            writeln!(f, "  missing from actual: {fact:?}")?;
        }
        for fact in &self.unexpected_in_actual {
            writeln!(f, "  unexpected in actual: {fact:?}")?;
        }
        Ok(())
    }
}

#[must_use]
pub fn diff_observations(expected: &ObservationSet, actual: &ObservationSet) -> ObservationDiff {
    let first_mismatch = expected
        .facts
        .iter()
        .zip(&actual.facts)
        .position(|(expected, actual)| expected != actual)
        .unwrap_or_else(|| expected.facts.len().min(actual.facts.len()));
    ObservationDiff {
        phase: expected.phase.clone(),
        missing_from_actual: expected.facts[first_mismatch..].to_vec(),
        unexpected_in_actual: actual.facts[first_mismatch..].to_vec(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerKind {
    Solaris,
    Vanilla,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CoreAction {
    WaitTicks { ticks: u8 },
    MoveBy { dx_cm: i16, dz_cm: i16 },
    Look { yaw_deg: i16, pitch_deg: i16 },
    Reconnect,
}

impl CoreAction {
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::WaitTicks { ticks } => format!("wait:{ticks}"),
            Self::MoveBy { dx_cm, dz_cm } => format!("move:{dx_cm},{dz_cm}"),
            Self::Look { yaw_deg, pitch_deg } => format!("look:{yaw_deg},{pitch_deg}"),
            Self::Reconnect => "reconnect".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CoreActionGenerator {
    state: u64,
}

impl CoreActionGenerator {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    #[must_use]
    pub fn generate(seed: u64, count: usize) -> Vec<CoreAction> {
        let mut generator = Self::new(seed);
        (0..count).map(|_| generator.next_action()).collect()
    }

    pub fn next_action(&mut self) -> CoreAction {
        match self.next_u32() % 4 {
            0 => CoreAction::WaitTicks {
                ticks: 1 + (self.next_u32() % 20) as u8,
            },
            1 => CoreAction::MoveBy {
                dx_cm: (self.next_u32() % 401) as i16 - 200,
                dz_cm: (self.next_u32() % 401) as i16 - 200,
            },
            2 => CoreAction::Look {
                yaw_deg: (self.next_u32() % 360) as i16 - 180,
                pitch_deg: (self.next_u32() % 181) as i16 - 90,
            },
            _ => CoreAction::Reconnect,
        }
    }

    fn next_u32(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        (self.state >> 32) as u32
    }
}

#[must_use]
pub fn shrink_action_prefix(actions: &[CoreAction], failing_len: usize) -> &[CoreAction] {
    &actions[..actions.len().min(failing_len)]
}

#[derive(Debug, Clone, Copy)]
pub struct ScenarioContext {
    pub kind: ServerKind,
    pub addr: SocketAddr,
}

pub type ScenarioFuture<'a> = Pin<Box<dyn Future<Output = Result<ObservationSet>> + Send + 'a>>;

pub async fn read_typed_skipping_startup_noise<P: Packet>(client: &mut Client) -> Result<P> {
    loop {
        let mut frame = client.read_frame().await?;
        if frame.id == P::ID {
            let body = frame.body.clone();
            return P::decode(&mut frame.body)
                .with_context(|| format!("decode packet id 0x{:02X} body={:02X?}", P::ID, body));
        }
        if is_startup_noise_packet(frame.id) {
            continue;
        }
        bail!(
            "unexpected packet id: want 0x{:02X}, got 0x{:02X}",
            P::ID,
            frame.id
        );
    }
}

pub async fn read_packet_id_skipping_startup_noise(
    client: &mut Client,
    packet_id: i32,
) -> Result<()> {
    loop {
        let frame = client.read_frame().await?;
        if frame.id == packet_id {
            return Ok(());
        }
        if is_startup_noise_packet(frame.id) {
            continue;
        }
        bail!(
            "unexpected packet id: want 0x{packet_id:02X}, got 0x{:02X}",
            frame.id
        );
    }
}

fn is_startup_noise_packet(packet_id: i32) -> bool {
    matches!(
        packet_id,
        CLIENTBOUND_RECIPE_BOOK_ADD_ID
            | CLIENTBOUND_RECIPE_BOOK_REMOVE_ID
            | CLIENTBOUND_RECIPE_BOOK_SETTINGS_ID
            | CLIENTBOUND_PLAYER_INFO_UPDATE_ID
            | CLIENTBOUND_SERVER_DATA_ID
            | CLIENTBOUND_TICKING_STATE_ID
            | CLIENTBOUND_TICKING_STEP_ID
            | CLIENTBOUND_UPDATE_RECIPES_ID
    )
}

/// Shared scenario surface. Implementors run the same logical flow against a
/// supplied server address and return ordered observations for diffing.
pub trait ParityScenario: Send + Sync {
    fn name(&self) -> &'static str;
    fn run<'a>(&'a self, ctx: ScenarioContext) -> ScenarioFuture<'a>;
}

/// Deterministic core-action scenario shared by Solaris-only smoke tests and
/// vanilla-backed parity diffs. It drives the login/play prelude, then executes
/// a small movement/look/wait sequence while recording ordered observations.
#[derive(Debug, Clone)]
pub struct CoreActionSequenceScenario {
    name: &'static str,
    actions: Vec<CoreAction>,
}

impl CoreActionSequenceScenario {
    #[must_use]
    pub fn new(name: &'static str, actions: Vec<CoreAction>) -> Self {
        Self { name, actions }
    }

    #[must_use]
    pub fn actions(&self) -> &[CoreAction] {
        &self.actions
    }
}

impl ParityScenario for CoreActionSequenceScenario {
    fn name(&self) -> &'static str {
        self.name
    }

    fn run<'a>(&'a self, ctx: ScenarioContext) -> ScenarioFuture<'a> {
        Box::pin(async move { observe_core_action_sequence(ctx, self.name, &self.actions).await })
    }
}

pub async fn observe_core_action_sequence(
    ctx: ScenarioContext,
    phase: &str,
    actions: &[CoreAction],
) -> Result<ObservationSet> {
    let subject = match ctx.kind {
        ServerKind::Solaris => "solaris",
        ServerKind::Vanilla => "vanilla",
    };
    let mut client = Client::connect(ctx.addr).await?;
    let _login = client.drive_login(ctx.addr, subject).await?;
    client.drive_configuration().await?;

    let mut observations = ObservationSet::new(subject, phase);
    let _: LoginPlay = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen { id: LoginPlay::ID });
    let _: ClientboundChangeDifficulty = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundChangeDifficulty::ID,
    });
    let _: ClientboundPlayerAbilities = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundPlayerAbilities::ID,
    });
    let _: ClientboundSetHeldSlot = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundSetHeldSlot::ID,
    });
    let _: EntityEvent = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: EntityEvent::ID,
    });
    read_packet_id_skipping_startup_noise(&mut client, ClientboundCommands::ID).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundCommands::ID,
    });
    let sync: SynchronizePlayerPosition = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: SynchronizePlayerPosition::ID,
    });
    observations.push(ObservationFact::Note {
        key: "spawn_position_received".into(),
        value: "true".into(),
    });
    let _: ClientboundInitializeBorder = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundInitializeBorder::ID,
    });
    let _: ClientboundSetTime = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundSetTime::ID,
    });
    let _: SetDefaultSpawnPosition = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: SetDefaultSpawnPosition::ID,
    });
    let _: GameEvent = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen { id: GameEvent::ID });
    let _: SetCenterChunk = read_typed_skipping_startup_noise(&mut client).await?;
    observations.push(ObservationFact::PacketSeen {
        id: SetCenterChunk::ID,
    });
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await?;
    client.write_packet(&ServerboundPlayerLoaded).await?;

    let mut saw_inventory = false;
    execute_core_actions(
        &mut client,
        &mut observations,
        &mut saw_inventory,
        actions,
        sync.x,
        sync.y,
        sync.z,
    )
    .await?;
    observe_post_action_liveness(&mut client, &mut observations, saw_inventory).await?;
    Ok(observations.normalize_sequence())
}

async fn observe_post_action_liveness(
    client: &mut Client,
    observations: &mut ObservationSet,
    mut saw_inventory: bool,
) -> Result<()> {
    if saw_inventory {
        observations.push(ObservationFact::Note {
            key: "post_action_liveness".into(),
            value: "clientbound_frame".into(),
        });
        return Ok(());
    }
    let mut saw_frame = false;
    for index in 0..64 {
        let timeout = if index == 0 {
            Duration::from_secs(5)
        } else {
            Duration::from_millis(250)
        };
        let frame = match client.read_frame_with_timeout(timeout).await {
            Ok(frame) => frame,
            Err(err) if saw_frame => {
                return Err(err).context("post-action frame drain ended before inventory snapshot");
            }
            Err(err) => return Err(err).context("wait for post-action server liveness frame"),
        };
        saw_frame = true;
        observe_core_frame(
            client,
            observations,
            frame.id,
            &frame.body,
            &mut saw_inventory,
        )
        .await?;

        if saw_inventory {
            break;
        }
    }
    if !saw_inventory {
        bail!("post-action inventory snapshot was not observed");
    }
    observations.push(ObservationFact::Note {
        key: "post_action_liveness".into(),
        value: "clientbound_frame".into(),
    });
    Ok(())
}

async fn observe_core_frame(
    client: &mut Client,
    observations: &mut ObservationSet,
    id: i32,
    body: &bytes::Bytes,
    saw_inventory: &mut bool,
) -> Result<Option<i64>> {
    // Ambient entity packets belong to the dedicated lifecycle scenario. Their
    // runtime IDs depend on chunk completion order and are not core-action state.
    if id == ClientboundKeepAlive::ID {
        let mut body = body.clone();
        let keepalive = ClientboundKeepAlive::decode(&mut body)?;
        client
            .write_packet(&ServerboundKeepAlive { id: keepalive.id })
            .await?;
    } else if id == ClientboundSetTime::ID {
        let mut body = body.clone();
        return Ok(Some(ClientboundSetTime::decode(&mut body)?.game_time));
    } else if id == ClientboundSetHeldSlot::ID {
        let mut body = body.clone();
        let held = ClientboundSetHeldSlot::decode(&mut body)?;
        observations.push(ObservationFact::HeldSlotChanged { slot: held.slot });
    } else if id == ClientboundContainerSetContent::ID {
        let mut body = body.clone();
        let inventory = ClientboundContainerSetContent::decode(&mut body)?;
        let slots = u16::try_from(inventory.items.len())
            .context("inventory slot count exceeds observation range")?;
        let non_empty_slots = u16::try_from(
            inventory
                .items
                .iter()
                .filter(|item| !item.is_empty())
                .count(),
        )
        .context("non-empty inventory slot count exceeds observation range")?;
        observations.push(ObservationFact::InventoryContent {
            container_id: inventory.container_id,
            state_id: inventory.state_id,
            slots,
            non_empty_slots,
            carried_count: inventory.carried_item.count,
        });
        *saw_inventory = true;
    } else if id == ClientboundSetHealth::ID {
        let mut body = body.clone();
        let health = ClientboundSetHealth::decode(&mut body)?;
        observations.push(ObservationFact::Health {
            half_hearts_milli: (health.health * 1000.0).round() as i32,
            food: health.food,
        });
    } else if id == ClientboundContainerSetSlot::ID {
        let mut body = body.clone();
        let slot = ClientboundContainerSetSlot::decode(&mut body)?;
        observations.push(ObservationFact::ContainerSlotContent {
            container_id: slot.container_id,
            state_id: slot.state_id,
            slot: slot.slot,
            item_id: slot.item_stack.item_id,
            count: slot.item_stack.count,
        });
    }
    Ok(None)
}

async fn wait_for_server_ticks(
    client: &mut Client,
    observations: &mut ObservationSet,
    saw_inventory: &mut bool,
    ticks: u64,
) -> Result<()> {
    if ticks == 0 {
        return Ok(());
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut baseline = None;
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .context("wait for server tick notification")?;
        let Some(game_time) =
            observe_core_frame(client, observations, frame.id, &frame.body, saw_inventory).await?
        else {
            continue;
        };
        let start = *baseline.get_or_insert(game_time);
        if game_time.saturating_sub(start) >= i64::try_from(ticks)? {
            return Ok(());
        }
    }
}

async fn execute_core_actions(
    client: &mut Client,
    observations: &mut ObservationSet,
    saw_inventory: &mut bool,
    actions: &[CoreAction],
    mut x: f64,
    y: f64,
    mut z: f64,
) -> Result<()> {
    let mut yaw = 0.0_f32;
    let mut pitch = 0.0_f32;
    let flags = MovePlayerFlags::new(false, false);

    for (index, action) in actions.iter().enumerate() {
        observations.push(ObservationFact::Note {
            key: format!("action.{index}"),
            value: action.summary(),
        });
        match *action {
            CoreAction::WaitTicks { ticks } => {
                client
                    .write_packet(&ServerboundMovePlayerStatusOnly { flags })
                    .await?;
                observations.push(ObservationFact::PacketSeen {
                    id: ServerboundMovePlayerStatusOnly::ID,
                });
                wait_for_server_ticks(client, observations, saw_inventory, u64::from(ticks))
                    .await?;
            }
            CoreAction::MoveBy { dx_cm, dz_cm } => {
                x += f64::from(dx_cm) / 100.0;
                z += f64::from(dz_cm) / 100.0;
                client
                    .write_packet(&ServerboundMovePlayerPos { x, y, z, flags })
                    .await?;
                observations.push(ObservationFact::PacketSeen {
                    id: ServerboundMovePlayerPos::ID,
                });
            }
            CoreAction::Look { yaw_deg, pitch_deg } => {
                yaw = f32::from(yaw_deg);
                pitch = f32::from(pitch_deg);
                client
                    .write_packet(&ServerboundMovePlayerRot { yaw, pitch, flags })
                    .await?;
                observations.push(ObservationFact::PacketSeen {
                    id: ServerboundMovePlayerRot::ID,
                });
            }
            CoreAction::Reconnect => {
                observations.push(ObservationFact::Note {
                    key: format!("action.{index}.skipped"),
                    value: "reconnect".into(),
                });
            }
        }
    }

    client
        .write_packet(&ServerboundMovePlayerRot { yaw, pitch, flags })
        .await?;
    observations.push(ObservationFact::Note {
        key: "actions_executed".into(),
        value: actions.len().to_string(),
    });
    Ok(())
}

/// Pick a currently-free localhost port for a child server process.
pub fn reserve_local_port() -> Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).context("reserve localhost port")?;
    Ok(listener.local_addr()?.port())
}

/// Owns a running vanilla oracle process. Dropping it tries to stop the server
/// gracefully before killing the child as a fallback.
pub struct VanillaServerProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    log_rx: mpsc::Receiver<String>,
    addr: SocketAddr,
}

impl VanillaServerProcess {
    pub fn launch(jar: &Path, work_dir: &Path, timeout: Duration) -> Result<Self> {
        if !jar.is_file() {
            bail!("vanilla server jar missing: {}", jar.display());
        }
        std::fs::create_dir_all(work_dir)
            .with_context(|| format!("create {}", work_dir.display()))?;
        std::fs::write(work_dir.join("eula.txt"), "eula=true\n")?;
        let port = reserve_local_port()?;
        std::fs::write(
            work_dir.join("server.properties"),
            format!(
                "online-mode=false\nserver-ip=127.0.0.1\nserver-port={port}\nlevel-name=world\nview-distance=2\nsimulation-distance=2\nspawn-protection=0\nallow-flight=true\n"
            ),
        )?;

        let java = std::env::var_os("JAVA").unwrap_or_else(|| "java".into());
        let mut child = Command::new(&java)
            .arg("-Xms256M")
            .arg("-Xmx1G")
            .arg("-jar")
            .arg(jar)
            .arg("nogui")
            .current_dir(work_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "launch vanilla oracle {} with {}",
                    jar.display(),
                    Path::new(&java).display()
                )
            })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("missing child stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("missing child stderr"))?;
        let stdin = child.stdin.take();
        let (tx, rx) = mpsc::channel();
        spawn_log_watcher(stdout, tx.clone());
        spawn_log_watcher(stderr, tx);

        let deadline = Instant::now() + timeout;
        let mut recent = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                let _ = child.kill();
                bail!(
                    "timed out waiting for vanilla oracle: {}",
                    recent.join("\n")
                );
            }
            match rx.recv_timeout(remaining) {
                Ok(line) => {
                    recent.push(line.clone());
                    if recent.len() > 20 {
                        recent.remove(0);
                    }
                    if line.contains("Done") || line.contains("For help, type") {
                        return Ok(Self {
                            child,
                            stdin,
                            log_rx: rx,
                            addr: SocketAddr::from(([127, 0, 0, 1], port)),
                        });
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let _ = child.kill();
                    bail!(
                        "timed out waiting for vanilla oracle: {}",
                        recent.join("\n")
                    );
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let status = child.wait()?;
                    bail!(
                        "vanilla oracle exited before ready with {status}: {}",
                        recent.join("\n")
                    );
                }
            }
        }
    }

    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn send_command(&mut self, command: &str) -> Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("vanilla oracle stdin is closed"))?;
        stdin.write_all(command.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(())
    }

    pub fn wait_for_log(
        &mut self,
        timeout: Duration,
        matches: impl Fn(&str) -> bool,
    ) -> Result<String> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!("timed out waiting for vanilla oracle log event");
            }
            match self.log_rx.recv_timeout(remaining) {
                Ok(line) if matches(&line) => return Ok(line),
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let status = self.child.wait()?;
                    bail!("vanilla oracle exited while waiting for log event: {status}");
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    bail!("timed out waiting for vanilla oracle log event");
                }
            }
        }
    }

    pub fn stop(mut self) -> Result<()> {
        self.stop_inner()
    }

    fn stop_inner(&mut self) -> Result<()> {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.write_all(b"stop\n");
            let _ = stdin.flush();
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                let _ = self.child.kill();
                let _ = self.child.wait();
                bail!("timed out waiting for vanilla oracle to stop");
            }
            match self.log_rx.recv_timeout(remaining) {
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = self.child.wait()?;
                    return Ok(());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    bail!("timed out waiting for vanilla oracle to stop");
                }
            }
        }
    }
}

impl Drop for VanillaServerProcess {
    fn drop(&mut self) {
        let _ = self.stop_inner();
    }
}

fn spawn_log_watcher<R>(reader: R, tx: mpsc::Sender<String>)
where
    R: std::io::Read + Send + 'static,
{
    thread::spawn(move || {
        let reader = BufReader::new(reader);
        for line in reader.lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modern_java_major_versions() {
        assert_eq!(
            parse_java_major_version("openjdk version \"25.0.1\" 2026-01-01"),
            Some(25)
        );
        assert_eq!(
            parse_java_major_version("openjdk version \"21.0.9\" 2025-10-21 LTS"),
            Some(21)
        );
    }

    #[test]
    fn missing_oracle_reports_clear_skip_message() {
        let temp = tempfile::tempdir().expect("tempdir");
        let availability = vanilla_oracle_availability(temp.path());
        assert!(!availability.is_available());
        let message = availability.skip_message().expect("skip message");
        assert!(message.contains(".analysis/server.jar"));
        assert!(message.contains("skipping vanilla-backed parity test"));
    }

    #[test]
    fn core_action_generator_is_seeded_and_shrinkable() {
        let first = CoreActionGenerator::generate(0x51, 8);
        let second = CoreActionGenerator::generate(0x51, 8);
        let different = CoreActionGenerator::generate(0x52, 8);

        assert_eq!(first, second);
        assert_ne!(first, different);
        assert_eq!(shrink_action_prefix(&first, 3), &first[..3]);
        assert_eq!(shrink_action_prefix(&first, 99), &first[..]);
        assert!(first.iter().all(|action| !action.summary().is_empty()));
    }

    #[test]
    fn observation_diff_reports_fact_mismatch_and_points_at_phase() {
        let mut expected = ObservationSet::new("vanilla", "break-dirt");
        expected.push(ObservationFact::PacketSeen { id: 0x23 });
        expected.push(ObservationFact::BlockState {
            x: 0,
            y: 64,
            z: 0,
            state_id: 0,
        });

        let mut actual = ObservationSet::new("solaris", "break-dirt");
        actual.push(ObservationFact::PacketSeen { id: 0x23 });
        actual.push(ObservationFact::BlockState {
            x: 0,
            y: 64,
            z: 0,
            state_id: 1,
        });

        let diff = diff_observations(&expected, &actual);
        assert_eq!(diff.phase, "break-dirt");
        assert_eq!(diff.missing_from_actual.len(), 1);
        assert_eq!(diff.unexpected_in_actual.len(), 1);
        assert!(diff.to_string().contains("missing from actual"));
    }

    #[test]
    fn sequence_normalization_preserves_order_and_multiplicity() {
        let mut left = ObservationSet::new("vanilla", "spawn");
        left.push(ObservationFact::PacketSeen { id: 2 });
        left.push(ObservationFact::PacketSeen { id: 1 });
        left.push(ObservationFact::PacketSeen { id: 2 });

        let normalized = left.clone().normalize_sequence();
        assert_eq!(normalized.facts(), left.facts());

        let mut right = ObservationSet::new("solaris", "spawn");
        right.push(ObservationFact::PacketSeen { id: 2 });
        right.push(ObservationFact::PacketSeen { id: 2 });
        right.push(ObservationFact::PacketSeen { id: 1 });

        let diff = diff_observations(&left, &right);
        assert_eq!(diff.missing_from_actual, left.facts()[1..]);
        assert_eq!(diff.unexpected_in_actual, right.facts()[1..]);
    }

    #[test]
    fn set_normalization_discards_order_and_multiplicity() {
        let mut observations = ObservationSet::new("vanilla", "stable-world-state");
        observations.push(ObservationFact::PacketSeen { id: 2 });
        observations.push(ObservationFact::PacketSeen { id: 1 });
        observations.push(ObservationFact::PacketSeen { id: 2 });

        assert_eq!(
            observations.normalize_set().facts(),
            &[
                ObservationFact::PacketSeen { id: 1 },
                ObservationFact::PacketSeen { id: 2 }
            ]
        );
    }
}
