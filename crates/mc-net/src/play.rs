//! Play state handler.
//!
//! M1.g.3 scope: send the four packets a vanilla client expects when
//! transitioning into Play state, then run a `ClientboundKeepAlive` →
//! `ServerboundKeepAlive` loop until the client disconnects or the
//! peer-side keepalive timeout fires.
//!
//! ```text
//! S → C  Login (Play)
//! S → C  Synchronize Player Position
//! S → C  Set Default Spawn Position
//! S → C  Game Event (start_waiting_for_level_chunks)
//! S → C  Keep Alive   (every 15 s; client must echo within 30 s)
//! ```
//!
//! No chunk data is sent — the client renders a black world. That is the
//! M1.g bar; chunk streaming is M2-M3 territory.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use mc_data::block_light::BlockLightTable;
use mc_data::items::ItemRegistry;
use mc_data::{Registry, VanillaData};
use mc_entity::{EntityId, EntityLifecycle, EntityStore, GoalState, SpawnEntity, Vec3};
use mc_protocol::codec::Identifier;
use mc_protocol::frame::{Compression, encode_frame};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, BlockChangedAck, BlockUpdate, ChunkHeightmap, ClientboundContainerSetContent,
    ClientboundContainerSetSlot, ClientboundKeepAlive, ClientboundSetHeldSlot,
    ConfirmTeleportation, EntityAnimation, EntityAnimationAction, EntityPositionSync, EntityVec3,
    ForgetLevelChunk, GameEvent, ItemStack, LevelChunkWithLight, LightData, LightUpdate, LoginPlay,
    MoveEntityPosRot, MovePlayerFlags, PlayerActionKind, PlayerInfoActions, PlayerInfoEntry,
    PlayerInfoRemove, PlayerInfoUpdate, PositionMoveRotation, RemoveEntities, RotateHead,
    SectionBlockChange, SectionBlocksUpdate, ServerboundKeepAlive, ServerboundMovePlayerPos,
    ServerboundMovePlayerPosRot, ServerboundMovePlayerRot, ServerboundMovePlayerStatusOnly,
    ServerboundPlayerAction, ServerboundSetCarriedItem, ServerboundUseItemOn, SetCenterChunk,
    SetEntityMotion, SynchronizePlayerPosition, pack_section_pos, pack_section_relative_pos,
    unpack_block_pos,
};
use mc_world::light::{
    ChunkLight, LightCache, LightWorkspace, apply_block_change_to_light, compute_chunk_light_in,
};
use mc_world::wire::{client_heightmaps, encode_chunk_data, encode_chunk_light};
use mc_world::{Chunk, ChunkPos};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Semaphore, mpsc};
use tokio::time::{MissedTickBehavior, interval};
use tracing::{debug, info, warn};

use crate::connection::{read_frame, write_packet};
use crate::error::ConnectionError;
use crate::login::LoggedInProfile;
use crate::server::{ServerConfig, WorldHandle};
use crate::{
    ChunkPipelinePolicy, ChunkPipelineStopReason, ChunkPriority, ChunkRequest, ChunkScheduler,
};

thread_local! {
    static CHUNK_LIGHT_WORKSPACE: RefCell<LightWorkspace> = RefCell::new(LightWorkspace::new());
}

/// How often we ping the client. Vanilla's value.
pub const KEEPALIVE_PERIOD: Duration = Duration::from_secs(15);
/// How long we wait for the client's echo before disconnecting. Vanilla's value.
pub const KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(30);

const SPAWN_X: f64 = 0.5;
// The bundled test world uses vanilla's flat-preset surface: bedrock
// at Y=-64, dirt at Y=-63..-62, grass at Y=-61. Spawn one block
// above the grass so the client lands cleanly without freefall.
// (M3's old SPAWN_Y=64 worked only because the chunk burst was fast
// enough to land before the client picked up physics; M4's slower
// debug-mode burst exposed the latent bug.)
const DEFAULT_SPAWN_Y: f64 = -59.0;
const SPAWN_Z: f64 = 0.5;
const PLAYER_ENTITY_TYPE_ID: i32 = 155;
const COW_ENTITY_TYPE_ID: i32 = 30;
const SERVER_ENTITY_ID_START: i32 = 1_000_000;
pub(crate) const ENTITY_TICK_PERIOD: Duration = Duration::from_millis(50);
const ENTITY_MOVE_SEND_INTERVAL_TICKS: u64 = 3;

/// Default chunk radius around the player when no operator override is present.
pub const DEFAULT_VIEW_DISTANCE: i32 = 10;

type SessionId = u64;

#[derive(Debug, Clone)]
enum OutboundCommand {
    BlockDeltas(Vec<BlockDelta>),
    LightUpdates(Vec<OutboundLightUpdate>),
    SpawnPlayer(PlayerEntitySnapshot),
    MovePlayer(PlayerEntitySnapshot),
    DespawnPlayer(PlayerEntitySnapshot),
    SpawnEntity(ServerEntitySnapshot),
    MoveEntityRelative(ServerEntityMove),
    DespawnEntity(ServerEntitySnapshot),
    AnimatePlayer { entity_id: i32 },
}

#[derive(Debug, Clone)]
struct OutboundLightUpdate {
    pos: ChunkPos,
    light: ChunkLight,
    wire: LightData,
}

#[derive(Debug, Clone)]
struct SessionRecipient {
    id: SessionId,
    tx: mpsc::Sender<OutboundCommand>,
}

#[derive(Debug, Clone)]
struct VisibilityDispatch {
    recipient: SessionRecipient,
    command: OutboundCommand,
}

#[derive(Debug, Clone)]
struct PlayerEntitySnapshot {
    session_id: SessionId,
    entity_id: i32,
    uuid: uuid::Uuid,
    name: String,
    pose: PlayerPose,
}

#[derive(Debug, Clone)]
struct ServerEntitySnapshot {
    id: EntityId,
    uuid: uuid::Uuid,
    type_id: i32,
    type_name: String,
    position: Vec3,
    rotation: mc_entity::Rotation,
    velocity: Vec3,
    on_ground: bool,
}

#[derive(Debug, Clone)]
struct ServerEntityMove {
    id: EntityId,
    delta: Vec3,
    rotation: mc_entity::Rotation,
    on_ground: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EntityPhysicsQuery {
    pub id: EntityId,
    pub position: Vec3,
    pub velocity: Vec3,
    pub on_ground: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EntityPhysicsStep {
    pub id: EntityId,
    pub position: Vec3,
    pub velocity: Vec3,
    pub on_ground: bool,
}

#[derive(Debug)]
struct PlaySession {
    name: String,
    uuid: uuid::Uuid,
    entity_id: i32,
    pose: PlayerPose,
    center: (i32, i32),
    view_distance: i32,
    desired: HashSet<(i32, i32)>,
    loaded: HashSet<(i32, i32)>,
    visible_players: HashSet<SessionId>,
    visible_entities: HashSet<EntityId>,
    tx: mpsc::Sender<OutboundCommand>,
}

#[derive(Debug)]
struct SessionRegistryInner {
    next_id: SessionId,
    sessions: HashMap<SessionId, PlaySession>,
    tickets: HashMap<(i32, i32), HashSet<SessionId>>,
    prepared: HashMap<(i32, i32), Arc<PreparedChunkFrame>>,
    entities: EntityStore,
    last_sent_entity_positions: HashMap<EntityId, Vec3>,
    demo_entities_ready: bool,
}

impl Default for SessionRegistryInner {
    fn default() -> Self {
        Self {
            next_id: 0,
            sessions: HashMap::new(),
            tickets: HashMap::new(),
            prepared: HashMap::new(),
            entities: EntityStore::with_next_id(SERVER_ENTITY_ID_START - 1),
            last_sent_entity_positions: HashMap::new(),
            demo_entities_ready: false,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct SessionRegistry {
    inner: Mutex<SessionRegistryInner>,
}

impl SessionRegistry {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn register(
        &self,
        profile: &LoggedInProfile,
        center: (i32, i32),
        view_distance: i32,
        desired: HashSet<(i32, i32)>,
        tx: mpsc::Sender<OutboundCommand>,
        pose: PlayerPose,
    ) -> (SessionId, Vec<VisibilityDispatch>) {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        inner.next_id = inner.next_id.wrapping_add(1).max(1);
        let id = inner.next_id;
        let entity_id = i32::try_from(id).unwrap_or(i32::MAX);
        for &chunk in &desired {
            inner.tickets.entry(chunk).or_default().insert(id);
        }
        inner.sessions.insert(
            id,
            PlaySession {
                name: profile.name.clone(),
                uuid: profile.uuid,
                entity_id,
                pose,
                center,
                view_distance,
                desired,
                loaded: HashSet::new(),
                visible_players: HashSet::new(),
                visible_entities: HashSet::new(),
                tx,
            },
        );
        let dispatches = refresh_visibility_locked(&mut inner);
        debug!(
            session_id = id,
            entity_id,
            player = %profile.name,
            center_cx = center.0,
            center_cz = center.1,
            view_distance,
            sessions = inner.sessions.len(),
            tickets = inner.tickets.len(),
            "play session registered"
        );
        (id, dispatches)
    }

    fn unregister(&self, id: SessionId) -> Vec<VisibilityDispatch> {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        let Some(session) = inner.sessions.remove(&id) else {
            return Vec::new();
        };
        let snapshot = session_snapshot(id, &session);
        let mut dispatches = Vec::new();
        for (&observer_id, observer) in &mut inner.sessions {
            if observer.visible_players.remove(&id) {
                dispatches.push(VisibilityDispatch {
                    recipient: SessionRecipient {
                        id: observer_id,
                        tx: observer.tx.clone(),
                    },
                    command: OutboundCommand::DespawnPlayer(snapshot.clone()),
                });
            }
        }
        let desired_len = session.desired.len();
        let loaded_len = session.loaded.len();
        for chunk in session.desired {
            if remove_ticket(&mut inner.tickets, chunk, id) {
                inner.prepared.remove(&chunk);
            }
        }
        debug!(
            session_id = id,
            player = %session.name,
            desired = desired_len,
            loaded = loaded_len,
            sessions = inner.sessions.len(),
            tickets = inner.tickets.len(),
            "play session unregistered"
        );
        dispatches
    }

    fn replace_view(
        &self,
        id: SessionId,
        center: (i32, i32),
        desired: HashSet<(i32, i32)>,
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        let (released, acquired, desired_len, view_distance) = {
            let Some(session) = inner.sessions.get_mut(&id) else {
                return Vec::new();
            };
            let old = std::mem::replace(&mut session.desired, desired);
            session.center = center;
            (
                old.difference(&session.desired)
                    .copied()
                    .collect::<Vec<_>>(),
                session
                    .desired
                    .difference(&old)
                    .copied()
                    .collect::<Vec<_>>(),
                session.desired.len(),
                session.view_distance,
            )
        };

        for chunk in released {
            if remove_ticket(&mut inner.tickets, chunk, id) {
                inner.prepared.remove(&chunk);
            }
        }
        for chunk in acquired {
            inner.tickets.entry(chunk).or_default().insert(id);
        }
        let dispatches = refresh_visibility_locked(&mut inner);
        debug!(
            session_id = id,
            center_cx = center.0,
            center_cz = center.1,
            view_distance,
            desired = desired_len,
            global_tickets = inner.tickets.len(),
            shared_tickets = inner.tickets.values().filter(|s| s.len() > 1).count(),
            "play session view tickets replaced"
        );
        dispatches
    }

    fn mark_loaded(&self, id: SessionId, chunk: (i32, i32)) -> Vec<VisibilityDispatch> {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        if let Some(session) = inner.sessions.get_mut(&id) {
            session.loaded.insert(chunk);
            refresh_visibility_locked(&mut inner)
        } else {
            Vec::new()
        }
    }

    fn mark_unloaded(&self, id: SessionId, chunks: &[(i32, i32)]) -> Vec<VisibilityDispatch> {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        if let Some(session) = inner.sessions.get_mut(&id) {
            for chunk in chunks {
                session.loaded.remove(chunk);
            }
            refresh_visibility_locked(&mut inner)
        } else {
            Vec::new()
        }
    }

    fn update_pose(&self, id: SessionId, pose: PlayerPose) -> Vec<VisibilityDispatch> {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        let old_observers = visible_observers_locked(&inner, id);
        let Some(session) = inner.sessions.get_mut(&id) else {
            return Vec::new();
        };
        session.pose = pose;
        let mut dispatches = refresh_visibility_locked(&mut inner);
        let new_observers = visible_observers_locked(&inner, id);
        let Some(snapshot) = inner
            .sessions
            .get(&id)
            .map(|session| session_snapshot(id, session))
        else {
            return dispatches;
        };
        for observer_id in old_observers.intersection(&new_observers) {
            if let Some(observer) = inner.sessions.get(observer_id) {
                dispatches.push(VisibilityDispatch {
                    recipient: SessionRecipient {
                        id: *observer_id,
                        tx: observer.tx.clone(),
                    },
                    command: OutboundCommand::MovePlayer(snapshot.clone()),
                });
            }
        }
        dispatches
    }

    fn broadcast_player_animation(&self, id: SessionId) -> Vec<VisibilityDispatch> {
        let inner = self.inner.lock().expect("session registry poisoned");
        let Some(session) = inner.sessions.get(&id) else {
            return Vec::new();
        };
        let entity_id = session.entity_id;
        visible_observers_locked(&inner, id)
            .into_iter()
            .filter_map(|observer_id| {
                let observer = inner.sessions.get(&observer_id)?;
                Some(VisibilityDispatch {
                    recipient: SessionRecipient {
                        id: observer_id,
                        tx: observer.tx.clone(),
                    },
                    command: OutboundCommand::AnimatePlayer { entity_id },
                })
            })
            .collect()
    }

    fn ensure_demo_entities(&self, spawn_y: f64) -> Vec<VisibilityDispatch> {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        if !inner.demo_entities_ready {
            let position = Vec3::new(SPAWN_X + 4.0, spawn_y, SPAWN_Z);
            let mut cow = SpawnEntity::new(COW_ENTITY_TYPE_ID, "minecraft:cow", position);
            cow.goal = GoalState::Wander {
                speed: 0.8,
                period_ticks: 80,
            };
            let id = inner.entities.spawn(cow);
            inner.last_sent_entity_positions.insert(id, position);
            inner.demo_entities_ready = true;
            debug!(entity_id = id.0, "spawned demo vanilla cow entity");
        }
        refresh_visibility_locked(&mut inner)
    }

    pub(crate) fn tick_entities_and_collect_physics_queries(
        &self,
        tick: u64,
    ) -> Vec<EntityPhysicsQuery> {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        if inner.entities.is_empty() {
            return Vec::new();
        }
        inner.entities.tick_goals(tick);
        inner
            .entities
            .snapshots()
            .map(|entity| EntityPhysicsQuery {
                id: entity.id,
                position: entity.position,
                velocity: entity.velocity,
                on_ground: entity.on_ground,
            })
            .collect()
    }

    pub(crate) fn apply_entity_physics_and_dispatch(&self, tick: u64, steps: &[EntityPhysicsStep]) {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        if inner.entities.is_empty() {
            return;
        }
        let old_visible: HashMap<_, _> = inner
            .entities
            .snapshots()
            .map(|entity| {
                (
                    entity.id,
                    visible_entity_observers_locked(&inner, entity.id),
                )
            })
            .collect();
        for step in steps {
            let _ = inner.entities.set_position(step.id, step.position);
            let _ = inner.entities.set_velocity(step.id, step.velocity);
            let _ = inner.entities.set_on_ground(step.id, step.on_ground);
        }
        let mut dispatches = refresh_visibility_locked(&mut inner);
        if !tick.is_multiple_of(ENTITY_MOVE_SEND_INTERVAL_TICKS) {
            drop(inner);
            dispatch_visibility_commands(dispatches);
            return;
        }

        let snapshots: Vec<_> = inner.entities.snapshots().collect();
        for entity in snapshots {
            let snapshot = server_entity_snapshot_from(entity);
            let Some(old_position) = inner.last_sent_entity_positions.get(&snapshot.id).copied()
            else {
                inner
                    .last_sent_entity_positions
                    .insert(snapshot.id, snapshot.position);
                continue;
            };
            let delta = Vec3 {
                x: snapshot.position.x - old_position.x,
                y: snapshot.position.y - old_position.y,
                z: snapshot.position.z - old_position.z,
            };
            if delta.x == 0.0 && delta.y == 0.0 && delta.z == 0.0 {
                continue;
            }
            let new_visible = visible_entity_observers_locked(&inner, snapshot.id);
            for observer_id in old_visible
                .get(&snapshot.id)
                .into_iter()
                .flat_map(|observers| observers.intersection(&new_visible))
            {
                if let Some(observer) = inner.sessions.get(observer_id) {
                    dispatches.push(VisibilityDispatch {
                        recipient: SessionRecipient {
                            id: *observer_id,
                            tx: observer.tx.clone(),
                        },
                        command: OutboundCommand::MoveEntityRelative(ServerEntityMove {
                            id: snapshot.id,
                            delta,
                            rotation: snapshot.rotation,
                            on_ground: snapshot.on_ground,
                        }),
                    });
                }
            }
            inner
                .last_sent_entity_positions
                .insert(snapshot.id, snapshot.position);
        }
        drop(inner);
        dispatch_visibility_commands(dispatches);
    }

    fn loaded_recipients_for_chunks(
        &self,
        chunks: &HashSet<(i32, i32)>,
        except: SessionId,
    ) -> Vec<SessionRecipient> {
        let inner = self.inner.lock().expect("session registry poisoned");
        let mut ids = HashSet::new();
        for chunk in chunks {
            if let Some(subscribers) = inner.tickets.get(chunk) {
                ids.extend(subscribers.iter().copied().filter(|id| *id != except));
            }
        }
        ids.into_iter()
            .filter_map(|id| {
                let session = inner.sessions.get(&id)?;
                if !chunks.iter().any(|chunk| session.loaded.contains(chunk)) {
                    return None;
                }
                Some(SessionRecipient {
                    id,
                    tx: session.tx.clone(),
                })
            })
            .collect()
    }

    fn prepared_chunk(&self, chunk: (i32, i32)) -> Option<Arc<PreparedChunkFrame>> {
        let inner = self.inner.lock().expect("session registry poisoned");
        inner.prepared.get(&chunk).cloned()
    }

    fn cache_prepared_chunk(&self, chunk: (i32, i32), prepared: Arc<PreparedChunkFrame>) {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        if inner.tickets.contains_key(&chunk) {
            inner.prepared.entry(chunk).or_insert(prepared);
        }
    }

    fn invalidate_prepared_chunks(&self, chunks: &HashSet<(i32, i32)>) {
        if chunks.is_empty() {
            return;
        }
        let mut inner = self.inner.lock().expect("session registry poisoned");
        for chunk in chunks {
            inner.prepared.remove(chunk);
        }
    }
}

fn session_snapshot(id: SessionId, session: &PlaySession) -> PlayerEntitySnapshot {
    PlayerEntitySnapshot {
        session_id: id,
        entity_id: session.entity_id,
        uuid: session.uuid,
        name: session.name.clone(),
        pose: session.pose,
    }
}

fn server_entity_snapshot_from(entity: mc_entity::EntitySnapshot) -> ServerEntitySnapshot {
    ServerEntitySnapshot {
        id: entity.id,
        uuid: entity.uuid,
        type_id: entity.type_id,
        type_name: entity.type_name,
        position: entity.position,
        rotation: entity.rotation,
        velocity: entity.velocity,
        on_ground: entity.on_ground,
    }
}

fn server_entity_chunk_pos(entity: &ServerEntitySnapshot) -> (i32, i32) {
    chunk_pos_from_coords(entity.position.x, entity.position.z)
}

fn visible_observers_locked(inner: &SessionRegistryInner, target: SessionId) -> HashSet<SessionId> {
    inner
        .sessions
        .iter()
        .filter_map(|(&observer_id, observer)| {
            observer
                .visible_players
                .contains(&target)
                .then_some(observer_id)
        })
        .collect()
}

fn visible_entity_observers_locked(
    inner: &SessionRegistryInner,
    target: EntityId,
) -> HashSet<SessionId> {
    inner
        .sessions
        .iter()
        .filter_map(|(&observer_id, observer)| {
            observer
                .visible_entities
                .contains(&target)
                .then_some(observer_id)
        })
        .collect()
}

fn refresh_visibility_locked(inner: &mut SessionRegistryInner) -> Vec<VisibilityDispatch> {
    let ids: Vec<_> = inner.sessions.keys().copied().collect();
    let snapshots: HashMap<_, _> = inner
        .sessions
        .iter()
        .map(|(&id, session)| (id, session_snapshot(id, session)))
        .collect();
    let entity_snapshots: HashMap<_, _> = inner
        .entities
        .snapshots()
        .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
        .map(|entity| {
            let snapshot = server_entity_snapshot_from(entity);
            (snapshot.id, snapshot)
        })
        .collect();
    let desired_by_observer: HashMap<_, HashSet<_>> = ids
        .iter()
        .filter_map(|observer_id| {
            let observer = inner.sessions.get(observer_id)?;
            let desired = ids
                .iter()
                .copied()
                .filter(|target_id| {
                    target_id != observer_id
                        && snapshots.get(target_id).is_some_and(|target| {
                            observer.loaded.contains(&target.pose.chunk_pos())
                        })
                })
                .collect();
            Some((*observer_id, desired))
        })
        .collect();
    let desired_entities_by_observer: HashMap<_, HashSet<_>> = ids
        .iter()
        .filter_map(|observer_id| {
            let observer = inner.sessions.get(observer_id)?;
            let desired = entity_snapshots
                .iter()
                .filter_map(|(&entity_id, entity)| {
                    observer
                        .loaded
                        .contains(&server_entity_chunk_pos(entity))
                        .then_some(entity_id)
                })
                .collect();
            Some((*observer_id, desired))
        })
        .collect();

    let mut dispatches = Vec::new();
    for observer_id in ids {
        let Some(observer) = inner.sessions.get_mut(&observer_id) else {
            continue;
        };
        let desired = desired_by_observer
            .get(&observer_id)
            .cloned()
            .unwrap_or_default();
        for target_id in desired.difference(&observer.visible_players) {
            if let Some(snapshot) = snapshots.get(target_id) {
                dispatches.push(VisibilityDispatch {
                    recipient: SessionRecipient {
                        id: observer_id,
                        tx: observer.tx.clone(),
                    },
                    command: OutboundCommand::SpawnPlayer(snapshot.clone()),
                });
            }
        }
        for target_id in observer.visible_players.difference(&desired) {
            if let Some(snapshot) = snapshots.get(target_id) {
                dispatches.push(VisibilityDispatch {
                    recipient: SessionRecipient {
                        id: observer_id,
                        tx: observer.tx.clone(),
                    },
                    command: OutboundCommand::DespawnPlayer(snapshot.clone()),
                });
            }
        }
        observer.visible_players = desired;

        let desired_entities = desired_entities_by_observer
            .get(&observer_id)
            .cloned()
            .unwrap_or_default();
        for entity_id in desired_entities.difference(&observer.visible_entities) {
            if let Some(snapshot) = entity_snapshots.get(entity_id) {
                dispatches.push(VisibilityDispatch {
                    recipient: SessionRecipient {
                        id: observer_id,
                        tx: observer.tx.clone(),
                    },
                    command: OutboundCommand::SpawnEntity(snapshot.clone()),
                });
            }
        }
        for entity_id in observer.visible_entities.difference(&desired_entities) {
            if let Some(snapshot) = entity_snapshots.get(entity_id) {
                dispatches.push(VisibilityDispatch {
                    recipient: SessionRecipient {
                        id: observer_id,
                        tx: observer.tx.clone(),
                    },
                    command: OutboundCommand::DespawnEntity(snapshot.clone()),
                });
            }
        }
        observer.visible_entities = desired_entities;
    }
    dispatches
}

fn dispatch_visibility_commands(dispatches: Vec<VisibilityDispatch>) {
    for dispatch in dispatches {
        if let Err(err) = dispatch.recipient.tx.try_send(dispatch.command) {
            debug!(
                recipient = dispatch.recipient.id,
                error = %err,
                "dropping player visibility command"
            );
        }
    }
}

fn remove_ticket(
    tickets: &mut HashMap<(i32, i32), HashSet<SessionId>>,
    chunk: (i32, i32),
    id: SessionId,
) -> bool {
    if let Some(subscribers) = tickets.get_mut(&chunk) {
        subscribers.remove(&id);
        if subscribers.is_empty() {
            tickets.remove(&chunk);
            return true;
        }
    }
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BlockDelta {
    x: i32,
    y: i32,
    z: i32,
    state_id: mc_world::BlockStateId,
}

#[derive(Debug, Clone, Copy)]
struct PlayerPose {
    x: f64,
    y: f64,
    z: f64,
    yaw: f32,
    pitch: f32,
    flags: MovePlayerFlags,
}

impl PlayerPose {
    fn new(x: f64, y: f64, z: f64) -> Self {
        Self {
            x,
            y,
            z,
            yaw: 0.0,
            pitch: 0.0,
            flags: MovePlayerFlags::new(false, false),
        }
    }

    fn chunk_pos(self) -> (i32, i32) {
        chunk_pos_from_coords(self.x, self.z)
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct ChunkBuildTiming {
    chunk_data_ms: u64,
    heightmap_ms: u64,
    light_compute_ms: u64,
    light_encode_ms: u64,
}

impl ChunkBuildTiming {
    fn add(&mut self, other: ChunkBuildTiming) {
        self.chunk_data_ms += other.chunk_data_ms;
        self.heightmap_ms += other.heightmap_ms;
        self.light_compute_ms += other.light_compute_ms;
        self.light_encode_ms += other.light_encode_ms;
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct ChunkWriteTiming {
    packet_encode_ms: u64,
    frame_ms: u64,
    socket_write_ms: u64,
    framed_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChunkStreamStep {
    Progress,
    Complete,
}

struct ChunkStreamState {
    world: WorldHandle,
    biomes: Arc<Registry>,
    block_light: Option<Arc<BlockLightTable>>,
    compression: Compression,
    sessions: Arc<SessionRegistry>,
    session_id: SessionId,
    io_permits: Arc<Semaphore>,
    cpu_permits: Arc<Semaphore>,
    result_tx: mpsc::Sender<ChunkPrepareResult>,
    result_rx: mpsc::Receiver<ChunkPrepareResult>,
    ready: BTreeMap<u32, ChunkPrepareResult>,
    policy: ChunkPipelinePolicy,
    result_queue_size: usize,
    center_cx: i32,
    center_cz: i32,
    view_distance: i32,
    scheduler: ChunkScheduler,
    staged: HashSet<(i32, i32)>,
    loaded: HashSet<(i32, i32)>,
    started: Instant,
    fetch_ms: u64,
    build_timing: ChunkBuildTiming,
    packet_encode_ms: u64,
    frame_ms: u64,
    socket_write_ms: u64,
    framed_bytes: usize,
    first_chunk_ms: Option<u64>,
    ring1_complete_ms: Option<u64>,
    ring2_complete_ms: Option<u64>,
    ring_emitted: Vec<usize>,
    emitted: usize,
    absent: usize,
    bytes: usize,
    dispatch_turns: usize,
    yielded_turns: usize,
    dispatched: usize,
    max_in_flight: usize,
    max_ready: usize,
    last_stop_reason: ChunkPipelineStopReason,
    wait_for_first_chunk: bool,
}

#[derive(Debug, Clone)]
struct PreparedChunkFrame {
    frame: Bytes,
    light: Option<ChunkLight>,
    packet_data_len: usize,
    build_timing: ChunkBuildTiming,
    write_timing: ChunkWriteTiming,
}

enum ChunkPrepareOutcome {
    Ready(Box<PreparedChunkFrame>),
    Absent,
    Failed(String),
}

struct ChunkPrepareResult {
    request: crate::ChunkRequest,
    fetch_ms: u64,
    staged: Vec<(i32, i32)>,
    outcome: ChunkPrepareOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BlockDeltaPacket {
    Single(BlockDelta),
    Section {
        section_x: i32,
        section_y: i32,
        section_z: i32,
        changes: Vec<BlockDelta>,
    },
}

/// Pack `(x, y, z)` into vanilla's `BlockPos` `i64` representation.
/// Currently used only by tests but kept here for the eventual
/// re-introduction of `SetDefaultSpawnPosition` and other block-pos
/// carrying clientbound packets.
#[allow(dead_code)]
fn pack_block_pos(x: i32, y: i32, z: i32) -> i64 {
    (((x as i64) & 0x3FF_FFFF) << 38) | (((z as i64) & 0x3FF_FFFF) << 12) | ((y as i64) & 0xFFF)
}

/// Pick the dimension that the player will spawn into. We pick the first
/// alphabetical entry of `dimension_type` for both real vanilla data
/// (`minecraft:overworld`) and test stubs (`minecraft:alpha`).
fn spawn_dimension(data: &VanillaData) -> Option<(i32, &Identifier, &[Identifier])> {
    let registry = data.registry("dimension_type")?;
    let first = registry.entries.first()?;
    Some((0, first, registry.entries.as_slice()))
}

async fn spawn_position(config: &ServerConfig) -> (f64, f64, f64) {
    let y = adaptive_spawn_y(config).await.unwrap_or(DEFAULT_SPAWN_Y);
    (SPAWN_X, y, SPAWN_Z)
}

async fn adaptive_spawn_y(config: &ServerConfig) -> Option<f64> {
    let world = config.world.as_ref()?;
    let table = config.block_light.as_deref()?;
    let mut storage = world.lock().await;
    let chunk = storage.get_chunk_mut(ChunkPos { x: 0, z: 0 }).ok()??;
    chunk.rebuild_highest_opaque(table);
    let top = chunk.highest_opaque_y(0, 0)?;
    Some((top + 2) as f64)
}

pub(crate) async fn handle<R, W>(
    reader: &mut R,
    writer: &mut W,
    buf: &mut BytesMut,
    compression: Compression,
    profile: &LoggedInProfile,
    config: &ServerConfig,
    sessions: Arc<SessionRegistry>,
) -> Result<(), ConnectionError>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    // `blocks` rides along on the config — currently unused by the
    // Play handler because the chunk encoder reads palette IDs straight
    // from the chunk; it'll matter once we synthesise placeholder
    // chunks or do block-update packets.
    let _ = &config.blocks;
    let data: &VanillaData = &config.data;
    let (dim_id, dim_name, dim_names) = spawn_dimension(data).ok_or_else(|| {
        ConnectionError::Codec(mc_protocol::CodecError::InvalidIdentifier(
            "no dimension_type entries available".into(),
        ))
    })?;

    info!(
        player = %profile.name,
        uuid = %profile.uuid,
        spawn_dimension = %dim_name,
        "entering Play state"
    );

    let (spawn_x, spawn_y, spawn_z) = spawn_position(config).await;
    dispatch_visibility_commands(sessions.ensure_demo_entities(spawn_y));

    let (spawn_cx, spawn_cz) = spawn_chunk_pos();
    let (outbound_tx, outbound_rx) =
        mpsc::channel(config.chunk_pipeline.chunk_result_queue_size.max(16));
    let initial_desired = if config.world.is_some() {
        desired_chunk_set(spawn_cx, spawn_cz, config.view_distance)
    } else {
        HashSet::new()
    };
    let initial_pose = PlayerPose::new(spawn_x, spawn_y, spawn_z);
    let (session_id, visibility) = sessions.register(
        profile,
        (spawn_cx, spawn_cz),
        config.view_distance,
        initial_desired,
        outbound_tx,
        initial_pose,
    );

    // 1. Login (Play).
    let login = LoginPlay {
        entity_id: i32::try_from(session_id).unwrap_or(i32::MAX),
        is_hardcore: false,
        dimension_names: dim_names.to_vec(),
        max_players: 20,
        view_distance: config.view_distance,
        simulation_distance: config.view_distance,
        reduced_debug_info: false,
        enable_respawn_screen: true,
        do_limited_crafting: false,
        dimension_type_id: dim_id,
        dimension_name: dim_name.clone(),
        hashed_seed: 0,
        game_mode: 0, // survival
        previous_game_mode: -1,
        is_debug: false,
        is_flat: true,
        death_location: None,
        portal_cooldown: 0,
        sea_level: 63,
        enforces_secure_chat: false,
    };
    write_packet(writer, &login, compression).await?;
    dispatch_visibility_commands(visibility);

    // 2. Synchronize Player Position. teleport_id=1; we'll watch for
    //    `ConfirmTeleportation(1)` in the loop below but don't block
    //    on it — if the client ignores it the world still loads, just
    //    desynced.
    write_packet(
        writer,
        &SynchronizePlayerPosition {
            teleport_id: 1,
            x: spawn_x,
            y: spawn_y,
            z: spawn_z,
            dx: 0.0,
            dy: 0.0,
            dz: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            relative_flags: 0,
        },
        compression,
    )
    .await?;

    // 3. Set Default Spawn Position — was historically sent here to set
    //    the compass anchor. Skipped in M1.g: in the 26.1.2 wire capture
    //    the matching 8-byte clientbound packet looks like its layout
    //    changed (no `angle` field), and its ID is uncertain. The
    //    client renders without a configured compass target — minor
    //    cosmetic regression, not a protocol error. Re-introduce once
    //    the new shape is verified.

    // 4. Game Event: start waiting for chunks. Tells the client to
    //    drop the loading screen even though no chunks are coming.
    write_packet(
        writer,
        &GameEvent {
            event: GameEvent::EVENT_START_WAITING_FOR_CHUNKS,
            value: 0.0,
        },
        compression,
    )
    .await?;

    // 5. Set Center Chunk + view-distance window. Spawn is at
    //    (SPAWN_X, SPAWN_Z); the chunk anchor is the chunk that
    //    contains it, and we stream ±view_distance around it.
    write_packet(
        writer,
        &SetCenterChunk {
            chunk_x: spawn_cx,
            chunk_z: spawn_cz,
        },
        compression,
    )
    .await?;

    let mut light_cache = LightCache::new();
    let mut chunk_stream = config.world.as_ref().and_then(|world| {
        let biomes = data.registry("worldgen/biome")?;
        Some(ChunkStreamState::new(
            Arc::clone(world),
            Arc::new(biomes.clone()),
            config.block_light.as_ref().map(Arc::clone),
            compression,
            Arc::clone(&sessions),
            session_id,
            spawn_cx,
            spawn_cz,
            config.view_distance,
            config.chunk_pipeline,
        ))
    });
    if config.world.is_some() && chunk_stream.is_none() {
        warn!("worldgen/biome registry missing; skipping chunk emission");
    }
    let result = async {
        if let Some(stream) = chunk_stream.as_mut()
            && stream.step(writer, &mut light_cache).await? == ChunkStreamStep::Complete
        {
            stream.log_summary();
        }

        // 6. Seed the player inventory (M6). Send SetHeldSlot{0} +
        //    ContainerSetContent with the starter kit. Doing this even
        //    when there's no world configured so the hotbar UI fills in
        //    on connect-without-world (the M6 manual gate uses a world,
        //    but tests / chunk-less debug runs should still display the
        //    kit). When the item registry is empty (test stubs) the
        //    starter inventory is mostly empty stacks — packets ship
        //    anyway, just with `count == 0` slots.
        let starter = build_starter_inventory(&config.items);
        write_packet(writer, &ClientboundSetHeldSlot { slot: 0 }, compression).await?;
        write_packet(
            writer,
            &ClientboundContainerSetContent {
                container_id: 0,
                state_id: 1,
                items: starter.as_wire_list(),
                carried_item: ItemStack::EMPTY,
            },
            compression,
        )
        .await?;

        // 7. Play loop. Runs until the connection drops or the client
        //    misses a heartbeat by more than `KEEPALIVE_TIMEOUT`. The
        //    interaction state passes the M5.d/M5.e/M6.f break/place
        //    handlers everything they need to mutate the world and emit
        //    relight + container packets back to the client.
        let mut interaction = config.world.as_ref().map(|world| InteractionState {
            world: Arc::clone(world),
            blocks: Arc::clone(&config.blocks),
            block_light: config.block_light.as_ref().map(Arc::clone),
            sessions: Arc::clone(&sessions),
            session_id,
            workspace: LightWorkspace::new(),
            light_cache: std::mem::take(&mut light_cache),
            compression,
            selected_hotbar_slot: 0,
            inventory: starter,
            inventory_state_id: 1,
            item_to_block: ItemToBlockTable::build(&config.items, &config.blocks),
        });
        play_loop(
            reader,
            writer,
            buf,
            compression,
            interaction.as_mut(),
            chunk_stream,
            Arc::clone(&sessions),
            session_id,
            initial_pose,
            outbound_rx,
        )
        .await
    }
    .await;

    dispatch_visibility_commands(sessions.unregister(session_id));
    result
}

/// Per-connection state the M5.d / M5.e / M6 interaction handlers
/// carry.
struct InteractionState {
    world: WorldHandle,
    blocks: Arc<mc_world::BlockRegistry>,
    block_light: Option<Arc<BlockLightTable>>,
    sessions: Arc<SessionRegistry>,
    session_id: SessionId,
    /// Reused across all interaction-driven relight computes for
    /// the lifetime of the connection (same amortisation pattern
    /// as `emit_chunks_around`).
    workspace: LightWorkspace,
    /// M9.a: per-chunk computed light, populated during the spawn
    /// burst and mutated in place by [`apply_block_change_to_light`]
    /// on every edit. Replaces the M5-era pattern of recomputing
    /// the full 3×3 neighbourhood for each affected chunk on every
    /// break/place.
    light_cache: LightCache,
    compression: Compression,
    /// M6.d: which item the player is currently holding. Bumped by
    /// `ServerboundSetCarriedItem` (0..=8) and consulted by
    /// `handle_use_item_on` to resolve the placed block.
    selected_hotbar_slot: u8,
    /// M6.e: a 46-slot window-0 inventory. Indices follow vanilla's
    /// numbering: 0..4 crafting (output + 2×2 input), 5..8 armor,
    /// 9..35 main rows, 36..44 hotbar, 45 offhand.
    inventory: PlayerInventory,
    /// M6.e: per-vanilla, the server bumps this counter on every
    /// inventory mutation it ships to the client; the client uses
    /// it to detect desyncs. Starts at 1 (after the seed
    /// ContainerSetContent on login).
    inventory_state_id: i32,
    /// M6.f: hard-coded item→block resolver for the M6 starter
    /// kit. Resolved once from the block registry at construction
    /// time; lookups are an `if`-ladder on the item id.
    item_to_block: ItemToBlockTable,
}

/// 46-slot player inventory (window 0).
///
/// Layout (vanilla wire numbering):
///   0       crafting result
///   1..=4   crafting 2×2 input
///   5..=8   armor (head, chest, legs, feet)
///   9..=35  main inventory rows
///   36..=44 hotbar
///   45      offhand
#[derive(Debug, Clone)]
struct PlayerInventory {
    slots: [ItemStack; 46],
}

impl PlayerInventory {
    /// Slot index where the hotbar begins on the wire.
    const HOTBAR_BASE: usize = 36;

    fn empty() -> Self {
        Self {
            slots: std::array::from_fn(|_| ItemStack::EMPTY),
        }
    }

    fn held(&self, hotbar_slot: u8) -> &ItemStack {
        &self.slots[Self::HOTBAR_BASE + hotbar_slot as usize]
    }

    fn held_mut(&mut self, hotbar_slot: u8) -> &mut ItemStack {
        &mut self.slots[Self::HOTBAR_BASE + hotbar_slot as usize]
    }

    fn as_wire_list(&self) -> Vec<ItemStack> {
        self.slots.to_vec()
    }
}

/// Hard-coded item→default-block-state lookup for the M6 starter
/// kit. Resolved once at connection setup from the block registry.
#[derive(Debug, Clone, Default)]
struct ItemToBlockTable {
    /// `(item_id, BlockStateId)` pairs. Stays tiny — full coverage
    /// is M9+ work.
    entries: Vec<(u32, mc_world::BlockStateId)>,
}

impl ItemToBlockTable {
    fn build(items: &ItemRegistry, blocks: &mc_world::BlockRegistry) -> Self {
        let starter_pairs = &[
            ("minecraft:stone", "minecraft:stone"),
            ("minecraft:dirt", "minecraft:dirt"),
            ("minecraft:oak_planks", "minecraft:oak_planks"),
            ("minecraft:torch", "minecraft:torch"),
        ];
        let mut entries = Vec::with_capacity(starter_pairs.len());
        for (item_name, block_name) in starter_pairs {
            let item_id = match mc_data::Identifier::parse(*item_name) {
                Ok(id) => id,
                Err(_) => continue,
            };
            let block_id = match mc_data::Identifier::parse(*block_name) {
                Ok(id) => id,
                Err(_) => continue,
            };
            let Some(item_pid) = items.id_of(&item_id) else {
                continue;
            };
            let Some(block) = blocks.block(&block_id) else {
                continue;
            };
            entries.push((item_pid, block.default));
        }
        Self { entries }
    }

    fn resolve(&self, item_id: u32) -> Option<mc_world::BlockStateId> {
        self.entries
            .iter()
            .find_map(|(id, state)| (*id == item_id).then_some(*state))
    }
}

/// Build a starter-kit inventory: hotbar 0..=3 = stone, dirt, oak
/// planks, torch; everything else empty. Items the registry doesn't
/// resolve (test stubs without the real `items.json`) are quietly
/// dropped — the slot stays empty.
fn build_starter_inventory(items: &ItemRegistry) -> PlayerInventory {
    let mut inv = PlayerInventory::empty();
    let starter_kit: [(&str, i32); 4] = [
        ("minecraft:stone", 64),
        ("minecraft:dirt", 64),
        ("minecraft:oak_planks", 64),
        ("minecraft:torch", 64),
    ];
    for (i, (name, count)) in starter_kit.iter().enumerate() {
        if let Ok(id) = mc_data::Identifier::parse(*name)
            && let Some(item_id) = items.id_of(&id)
        {
            inv.slots[PlayerInventory::HOTBAR_BASE + i] = ItemStack::new(item_id, *count);
        }
    }
    inv
}

/// `(chunk_x, chunk_z)` for the constant spawn point. Implemented as a
/// fn rather than inlined so the math is unit-testable and so M3.e can
/// share the formula when it computes the view-distance ring.
fn spawn_chunk_pos() -> (i32, i32) {
    chunk_pos_from_coords(SPAWN_X, SPAWN_Z)
}

fn chunk_pos_from_coords(x: f64, z: f64) -> (i32, i32) {
    (
        (x.floor() as i32).div_euclid(16),
        (z.floor() as i32).div_euclid(16),
    )
}

fn desired_chunk_set(center_cx: i32, center_cz: i32, view_distance: i32) -> HashSet<(i32, i32)> {
    spiral_chunks(center_cx, center_cz, view_distance).collect()
}

impl ChunkStreamState {
    #[allow(clippy::too_many_arguments)]
    fn new(
        world: WorldHandle,
        biomes: Arc<Registry>,
        block_light: Option<Arc<BlockLightTable>>,
        compression: Compression,
        sessions: Arc<SessionRegistry>,
        session_id: SessionId,
        center_cx: i32,
        center_cz: i32,
        view_distance: i32,
        policy: ChunkPipelinePolicy,
    ) -> Self {
        let vd = view_distance.max(0);
        let (result_tx, result_rx) = mpsc::channel(policy.chunk_result_queue_size);
        Self {
            world,
            biomes,
            block_light,
            compression,
            sessions,
            session_id,
            io_permits: Arc::new(Semaphore::new(policy.chunk_io_threads)),
            cpu_permits: Arc::new(Semaphore::new(policy.chunk_worker_threads)),
            result_tx,
            result_rx,
            ready: BTreeMap::new(),
            policy,
            result_queue_size: policy.chunk_result_queue_size,
            center_cx,
            center_cz,
            view_distance,
            scheduler: ChunkScheduler::new(prioritized_spiral(center_cx, center_cz, view_distance)),
            staged: HashSet::new(),
            loaded: HashSet::new(),
            started: Instant::now(),
            fetch_ms: 0,
            build_timing: ChunkBuildTiming::default(),
            packet_encode_ms: 0,
            frame_ms: 0,
            socket_write_ms: 0,
            framed_bytes: 0,
            first_chunk_ms: None,
            ring1_complete_ms: None,
            ring2_complete_ms: None,
            ring_emitted: vec![0; (vd + 1) as usize],
            emitted: 0,
            absent: 0,
            bytes: 0,
            dispatch_turns: 0,
            yielded_turns: 0,
            dispatched: 0,
            max_in_flight: 0,
            max_ready: 0,
            last_stop_reason: ChunkPipelineStopReason::QueueEmpty,
            wait_for_first_chunk: true,
        }
    }

    fn is_complete(&self) -> bool {
        self.scheduler.is_complete()
    }

    fn replan_center(&mut self, center_cx: i32, center_cz: i32) -> Vec<(i32, i32)> {
        if (self.center_cx, self.center_cz) == (center_cx, center_cz) {
            return Vec::new();
        }
        let desired = desired_chunk_set(center_cx, center_cz, self.view_distance);
        let unloads: Vec<_> = self.loaded.difference(&desired).copied().collect();
        for chunk in &unloads {
            self.loaded.remove(chunk);
        }
        let mut visibility =
            self.sessions
                .replace_view(self.session_id, (center_cx, center_cz), desired);
        visibility.extend(self.sessions.mark_unloaded(self.session_id, &unloads));
        dispatch_visibility_commands(visibility);
        self.center_cx = center_cx;
        self.center_cz = center_cz;
        self.ready.clear();
        self.scheduler
            .replace_view(prioritized_spiral(center_cx, center_cz, self.view_distance));
        self.reset_window_metrics();
        unloads
    }

    fn reset_window_metrics(&mut self) {
        self.staged.clear();
        self.started = Instant::now();
        self.fetch_ms = 0;
        self.build_timing = ChunkBuildTiming::default();
        self.packet_encode_ms = 0;
        self.frame_ms = 0;
        self.socket_write_ms = 0;
        self.framed_bytes = 0;
        self.first_chunk_ms = None;
        self.ring1_complete_ms = None;
        self.ring2_complete_ms = None;
        self.ring_emitted = vec![0; (self.view_distance.max(0) + 1) as usize];
        self.emitted = 0;
        self.absent = 0;
        self.bytes = 0;
        self.dispatch_turns = 0;
        self.yielded_turns = 0;
        self.dispatched = 0;
        self.max_in_flight = 0;
        self.max_ready = 0;
        self.last_stop_reason = ChunkPipelineStopReason::QueueEmpty;
        self.wait_for_first_chunk = false;
    }

    async fn step<W>(
        &mut self,
        writer: &mut W,
        light_cache: &mut LightCache,
    ) -> Result<ChunkStreamStep, ConnectionError>
    where
        W: AsyncWriteExt + Unpin,
    {
        let wait_for_first_chunk =
            self.wait_for_first_chunk && self.emitted == 0 && self.absent == 0;
        self.dispatch_available();
        self.drain_ready();

        let mut made_send_progress = self.emit_ready_batch(writer, light_cache).await?;
        if !made_send_progress && wait_for_first_chunk {
            while !self.scheduler.is_complete() {
                let Some(result) = self.result_rx.recv().await else {
                    break;
                };
                self.accept_result(result);
                self.drain_ready();
                if self.emit_ready_batch(writer, light_cache).await? {
                    made_send_progress = true;
                    break;
                }
            }
        }
        if made_send_progress || self.emitted > 0 || self.absent > 0 {
            self.wait_for_first_chunk = false;
        }

        if self.scheduler.is_complete() {
            self.last_stop_reason = ChunkPipelineStopReason::Complete;
            return Ok(ChunkStreamStep::Complete);
        }
        if !made_send_progress {
            self.yielded_turns += 1;
        }

        Ok(ChunkStreamStep::Progress)
    }

    async fn emit_ready_batch<W>(
        &mut self,
        writer: &mut W,
        light_cache: &mut LightCache,
    ) -> Result<bool, ConnectionError>
    where
        W: AsyncWriteExt + Unpin,
    {
        let limit = self.policy.chunk_send_rate.max(1) as usize;
        let mut emitted = 0usize;
        while emitted < limit && self.emit_next_ready(writer, light_cache).await? {
            emitted += 1;
        }
        if emitted == limit && !self.ready.is_empty() {
            self.last_stop_reason = ChunkPipelineStopReason::SendBudget;
        }
        Ok(emitted > 0)
    }

    fn dispatch_available(&mut self) {
        self.dispatch_turns += 1;
        let started = Instant::now();
        let mut dispatched_this_turn = 0usize;
        loop {
            if self.scheduler.in_flight_len() >= self.result_queue_size {
                self.last_stop_reason = ChunkPipelineStopReason::QueueFull;
                break;
            }
            if dispatched_this_turn >= self.policy.chunk_prepare_batch_size {
                self.last_stop_reason = ChunkPipelineStopReason::BatchLimit;
                break;
            }
            if self.policy.chunk_prepare_budget_ms > 0
                && started.elapsed().as_millis() as u64 >= self.policy.chunk_prepare_budget_ms
            {
                self.last_stop_reason = ChunkPipelineStopReason::TimeBudget;
                break;
            }
            let Some(request) = self.scheduler.poll_next() else {
                self.last_stop_reason = if self.scheduler.in_flight_len() == 0 {
                    ChunkPipelineStopReason::Complete
                } else {
                    ChunkPipelineStopReason::QueueEmpty
                };
                break;
            };
            if let Some(prepared) = self
                .sessions
                .prepared_chunk((request.chunk_x, request.chunk_z))
            {
                self.accept_result(ChunkPrepareResult {
                    request,
                    fetch_ms: 0,
                    staged: Vec::new(),
                    outcome: ChunkPrepareOutcome::Ready(Box::new((*prepared).clone())),
                });
                dispatched_this_turn += 1;
                self.dispatched += 1;
                continue;
            }
            let world = Arc::clone(&self.world);
            let biomes = Arc::clone(&self.biomes);
            let block_light = self.block_light.as_ref().map(Arc::clone);
            let io_permits = Arc::clone(&self.io_permits);
            let cpu_permits = Arc::clone(&self.cpu_permits);
            let compression = self.compression;
            let tx = self.result_tx.clone();
            tokio::spawn(async move {
                let result = prepare_chunk_request(
                    request,
                    world,
                    biomes,
                    block_light,
                    compression,
                    io_permits,
                    cpu_permits,
                )
                .await;
                let _ = tx.send(result).await;
            });
            dispatched_this_turn += 1;
            self.dispatched += 1;
        }
        self.max_in_flight = self.max_in_flight.max(self.scheduler.in_flight_len());
    }

    fn drain_ready(&mut self) {
        while let Ok(result) = self.result_rx.try_recv() {
            self.accept_result(result);
        }
    }

    fn accept_result(&mut self, result: ChunkPrepareResult) {
        if !self.scheduler.is_current(result.request) {
            return;
        }
        self.ready
            .entry(result.request.priority.sequence)
            .or_insert(result);
        self.max_ready = self.max_ready.max(self.ready.len());
    }

    async fn emit_next_ready<W>(
        &mut self,
        writer: &mut W,
        light_cache: &mut LightCache,
    ) -> Result<bool, ConnectionError>
    where
        W: AsyncWriteExt + Unpin,
    {
        let Some((_, result)) = self.ready.pop_first() else {
            return Ok(false);
        };
        let request = result.request;
        let cx = request.chunk_x;
        let cz = request.chunk_z;
        self.fetch_ms += result.fetch_ms;
        self.staged.extend(result.staged);

        match result.outcome {
            ChunkPrepareOutcome::Ready(prepared) => {
                if let Some(light) = prepared.light.clone() {
                    light_cache.insert(ChunkPos { x: cx, z: cz }, light);
                }
                let mut write_timing = prepared.write_timing;
                write_timing.socket_write_ms = write_framed_chunk(writer, &prepared.frame).await?;
                self.loaded.insert((cx, cz));
                dispatch_visibility_commands(self.sessions.mark_loaded(self.session_id, (cx, cz)));
                self.sessions
                    .cache_prepared_chunk((cx, cz), Arc::new((*prepared).clone()));
                self.build_timing.add(prepared.build_timing);
                self.record_emitted(cx, cz, prepared.packet_data_len, write_timing);
            }
            ChunkPrepareOutcome::Absent => {
                self.absent += 1;
                debug!(cx, cz, "no chunk in storage");
            }
            ChunkPrepareOutcome::Failed(err) => {
                warn!(cx, cz, error = %err, "chunk encode failed; skipping");
            }
        }

        self.scheduler.mark_finished(request);
        Ok(true)
    }

    fn record_emitted(
        &mut self,
        cx: i32,
        cz: i32,
        packet_data_len: usize,
        write_timing: ChunkWriteTiming,
    ) {
        self.packet_encode_ms += write_timing.packet_encode_ms;
        self.frame_ms += write_timing.frame_ms;
        self.socket_write_ms += write_timing.socket_write_ms;
        self.framed_bytes += write_timing.framed_bytes;
        self.emitted += 1;
        self.first_chunk_ms
            .get_or_insert_with(|| self.started.elapsed().as_millis() as u64);
        self.record_ring_progress(cx, cz);
        self.bytes += packet_data_len;
    }

    fn record_ring_progress(&mut self, cx: i32, cz: i32) {
        let ring = (cx - self.center_cx).abs().max((cz - self.center_cz).abs()) as usize;
        if let Some(count) = self.ring_emitted.get_mut(ring) {
            *count += 1;
            let needed = if ring == 0 { 1 } else { ring * 8 };
            if *count == needed {
                let elapsed = self.started.elapsed().as_millis() as u64;
                if ring == 1 {
                    self.ring1_complete_ms.get_or_insert(elapsed);
                } else if ring == 2 {
                    self.ring2_complete_ms.get_or_insert(elapsed);
                }
            }
        }
    }

    fn log_summary(&self) {
        info!(
            center_cx = self.center_cx,
            center_cz = self.center_cz,
            view_distance = self.view_distance,
            staged = self.staged.len(),
            emitted = self.emitted,
            absent = self.absent,
            bytes = self.bytes,
            framed_bytes = self.framed_bytes,
            fetch_ms = self.fetch_ms,
            chunk_data_ms = self.build_timing.chunk_data_ms,
            heightmap_ms = self.build_timing.heightmap_ms,
            light_compute_ms = self.build_timing.light_compute_ms,
            light_encode_ms = self.build_timing.light_encode_ms,
            packet_encode_ms = self.packet_encode_ms,
            frame_ms = self.frame_ms,
            socket_write_ms = self.socket_write_ms,
            chunk_send_rate = self.policy.chunk_send_rate,
            chunk_load_rate = self.policy.chunk_load_rate,
            chunk_generate_rate = self.policy.chunk_generate_rate,
            chunk_prepare_budget_ms = self.policy.chunk_prepare_budget_ms,
            chunk_prepare_batch_size = self.policy.chunk_prepare_batch_size,
            chunk_io_threads = self.policy.chunk_io_threads,
            chunk_worker_threads = self.policy.chunk_worker_threads,
            chunk_result_queue_size = self.policy.chunk_result_queue_size,
            compression_level = ?self.policy.compression_level,
            dispatch_turns = self.dispatch_turns,
            yielded_turns = self.yielded_turns,
            dispatched = self.dispatched,
            in_flight = self.scheduler.in_flight_len(),
            max_in_flight = self.max_in_flight,
            ready = self.ready.len(),
            max_ready = self.max_ready,
            stop_reason = ?self.last_stop_reason,
            first_chunk_ms = self.first_chunk_ms,
            ring1_complete_ms = self.ring1_complete_ms,
            ring2_complete_ms = self.ring2_complete_ms,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            "view-distance window flushed",
        );
    }
}

/// Iterate chunk positions around `(center_x, center_z)` outwards
/// to `view_distance` in chebyshev-ring order. The first cell is the
/// centre; subsequent yields are every cell on ring `r = 1`, then
/// every cell on ring `r = 2`, etc. Within a ring the order is
/// row-major over the bounding square — perceptually this still
/// "spreads" because each ring fills before the next starts.
/// Coverage is identical to a row-major scan: `(2*view_distance +
/// 1)²` cells total, each yielded exactly once.
fn spiral_chunks(
    center_x: i32,
    center_z: i32,
    view_distance: i32,
) -> impl Iterator<Item = (i32, i32)> {
    let vd = view_distance.max(0);
    let mut out = Vec::with_capacity(((2 * vd + 1).pow(2)) as usize);
    out.push((center_x, center_z));
    for r in 1..=vd {
        for dz in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dz.abs()) == r {
                    out.push((center_x + dx, center_z + dz));
                }
            }
        }
    }
    out.into_iter()
}

fn prioritized_spiral(
    center_x: i32,
    center_z: i32,
    view_distance: i32,
) -> impl Iterator<Item = (i32, i32, ChunkPriority)> {
    spiral_chunks(center_x, center_z, view_distance)
        .enumerate()
        .map(move |(sequence, (cx, cz))| {
            (
                cx,
                cz,
                ChunkPriority {
                    ring: (cx - center_x).abs().max((cz - center_z).abs()) as u32,
                    sequence: sequence as u32,
                },
            )
        })
}

async fn prepare_chunk_request(
    request: ChunkRequest,
    world: WorldHandle,
    biomes: Arc<Registry>,
    block_light: Option<Arc<BlockLightTable>>,
    compression: Compression,
    io_permits: Arc<Semaphore>,
    cpu_permits: Arc<Semaphore>,
) -> ChunkPrepareResult {
    let (centre, neighbourhood, staged, fetch_ms) = match load_chunk_neighbourhood(
        Arc::clone(&world),
        request.chunk_x,
        request.chunk_z,
        io_permits,
    )
    .await
    {
        Ok(loaded) => loaded,
        Err(err) => {
            return ChunkPrepareResult {
                request,
                fetch_ms: 0,
                staged: Vec::new(),
                outcome: ChunkPrepareOutcome::Failed(err),
            };
        }
    };

    let Some(centre) = centre else {
        return ChunkPrepareResult {
            request,
            fetch_ms,
            staged,
            outcome: ChunkPrepareOutcome::Absent,
        };
    };

    let cpu_permit = match cpu_permits.acquire_owned().await {
        Ok(permit) => permit,
        Err(_) => {
            return ChunkPrepareResult {
                request,
                fetch_ms,
                staged,
                outcome: ChunkPrepareOutcome::Failed("CPU worker pool closed".into()),
            };
        }
    };

    let outcome = match tokio::task::spawn_blocking(move || {
        let _permit = cpu_permit;
        let built = if let Some(table) = block_light.as_deref() {
            CHUNK_LIGHT_WORKSPACE.with_borrow_mut(|workspace| {
                build_chunk_packet(
                    centre.as_ref(),
                    &neighbourhood,
                    biomes.as_ref(),
                    Some(table),
                    Some(workspace),
                    request.chunk_x,
                    request.chunk_z,
                )
            })
        } else {
            build_chunk_packet(
                centre.as_ref(),
                &neighbourhood,
                biomes.as_ref(),
                None,
                None,
                request.chunk_x,
                request.chunk_z,
            )
        }
        .map_err(|err| err.to_string())?;
        frame_chunk_packet(built, compression).map_err(|err| err.to_string())
    })
    .await
    {
        Ok(Ok(prepared)) => ChunkPrepareOutcome::Ready(Box::new(prepared)),
        Ok(Err(err)) => ChunkPrepareOutcome::Failed(err),
        Err(err) => ChunkPrepareOutcome::Failed(err.to_string()),
    };

    ChunkPrepareResult {
        request,
        fetch_ms,
        staged,
        outcome,
    }
}

type LoadedNeighbourhood = (
    Option<Arc<Chunk>>,
    [[Option<Arc<Chunk>>; 3]; 3],
    Vec<(i32, i32)>,
    u64,
);

async fn load_chunk_neighbourhood(
    world: WorldHandle,
    cx: i32,
    cz: i32,
    io_permits: Arc<Semaphore>,
) -> Result<LoadedNeighbourhood, String> {
    let _permit = io_permits
        .acquire_owned()
        .await
        .map_err(|_| "IO worker pool closed".to_string())?;
    let fetch_started = Instant::now();
    let mut neighbourhood: [[Option<Arc<Chunk>>; 3]; 3] =
        std::array::from_fn(|_| std::array::from_fn(|_| None));
    let mut centre = None;
    let mut staged = Vec::new();

    let mut storage = world.lock().await;
    for (dz, row) in neighbourhood.iter_mut().enumerate() {
        for (dx, slot) in row.iter_mut().enumerate() {
            let ncx = cx + (dx as i32 - 1);
            let ncz = cz + (dz as i32 - 1);
            match storage.get_chunk(ChunkPos { x: ncx, z: ncz }) {
                Ok(Some(chunk)) => {
                    let chunk = Arc::new(chunk.clone());
                    if dx == 1 && dz == 1 {
                        centre = Some(Arc::clone(&chunk));
                    }
                    *slot = Some(chunk);
                    staged.push((ncx, ncz));
                }
                Ok(None) => {}
                Err(err) => warn!(cx = ncx, cz = ncz, error = %err, "chunk read failed; skipping"),
            }
        }
    }

    Ok((
        centre,
        neighbourhood,
        staged,
        fetch_started.elapsed().as_millis() as u64,
    ))
}

struct BuiltChunkPacket {
    packet: LevelChunkWithLight,
    light: Option<ChunkLight>,
    timing: ChunkBuildTiming,
}

#[allow(clippy::too_many_arguments)]
fn build_chunk_packet(
    centre: &Chunk,
    neighbourhood: &[[Option<Arc<Chunk>>; 3]; 3],
    biomes: &Registry,
    block_light: Option<&BlockLightTable>,
    workspace: Option<&mut LightWorkspace>,
    cx: i32,
    cz: i32,
) -> Result<BuiltChunkPacket, mc_world::wire::WireError> {
    let mut timing = ChunkBuildTiming::default();

    let chunk_data_started = Instant::now();
    let data = encode_chunk_data(centre, biomes)?;
    timing.chunk_data_ms = chunk_data_started.elapsed().as_millis() as u64;

    let heightmap_started = Instant::now();
    let heightmaps = client_heightmaps(centre)
        .into_iter()
        .map(|h| ChunkHeightmap {
            type_id: h.type_id,
            data: h.data,
        })
        .collect();
    timing.heightmap_ms = heightmap_started.elapsed().as_millis() as u64;

    let mut computed_light = None;
    let light = match (block_light, workspace) {
        (Some(table), Some(ws)) => {
            // Centre slot is the chunk we already have a reference to;
            // off-centre slots come from the staged map.
            let mut refs: [[Option<&Chunk>; 3]; 3] = [[None; 3]; 3];
            for (dz, row) in neighbourhood.iter().enumerate() {
                for (dx, slot) in row.iter().enumerate() {
                    refs[dz][dx] = slot.as_deref();
                }
            }
            refs[1][1] = Some(centre);

            let light_compute_started = Instant::now();
            let computed = compute_chunk_light_in(ws, refs, table);
            timing.light_compute_ms = light_compute_started.elapsed().as_millis() as u64;

            let light_encode_started = Instant::now();
            let wire = encode_chunk_light(&computed);
            timing.light_encode_ms = light_encode_started.elapsed().as_millis() as u64;
            computed_light = Some(computed);
            LightData {
                sky_y_mask: wire.sky_y_mask,
                block_y_mask: wire.block_y_mask,
                empty_sky_y_mask: wire.empty_sky_y_mask,
                empty_block_y_mask: wire.empty_block_y_mask,
                sky_updates: wire.sky_updates,
                block_updates: wire.block_updates,
            }
        }
        _ => LightData::empty(),
    };
    Ok(BuiltChunkPacket {
        packet: LevelChunkWithLight {
            chunk_x: cx,
            chunk_z: cz,
            heightmaps,
            data,
            block_entities: Vec::new(),
            light,
        },
        light: computed_light,
        timing,
    })
}

fn frame_chunk_packet(
    built: BuiltChunkPacket,
    compression: Compression,
) -> Result<PreparedChunkFrame, ConnectionError> {
    let mut timing = ChunkWriteTiming::default();

    let packet_encode_started = Instant::now();
    let mut body = BytesMut::new();
    built.packet.encode(&mut body)?;
    timing.packet_encode_ms = packet_encode_started.elapsed().as_millis() as u64;
    let packet_data_len = built.packet.data.len();

    let frame_started = Instant::now();
    let framed = encode_frame(LevelChunkWithLight::ID, &body, compression)?;
    timing.frame_ms = frame_started.elapsed().as_millis() as u64;
    timing.framed_bytes = framed.len();

    Ok(PreparedChunkFrame {
        frame: framed,
        light: built.light,
        packet_data_len,
        build_timing: built.timing,
        write_timing: timing,
    })
}

async fn write_framed_chunk<W>(writer: &mut W, framed: &[u8]) -> Result<u64, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let socket_write_started = Instant::now();
    writer.write_all(framed).await?;
    Ok(socket_write_started.elapsed().as_millis() as u64)
}

/// M5.d: handle a serverbound `PlayerAction`. Acts on
/// `START_DESTROY_BLOCK` / `STOP_DESTROY_BLOCK` (creative-style
/// instant break — survival mining timing arrives with a future
/// inventory milestone) by setting the target cell to air,
/// recomputing light over the affected 1+4 chunks, and emitting
/// the resulting wire packets back to the client. Every action
/// regardless of kind gets an ack so the client's prediction
/// state doesn't hang.
async fn handle_player_action<W>(
    state: &mut InteractionState,
    writer: &mut W,
    action: ServerboundPlayerAction,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let is_destroy = matches!(
        action.action,
        PlayerActionKind::StartDestroyBlock | PlayerActionKind::StopDestroyBlock
    );
    if !is_destroy {
        // DROP_*, RELEASE_USE_ITEM, SWAP_ITEM_WITH_OFFHAND, STAB —
        // all out of scope for M5. Ack so the client doesn't hang
        // on a prediction.
        debug!(
            action = ?action.action,
            sequence = action.sequence,
            "non-destroy player action ignored"
        );
        write_packet(
            writer,
            &BlockChangedAck {
                sequence: action.sequence,
            },
            state.compression,
        )
        .await?;
        return Ok(());
    }

    let (x, y, z) = unpack_block_pos(action.position);
    let air = air_state_id(&state.blocks);

    dispatch_visibility_commands(state.sessions.broadcast_player_animation(state.session_id));
    apply_block_edit(state, writer, action.sequence, x, y, z, air).await
}

async fn send_block_deltas<W>(
    writer: &mut W,
    compression: Compression,
    deltas: &[BlockDelta],
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    for packet in plan_block_delta_packets(deltas) {
        match packet {
            BlockDeltaPacket::Single(delta) => {
                write_packet(
                    writer,
                    &BlockUpdate {
                        position: mc_protocol::packets::play::pack_block_pos(
                            delta.x, delta.y, delta.z,
                        ),
                        state_id: delta.state_id.0 as i32,
                    },
                    compression,
                )
                .await?;
            }
            BlockDeltaPacket::Section {
                section_x,
                section_y,
                section_z,
                changes,
            } => {
                write_packet(
                    writer,
                    &SectionBlocksUpdate {
                        section_pos: pack_section_pos(section_x, section_y, section_z),
                        changes: changes
                            .into_iter()
                            .map(|delta| SectionBlockChange {
                                relative_pos: pack_section_relative_pos(delta.x, delta.y, delta.z),
                                state_id: delta.state_id.0 as i32,
                            })
                            .collect(),
                    },
                    compression,
                )
                .await?;
            }
        }
    }
    Ok(())
}

async fn send_light_updates<W>(
    state: &mut InteractionState,
    writer: &mut W,
    updates: &[OutboundLightUpdate],
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    for update in updates {
        state.light_cache.insert(update.pos, update.light.clone());
        write_packet(
            writer,
            &LightUpdate {
                chunk_x: update.pos.x,
                chunk_z: update.pos.z,
                light: update.wire.clone(),
            },
            state.compression,
        )
        .await?;
    }
    Ok(())
}

fn broadcast_block_deltas(
    state: &InteractionState,
    chunks: &HashSet<(i32, i32)>,
    deltas: &[BlockDelta],
) {
    if deltas.is_empty() || chunks.is_empty() {
        return;
    }
    for recipient in state
        .sessions
        .loaded_recipients_for_chunks(chunks, state.session_id)
    {
        if let Err(err) = recipient
            .tx
            .try_send(OutboundCommand::BlockDeltas(deltas.to_vec()))
        {
            debug!(
                recipient = recipient.id,
                error = %err,
                "dropping subscriber block delta"
            );
        }
    }
}

fn broadcast_light_updates(state: &InteractionState, updates: &[OutboundLightUpdate]) {
    if updates.is_empty() {
        return;
    }
    let chunks: HashSet<_> = updates
        .iter()
        .map(|update| (update.pos.x, update.pos.z))
        .collect();
    for recipient in state
        .sessions
        .loaded_recipients_for_chunks(&chunks, state.session_id)
    {
        if let Err(err) = recipient
            .tx
            .try_send(OutboundCommand::LightUpdates(updates.to_vec()))
        {
            debug!(
                recipient = recipient.id,
                error = %err,
                "dropping subscriber light update"
            );
        }
    }
}

fn plan_block_delta_packets(deltas: &[BlockDelta]) -> Vec<BlockDeltaPacket> {
    if deltas.len() <= 1 {
        return deltas
            .iter()
            .copied()
            .map(BlockDeltaPacket::Single)
            .collect();
    }

    let mut by_section: BTreeMap<(i32, i32, i32), Vec<BlockDelta>> = BTreeMap::new();
    for &delta in deltas {
        by_section
            .entry((
                delta.x.div_euclid(16),
                delta.y.div_euclid(16),
                delta.z.div_euclid(16),
            ))
            .or_default()
            .push(delta);
    }

    let mut packets = Vec::new();
    for ((section_x, section_y, section_z), changes) in by_section {
        if changes.len() == 1 {
            packets.push(BlockDeltaPacket::Single(changes[0]));
        } else {
            packets.push(BlockDeltaPacket::Section {
                section_x,
                section_y,
                section_z,
                changes,
            });
        }
    }
    packets
}

/// Shared back-half of the break / place flows: mutates the world
/// at `(x, y, z)` to `new_state`, broadcasts the block delta +
/// recomputed `LightUpdate`s + `BlockChangedAck` to the connected
/// client. The actual edit is applied via
/// `WorldStorage::set_block_at` so heightmap recompute lands in
/// the same atomic step.
async fn apply_block_edit<W>(
    state: &mut InteractionState,
    writer: &mut W,
    sequence: i32,
    x: i32,
    y: i32,
    z: i32,
    new_state: mc_world::BlockStateId,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let pos = mc_world::BlockPos { x, y, z };
    let table = state.block_light.as_ref().map(Arc::clone);

    // 1. Apply the mutation. Drop the lock as soon as it's done so
    //    the light-recompute path can re-acquire it for the
    //    neighbourhood read without deadlocking.
    let prev = {
        let mut storage = state.world.lock().await;
        match storage.set_block_at(pos, new_state) {
            Ok(p) => {
                if let (Some(prev), Some(table)) = (p, table.as_deref())
                    && prev != new_state
                    && let Err(err) = storage.update_highest_opaque_at(pos, table)
                {
                    warn!(error = %err, x, y, z, "highest-opaque heightmap update failed");
                }
                p
            }
            Err(err) => {
                warn!(error = %err, x, y, z, "set_block_at failed; skipping edit");
                write_packet(writer, &BlockChangedAck { sequence }, state.compression).await?;
                return Ok(());
            }
        }
    };

    // Absent chunk or no-op edit: ack the sequence so the client
    // doesn't roll back forever, but skip the BlockUpdate /
    // LightUpdate ripple.
    let Some(prev) = prev else {
        write_packet(writer, &BlockChangedAck { sequence }, state.compression).await?;
        return Ok(());
    };
    if prev == new_state {
        write_packet(writer, &BlockChangedAck { sequence }, state.compression).await?;
        return Ok(());
    }

    let block_delta = BlockDelta {
        x,
        y,
        z,
        state_id: new_state,
    };
    let edit_chunk = (x.div_euclid(16), z.div_euclid(16));
    let edit_chunks = HashSet::from([edit_chunk]);
    state.sessions.invalidate_prepared_chunks(&edit_chunks);

    // 2. Tell subscribed clients about the new block. Single edits stay on the
    //    historical BlockUpdate wire shape; only true batches use the
    //    section packet helper.
    send_block_deltas(writer, state.compression, &[block_delta]).await?;
    broadcast_block_deltas(state, &edit_chunks, &[block_delta]);

    // 3. Incremental relight (M9). Update cached per-chunk light in
    //    place via bounded BFS, then emit one `LightUpdate` per
    //    chunk whose stored arrays actually changed. Falls back to
    //    no-op if the cache is empty (e.g. edits before the spawn
    //    burst populated it) or the block-light table isn't loaded.
    if let Some(table) = table {
        let light_updates = send_incremental_relight(
            state,
            writer,
            &table,
            edit_chunk.0,
            edit_chunk.1,
            x,
            y,
            z,
            prev,
            new_state,
        )
        .await?;
        let light_chunks: HashSet<_> = light_updates
            .iter()
            .map(|update| (update.pos.x, update.pos.z))
            .collect();
        state.sessions.invalidate_prepared_chunks(&light_chunks);
        broadcast_light_updates(state, &light_updates);
    }

    // 4. Ack last — vanilla expects update-before-ack so the
    //    prediction reconciles against a known state.
    write_packet(writer, &BlockChangedAck { sequence }, state.compression).await?;
    Ok(())
}

/// M9: incremental relight. Pulls the post-edit 3×3 chunk
/// neighbourhood out of storage once, runs a bounded BFS that
/// mutates the per-chunk cached light in place, and emits one
/// `LightUpdate` per chunk whose stored arrays changed.
///
/// Falls back to a single-chunk full recompute when the cache
/// hasn't been pre-warmed (e.g. an edit lands before the spawn
/// burst's `build_chunk_packet` got to that chunk) — same coverage
/// as the old `send_relight_around` for the centre tile, but
/// without the 5× cost.
#[allow(clippy::too_many_arguments)]
async fn send_incremental_relight<W>(
    state: &mut InteractionState,
    writer: &mut W,
    table: &BlockLightTable,
    cx: i32,
    cz: i32,
    edit_x: i32,
    edit_y: i32,
    edit_z: i32,
    prev_state: mc_world::BlockStateId,
    new_state: mc_world::BlockStateId,
) -> Result<Vec<OutboundLightUpdate>, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    // 1. Pull the 3×3 chunks around the edit out of storage. The
    //    edit has already been applied, so these are post-edit.
    let mut chunks: HashMap<(i32, i32), Arc<Chunk>> = HashMap::new();
    {
        let mut storage = state.world.lock().await;
        for dcz in -1i32..=1 {
            for dcx in -1i32..=1 {
                let pos = ChunkPos {
                    x: cx + dcx,
                    z: cz + dcz,
                };
                match storage.get_chunk(pos) {
                    Ok(Some(c)) => {
                        chunks.insert((cx + dcx, cz + dcz), Arc::new(c.clone()));
                    }
                    Ok(None) => {}
                    Err(err) => {
                        warn!(error = %err, cx = pos.x, cz = pos.z, "relight neighbour read failed");
                    }
                }
            }
        }
    }

    let centre_pos = ChunkPos { x: cx, z: cz };

    // 2. If the edit chunk isn't in the cache yet (rare: edit before
    //    spawn burst reached it), seed it via a single full compute.
    if !state.light_cache.contains(centre_pos) {
        let mut refs: [[Option<&Chunk>; 3]; 3] = [[None; 3]; 3];
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                refs[(dz + 1) as usize][(dx + 1) as usize] =
                    chunks.get(&(cx + dx, cz + dz)).map(|a| a.as_ref());
            }
        }
        let Some(centre) = refs[1][1] else {
            return Ok(Vec::new()); // edit chunk vanished from storage — nothing to relight
        };
        let _ = centre;
        let light = compute_chunk_light_in(&mut state.workspace, refs, table);
        state.light_cache.insert(centre_pos, light);
    }

    // 3. Build the 3×3 reference array for the incremental update.
    let mut refs: [[Option<&Chunk>; 3]; 3] = [[None; 3]; 3];
    for dz in -1i32..=1 {
        for dx in -1i32..=1 {
            refs[(dz + 1) as usize][(dx + 1) as usize] =
                chunks.get(&(cx + dx, cz + dz)).map(|a| a.as_ref());
        }
    }

    let local_x = edit_x.rem_euclid(16) as u8;
    let local_z = edit_z.rem_euclid(16) as u8;

    let touched = apply_block_change_to_light(
        &mut state.light_cache,
        &refs,
        table,
        centre_pos,
        local_x,
        edit_y,
        local_z,
        prev_state,
        new_state,
    );

    // 4. Emit one LightUpdate per chunk whose cached light changed.
    let mut updates = Vec::new();
    for pos in touched {
        let Some(light) = state.light_cache.get(pos) else {
            continue;
        };
        let wire = encode_chunk_light(light);
        let light_data = LightData {
            sky_y_mask: wire.sky_y_mask,
            block_y_mask: wire.block_y_mask,
            empty_sky_y_mask: wire.empty_sky_y_mask,
            empty_block_y_mask: wire.empty_block_y_mask,
            sky_updates: wire.sky_updates,
            block_updates: wire.block_updates,
        };
        updates.push(OutboundLightUpdate {
            pos,
            light: light.clone(),
            wire: light_data.clone(),
        });
        write_packet(
            writer,
            &LightUpdate {
                chunk_x: pos.x,
                chunk_z: pos.z,
                light: light_data,
            },
            state.compression,
        )
        .await?;
    }
    Ok(updates)
}

fn air_state_id(registry: &mc_world::BlockRegistry) -> mc_world::BlockStateId {
    let air_id = mc_data::Identifier::parse("minecraft:air").expect("static identifier");
    registry
        .block(&air_id)
        .map(|b| b.default)
        .unwrap_or(mc_world::BlockStateId(0))
}

/// M6.f: handle a serverbound `UseItemOn`. Resolves the placed
/// block via the player's currently-held hotbar slot through the
/// item→block table. Drops the placement silently (still acking) if
/// the held stack is empty, if the held item has no block mapping
/// (e.g. food, tool), or if the target cell is non-air. On a
/// successful placement decrements the held stack's count and emits
/// `ContainerSetSlot` so the client sees the new count.
async fn handle_use_item_on<W>(
    state: &mut InteractionState,
    writer: &mut W,
    action: ServerboundUseItemOn,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let (cx, cy, cz) = unpack_block_pos(action.position);
    let (dx, dy, dz) = action.direction.normal();
    let (tx, ty, tz) = (cx + dx, cy + dy, cz + dz);

    let air = air_state_id(&state.blocks);

    // M6.f: resolve the placed block from the held item.
    let held_slot = state.selected_hotbar_slot;
    let held = state.inventory.held(held_slot).clone();
    let placed_state = if held.is_empty() {
        None
    } else {
        state.item_to_block.resolve(held.item_id)
    };
    let Some(placed_state) = placed_state else {
        debug!(
            sequence = action.sequence,
            held_item = held.item_id,
            held_count = held.count,
            "UseItemOn: held item is empty or not placeable; skipping"
        );
        write_packet(
            writer,
            &BlockChangedAck {
                sequence: action.sequence,
            },
            state.compression,
        )
        .await?;
        return Ok(());
    };

    // Validate: target cell must currently be air. We can borrow
    // the world briefly to peek; if the cell is non-air or absent,
    // skip the edit but still ack.
    let target_is_air = {
        let mut storage = state.world.lock().await;
        match storage.get_block(mc_world::BlockPos {
            x: tx,
            y: ty,
            z: tz,
        }) {
            Ok(Some(current)) => current == air,
            Ok(None) => false,
            Err(err) => {
                warn!(error = %err, x = tx, y = ty, z = tz, "UseItemOn target read failed");
                false
            }
        }
    };
    if !target_is_air {
        debug!(
            x = tx,
            y = ty,
            z = tz,
            "UseItemOn target not air; skipping placement"
        );
        write_packet(
            writer,
            &BlockChangedAck {
                sequence: action.sequence,
            },
            state.compression,
        )
        .await?;
        return Ok(());
    }

    apply_block_edit(state, writer, action.sequence, tx, ty, tz, placed_state).await?;
    dispatch_visibility_commands(state.sessions.broadcast_player_animation(state.session_id));

    // M6.f: decrement the held stack's count + tell the client the
    // new slot contents. Empty stacks ship as `count == 0`.
    {
        let held = state.inventory.held_mut(held_slot);
        held.count = held.count.saturating_sub(1);
        if held.count <= 0 {
            *held = ItemStack::EMPTY;
        }
    }
    state.inventory_state_id = state.inventory_state_id.wrapping_add(1);
    let new_slot_value = state.inventory.held(held_slot).clone();
    write_packet(
        writer,
        &ClientboundContainerSetSlot {
            container_id: 0,
            state_id: state.inventory_state_id,
            slot: (PlayerInventory::HOTBAR_BASE + held_slot as usize) as i16,
            item_stack: new_slot_value,
        },
        state.compression,
    )
    .await?;
    Ok(())
}

async fn replan_after_movement<W>(
    writer: &mut W,
    compression: Compression,
    chunk_stream: &mut Option<ChunkStreamState>,
    interaction: Option<&mut InteractionState>,
    old_center: (i32, i32),
    new_center: (i32, i32),
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if old_center == new_center {
        return Ok(());
    }
    write_packet(
        writer,
        &SetCenterChunk {
            chunk_x: new_center.0,
            chunk_z: new_center.1,
        },
        compression,
    )
    .await?;
    if let Some(stream) = chunk_stream.as_mut() {
        let unloads = stream.replan_center(new_center.0, new_center.1);
        let mut interaction = interaction;
        for (chunk_x, chunk_z) in unloads {
            if let Some(state) = interaction.as_deref_mut() {
                state.light_cache.remove(ChunkPos {
                    x: chunk_x,
                    z: chunk_z,
                });
            }
            write_packet(writer, &ForgetLevelChunk { chunk_x, chunk_z }, compression).await?;
        }
    }
    debug!(
        old_cx = old_center.0,
        old_cz = old_center.1,
        new_cx = new_center.0,
        new_cz = new_center.1,
        "chunk view center updated from movement"
    );
    Ok(())
}

fn player_info_entry(player: &PlayerEntitySnapshot) -> PlayerInfoEntry {
    PlayerInfoEntry {
        profile_id: player.uuid,
        name: player.name.clone(),
        listed: true,
        latency: 0,
        game_mode: 0,
        list_order: player.entity_id,
        show_hat: true,
    }
}

fn player_position(player: &PlayerEntitySnapshot) -> PositionMoveRotation {
    PositionMoveRotation {
        position: EntityVec3 {
            x: player.pose.x,
            y: player.pose.y,
            z: player.pose.z,
        },
        delta_movement: EntityVec3::ZERO,
        yaw: player.pose.yaw,
        pitch: player.pose.pitch,
    }
}

fn entity_position(entity: &ServerEntitySnapshot) -> PositionMoveRotation {
    PositionMoveRotation {
        position: EntityVec3 {
            x: entity.position.x,
            y: entity.position.y,
            z: entity.position.z,
        },
        delta_movement: EntityVec3 {
            x: entity.velocity.x,
            y: entity.velocity.y,
            z: entity.velocity.z,
        },
        yaw: entity.rotation.yaw,
        pitch: entity.rotation.pitch,
    }
}

async fn send_player_spawn<W>(
    writer: &mut W,
    compression: Compression,
    player: &PlayerEntitySnapshot,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    debug!(
        target_session = player.session_id,
        entity_id = player.entity_id,
        player = %player.name,
        "spawning visible player"
    );
    write_packet(
        writer,
        &PlayerInfoUpdate {
            actions: PlayerInfoActions::minimal_add_player(),
            entries: vec![player_info_entry(player)],
        },
        compression,
    )
    .await?;
    write_packet(
        writer,
        &AddEntity {
            entity_id: player.entity_id,
            uuid: player.uuid,
            entity_type_id: PLAYER_ENTITY_TYPE_ID,
            x: player.pose.x,
            y: player.pose.y,
            z: player.pose.z,
            movement: EntityVec3::ZERO,
            pitch: player.pose.pitch,
            yaw: player.pose.yaw,
            head_yaw: player.pose.yaw,
            data: 0,
        },
        compression,
    )
    .await?;
    send_player_move(writer, compression, player).await
}

async fn send_player_move<W>(
    writer: &mut W,
    compression: Compression,
    player: &PlayerEntitySnapshot,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &EntityPositionSync {
            entity_id: player.entity_id,
            values: player_position(player),
            on_ground: player.pose.flags.on_ground,
        },
        compression,
    )
    .await?;
    write_packet(
        writer,
        &RotateHead {
            entity_id: player.entity_id,
            head_yaw: player.pose.yaw,
        },
        compression,
    )
    .await?;
    Ok(())
}

async fn send_player_despawn<W>(
    writer: &mut W,
    compression: Compression,
    player: &PlayerEntitySnapshot,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    debug!(
        target_session = player.session_id,
        entity_id = player.entity_id,
        player = %player.name,
        "despawning visible player"
    );
    write_packet(
        writer,
        &RemoveEntities {
            entity_ids: vec![player.entity_id],
        },
        compression,
    )
    .await?;
    write_packet(
        writer,
        &PlayerInfoRemove {
            profile_ids: vec![player.uuid],
        },
        compression,
    )
    .await?;
    Ok(())
}

async fn send_entity_spawn<W>(
    writer: &mut W,
    compression: Compression,
    entity: &ServerEntitySnapshot,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    debug!(
        entity_id = entity.id.0,
        entity_type = %entity.type_name,
        "spawning visible server entity"
    );
    write_packet(
        writer,
        &AddEntity {
            entity_id: entity.id.0,
            uuid: entity.uuid,
            entity_type_id: entity.type_id,
            x: entity.position.x,
            y: entity.position.y,
            z: entity.position.z,
            movement: EntityVec3 {
                x: entity.velocity.x,
                y: entity.velocity.y,
                z: entity.velocity.z,
            },
            pitch: entity.rotation.pitch,
            yaw: entity.rotation.yaw,
            head_yaw: entity.rotation.head_yaw,
            data: 0,
        },
        compression,
    )
    .await?;
    send_entity_move(writer, compression, entity).await
}

async fn send_entity_move<W>(
    writer: &mut W,
    compression: Compression,
    entity: &ServerEntitySnapshot,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &EntityPositionSync {
            entity_id: entity.id.0,
            values: entity_position(entity),
            on_ground: entity.on_ground,
        },
        compression,
    )
    .await?;
    write_packet(
        writer,
        &SetEntityMotion {
            entity_id: entity.id.0,
            movement: EntityVec3 {
                x: entity.velocity.x,
                y: entity.velocity.y,
                z: entity.velocity.z,
            },
        },
        compression,
    )
    .await?;
    write_packet(
        writer,
        &RotateHead {
            entity_id: entity.id.0,
            head_yaw: entity.rotation.head_yaw,
        },
        compression,
    )
    .await?;
    Ok(())
}

async fn send_entity_relative_move<W>(
    writer: &mut W,
    compression: Compression,
    movement: &ServerEntityMove,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &MoveEntityPosRot {
            entity_id: movement.id.0,
            delta_x: MoveEntityPosRot::delta_to_short(movement.delta.x),
            delta_y: MoveEntityPosRot::delta_to_short(movement.delta.y),
            delta_z: MoveEntityPosRot::delta_to_short(movement.delta.z),
            yaw: MoveEntityPosRot::pack_degrees(movement.rotation.yaw),
            pitch: MoveEntityPosRot::pack_degrees(movement.rotation.pitch),
            on_ground: movement.on_ground,
        },
        compression,
    )
    .await?;
    write_packet(
        writer,
        &RotateHead {
            entity_id: movement.id.0,
            head_yaw: movement.rotation.head_yaw,
        },
        compression,
    )
    .await
}

async fn send_entity_despawn<W>(
    writer: &mut W,
    compression: Compression,
    entity: &ServerEntitySnapshot,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    debug!(
        entity_id = entity.id.0,
        entity_type = %entity.type_name,
        "despawning visible server entity"
    );
    write_packet(
        writer,
        &RemoveEntities {
            entity_ids: vec![entity.id.0],
        },
        compression,
    )
    .await
}

async fn send_player_animation<W>(
    writer: &mut W,
    compression: Compression,
    entity_id: i32,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &EntityAnimation {
            entity_id,
            action: EntityAnimationAction::SwingMainHand,
        },
        compression,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn play_loop<R, W>(
    reader: &mut R,
    writer: &mut W,
    buf: &mut BytesMut,
    compression: Compression,
    mut interaction: Option<&mut InteractionState>,
    mut chunk_stream: Option<ChunkStreamState>,
    sessions: Arc<SessionRegistry>,
    session_id: SessionId,
    mut player_pose: PlayerPose,
    mut outbound_rx: mpsc::Receiver<OutboundCommand>,
) -> Result<(), ConnectionError>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let mut ticker = interval(KEEPALIVE_PERIOD);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // The first `tick()` resolves immediately; drop it so we don't
    // race-send a keepalive before the client has processed initial
    // Play packets and the first chunk.
    ticker.tick().await;

    let mut next_id: i64 = 0;
    let mut last_response_at = Instant::now();
    let mut pending_id: Option<i64> = None;

    loop {
        let mut stream_finished = false;
        if let (Some(stream), Some(state)) = (chunk_stream.as_mut(), interaction.as_deref_mut())
            && !stream.is_complete()
        {
            match stream.step(writer, &mut state.light_cache).await? {
                ChunkStreamStep::Progress => {
                    stream_finished = stream.is_complete();
                    tokio::task::yield_now().await;
                }
                ChunkStreamStep::Complete => {
                    stream_finished = true;
                }
            }
        }
        if stream_finished {
            if let Some(stream) = chunk_stream.as_ref() {
                stream.log_summary();
            }
            last_response_at = Instant::now();
            pending_id = None;
        }

        tokio::select! {
            biased;
            command = outbound_rx.recv() => {
                match command {
                    Some(OutboundCommand::BlockDeltas(deltas)) => {
                        send_block_deltas(writer, compression, &deltas).await?;
                    }
                    Some(OutboundCommand::LightUpdates(updates)) => {
                        if let Some(state) = interaction.as_deref_mut() {
                            send_light_updates(state, writer, &updates).await?;
                        }
                    }
                    Some(OutboundCommand::SpawnPlayer(player)) => {
                        send_player_spawn(writer, compression, &player).await?;
                    }
                    Some(OutboundCommand::MovePlayer(player)) => {
                        send_player_move(writer, compression, &player).await?;
                    }
                    Some(OutboundCommand::DespawnPlayer(player)) => {
                        send_player_despawn(writer, compression, &player).await?;
                    }
                    Some(OutboundCommand::SpawnEntity(entity)) => {
                        send_entity_spawn(writer, compression, &entity).await?;
                    }
                    Some(OutboundCommand::MoveEntityRelative(movement)) => {
                        send_entity_relative_move(writer, compression, &movement).await?;
                    }
                    Some(OutboundCommand::DespawnEntity(entity)) => {
                        send_entity_despawn(writer, compression, &entity).await?;
                    }
                    Some(OutboundCommand::AnimatePlayer { entity_id }) => {
                        send_player_animation(writer, compression, entity_id).await?;
                    }
                    None => {}
                }
            }
            _ = ticker.tick(), if chunk_stream.as_ref().is_none_or(ChunkStreamState::is_complete) => {
                if last_response_at.elapsed() > KEEPALIVE_TIMEOUT {
                    warn!(
                        elapsed_ms = last_response_at.elapsed().as_millis() as u64,
                        "client missed keepalive deadline; closing"
                    );
                    return Ok(());
                }
                next_id = next_id.wrapping_add(1).max(1);
                pending_id = Some(next_id);
                write_packet(
                    writer,
                    &ClientboundKeepAlive { id: next_id },
                    compression,
                )
                .await?;
            }
            result = read_frame(reader, buf, compression) => {
                let frame = result?;
                if frame.id == ServerboundKeepAlive::ID {
                    let mut body = frame.body;
                    let echo = ServerboundKeepAlive::decode(&mut body)?;
                    if pending_id == Some(echo.id) {
                        last_response_at = Instant::now();
                        pending_id = None;
                    } else {
                        warn!(
                            expected = ?pending_id,
                            received = echo.id,
                            "keepalive id mismatch"
                        );
                    }
                } else if frame.id == ConfirmTeleportation::ID {
                    let mut body = frame.body;
                    let confirm = ConfirmTeleportation::decode(&mut body)?;
                    debug!(teleport_id = confirm.teleport_id, "teleport confirmed");
                } else if frame.id == ServerboundMovePlayerPos::ID {
                    let mut body = frame.body;
                    let movement = ServerboundMovePlayerPos::decode(&mut body)?;
                    let old_center = player_pose.chunk_pos();
                    player_pose.x = movement.x;
                    player_pose.y = movement.y;
                    player_pose.z = movement.z;
                    player_pose.flags = movement.flags;
                    let new_center = player_pose.chunk_pos();
                    dispatch_visibility_commands(sessions.update_pose(session_id, player_pose));
                    replan_after_movement(writer, compression, &mut chunk_stream, interaction.as_deref_mut(), old_center, new_center).await?;
                } else if frame.id == ServerboundMovePlayerPosRot::ID {
                    let mut body = frame.body;
                    let movement = ServerboundMovePlayerPosRot::decode(&mut body)?;
                    let old_center = player_pose.chunk_pos();
                    player_pose.x = movement.x;
                    player_pose.y = movement.y;
                    player_pose.z = movement.z;
                    player_pose.yaw = movement.yaw;
                    player_pose.pitch = movement.pitch;
                    player_pose.flags = movement.flags;
                    let new_center = player_pose.chunk_pos();
                    dispatch_visibility_commands(sessions.update_pose(session_id, player_pose));
                    replan_after_movement(writer, compression, &mut chunk_stream, interaction.as_deref_mut(), old_center, new_center).await?;
                } else if frame.id == ServerboundMovePlayerRot::ID {
                    let mut body = frame.body;
                    let movement = ServerboundMovePlayerRot::decode(&mut body)?;
                    player_pose.yaw = movement.yaw;
                    player_pose.pitch = movement.pitch;
                    player_pose.flags = movement.flags;
                    dispatch_visibility_commands(sessions.update_pose(session_id, player_pose));
                } else if frame.id == ServerboundMovePlayerStatusOnly::ID {
                    let mut body = frame.body;
                    let movement = ServerboundMovePlayerStatusOnly::decode(&mut body)?;
                    player_pose.flags = movement.flags;
                    dispatch_visibility_commands(sessions.update_pose(session_id, player_pose));
                } else if frame.id == ServerboundPlayerAction::ID {
                    let mut body = frame.body;
                    let action = ServerboundPlayerAction::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        handle_player_action(state, writer, action).await?;
                    } else {
                        debug!(
                            action = ?action.action,
                            sequence = action.sequence,
                            "PlayerAction ignored — no world configured"
                        );
                    }
                } else if frame.id == ServerboundUseItemOn::ID {
                    let mut body = frame.body;
                    let use_on = ServerboundUseItemOn::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        handle_use_item_on(state, writer, use_on).await?;
                    } else {
                        debug!(
                            sequence = use_on.sequence,
                            "UseItemOn ignored — no world configured"
                        );
                    }
                } else if frame.id == ServerboundSetCarriedItem::ID {
                    let mut body = frame.body;
                    let pick = ServerboundSetCarriedItem::decode(&mut body)?;
                    let slot = pick.slot.clamp(0, 8) as u8;
                    if let Some(state) = interaction.as_deref_mut() {
                        state.selected_hotbar_slot = slot;
                        debug!(slot, "hotbar selection updated");
                    }
                } else {
                    debug!(
                        id = format!("{:#04x}", frame.id),
                        "play packet ignored"
                    );
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(1)), if chunk_stream.as_ref().is_some_and(|stream| !stream.is_complete()) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_tick_cadence_matches_vanilla_cow_tracking() {
        assert_eq!(ENTITY_TICK_PERIOD, Duration::from_millis(50));
        assert_eq!(mc_physics::TICK_SECONDS, 0.05);
        assert_eq!(ENTITY_MOVE_SEND_INTERVAL_TICKS, 3);
    }

    #[test]
    fn pack_block_pos_round_trip() {
        // The packed-i64 representation is bit-exact what vanilla wants.
        // Just confirm the formula does not panic and that nominal
        // origin packs to 0.
        assert_eq!(pack_block_pos(0, 0, 0), 0);
        assert_ne!(pack_block_pos(1, 0, 0), 0);
        assert_ne!(pack_block_pos(0, 1, 0), 0);
        assert_ne!(pack_block_pos(0, 0, 1), 0);
    }

    #[test]
    fn spawn_chunk_pos_matches_origin() {
        // SPAWN_(X,Z) = (0.5, 0.5); the containing chunk is (0, 0).
        assert_eq!(spawn_chunk_pos(), (0, 0));
    }

    #[test]
    fn chunk_pos_from_coords_uses_floor_division() {
        assert_eq!(chunk_pos_from_coords(0.0, 0.0), (0, 0));
        assert_eq!(chunk_pos_from_coords(15.999, 15.999), (0, 0));
        assert_eq!(chunk_pos_from_coords(16.0, -0.001), (1, -1));
        assert_eq!(chunk_pos_from_coords(-0.001, -16.0), (-1, -1));
        assert_eq!(chunk_pos_from_coords(-16.001, 32.0), (-2, 2));
    }

    #[test]
    fn session_registry_drops_prepared_cache_with_last_ticket() {
        let registry = SessionRegistry::new();
        let (tx, _rx) = mpsc::channel(1);
        let profile = LoggedInProfile {
            uuid: uuid::Uuid::nil(),
            name: "tester".to_string(),
        };
        let (id, _) = registry.register(
            &profile,
            (0, 0),
            0,
            HashSet::from([(0, 0)]),
            tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );
        registry.cache_prepared_chunk(
            (0, 0),
            Arc::new(PreparedChunkFrame {
                frame: Bytes::from_static(b"chunk-frame"),
                light: None,
                packet_data_len: 0,
                build_timing: ChunkBuildTiming::default(),
                write_timing: ChunkWriteTiming::default(),
            }),
        );
        assert!(registry.prepared_chunk((0, 0)).is_some());

        let _ = registry.unregister(id);

        assert!(registry.prepared_chunk((0, 0)).is_none());
    }

    #[test]
    fn session_registry_spawns_and_despawns_visible_players() {
        let registry = SessionRegistry::new();
        let (alice_tx, _alice_rx) = mpsc::channel(8);
        let (bob_tx, _bob_rx) = mpsc::channel(8);
        let alice = LoggedInProfile {
            uuid: uuid::Uuid::from_u128(1),
            name: "Alice".to_string(),
        };
        let bob = LoggedInProfile {
            uuid: uuid::Uuid::from_u128(2),
            name: "Bob".to_string(),
        };

        let (alice_id, _) = registry.register(
            &alice,
            (0, 0),
            0,
            HashSet::from([(0, 0)]),
            alice_tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );
        assert!(registry.mark_loaded(alice_id, (0, 0)).is_empty());

        let (bob_id, dispatches) = registry.register(
            &bob,
            (0, 0),
            0,
            HashSet::from([(0, 0)]),
            bob_tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );
        assert!(dispatches.iter().any(|dispatch| {
            dispatch.recipient.id == alice_id
                && matches!(
                    &dispatch.command,
                    OutboundCommand::SpawnPlayer(player)
                        if player.session_id == bob_id && player.name == "Bob"
                )
        }));

        let dispatches = registry.mark_loaded(bob_id, (0, 0));
        assert!(dispatches.iter().any(|dispatch| {
            dispatch.recipient.id == bob_id
                && matches!(
                    &dispatch.command,
                    OutboundCommand::SpawnPlayer(player)
                        if player.session_id == alice_id && player.name == "Alice"
                )
        }));

        let dispatches = registry.update_pose(
            bob_id,
            PlayerPose {
                x: 48.5,
                y: DEFAULT_SPAWN_Y,
                z: 0.5,
                yaw: 0.0,
                pitch: 0.0,
                flags: MovePlayerFlags::new(true, false),
            },
        );
        assert!(dispatches.iter().any(|dispatch| {
            dispatch.recipient.id == alice_id
                && matches!(
                    &dispatch.command,
                    OutboundCommand::DespawnPlayer(player) if player.session_id == bob_id
                )
        }));
    }

    #[test]
    fn session_registry_unregister_removes_visible_player() {
        let registry = SessionRegistry::new();
        let (alice_tx, _alice_rx) = mpsc::channel(8);
        let (bob_tx, _bob_rx) = mpsc::channel(8);
        let alice = LoggedInProfile {
            uuid: uuid::Uuid::from_u128(1),
            name: "Alice".to_string(),
        };
        let bob = LoggedInProfile {
            uuid: uuid::Uuid::from_u128(2),
            name: "Bob".to_string(),
        };

        let (alice_id, _) = registry.register(
            &alice,
            (0, 0),
            0,
            HashSet::from([(0, 0)]),
            alice_tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );
        let _ = registry.mark_loaded(alice_id, (0, 0));
        let (bob_id, _) = registry.register(
            &bob,
            (0, 0),
            0,
            HashSet::from([(0, 0)]),
            bob_tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );

        let dispatches = registry.unregister(bob_id);
        assert!(dispatches.iter().any(|dispatch| {
            dispatch.recipient.id == alice_id
                && matches!(
                    &dispatch.command,
                    OutboundCommand::DespawnPlayer(player) if player.session_id == bob_id
                )
        }));
    }

    #[test]
    fn spiral_chunks_starts_at_centre() {
        let mut iter = spiral_chunks(3, -7, 2);
        assert_eq!(iter.next(), Some((3, -7)));
    }

    #[test]
    fn spiral_chunks_covers_every_cell_in_window() {
        for vd in 0..=4 {
            let collected: std::collections::HashSet<(i32, i32)> =
                spiral_chunks(0, 0, vd).collect();
            let expected_count = ((2 * vd + 1) as usize).pow(2);
            assert_eq!(
                collected.len(),
                expected_count,
                "vd={vd}: spiral should yield {expected_count} unique cells",
            );
            for dz in -vd..=vd {
                for dx in -vd..=vd {
                    assert!(
                        collected.contains(&(dx, dz)),
                        "vd={vd}: missing cell ({dx},{dz})"
                    );
                }
            }
        }
    }

    #[test]
    fn spiral_chunks_ring_order_monotonic() {
        // Within the iteration, the chebyshev distance must be
        // non-decreasing. That's the property that makes the
        // perceptual spread feel like a spiral rather than a scan.
        let mut last_ring = -1i32;
        for (dx, dz) in spiral_chunks(0, 0, 3) {
            let r = dx.abs().max(dz.abs());
            assert!(
                r >= last_ring,
                "non-monotonic ring sequence at cell ({dx},{dz}): r={r} < last={last_ring}"
            );
            last_ring = r;
        }
    }

    #[test]
    fn spawn_dimension_prefers_alphabetical_first() {
        let data = mc_data::testing::stub();
        let (id, name, all) = spawn_dimension(&data).unwrap();
        assert_eq!(id, 0);
        assert_eq!(name.as_str(), "minecraft:alpha");
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn block_delta_plan_keeps_single_edit_as_block_update() {
        let delta = BlockDelta {
            x: 1,
            y: -60,
            z: 2,
            state_id: mc_world::BlockStateId(1),
        };

        assert_eq!(
            plan_block_delta_packets(&[delta]),
            vec![BlockDeltaPacket::Single(delta)]
        );
    }

    #[test]
    fn block_delta_plan_groups_multiple_changes_in_same_section() {
        let first = BlockDelta {
            x: -1,
            y: -60,
            z: 2,
            state_id: mc_world::BlockStateId(1),
        };
        let second = BlockDelta {
            x: -2,
            y: -61,
            z: 3,
            state_id: mc_world::BlockStateId(2),
        };

        assert_eq!(
            plan_block_delta_packets(&[first, second]),
            vec![BlockDeltaPacket::Section {
                section_x: -1,
                section_y: -4,
                section_z: 0,
                changes: vec![first, second],
            }]
        );
    }

    #[test]
    fn block_delta_plan_does_not_section_pack_singletons() {
        let first = BlockDelta {
            x: 0,
            y: 0,
            z: 0,
            state_id: mc_world::BlockStateId(1),
        };
        let second = BlockDelta {
            x: 16,
            y: 0,
            z: 0,
            state_id: mc_world::BlockStateId(2),
        };

        assert_eq!(
            plan_block_delta_packets(&[first, second]),
            vec![
                BlockDeltaPacket::Single(first),
                BlockDeltaPacket::Single(second)
            ]
        );
    }
}
