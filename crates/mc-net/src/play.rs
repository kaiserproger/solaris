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
use mc_data::entity_types::EntityTypeRegistry;
use mc_data::items::ItemRegistry;
use mc_data::tags::TagsData;
use mc_data::{Registry, VanillaData};
use mc_entity::{
    EntityId, EntityItemStack, EntityLifecycle, EntityStore, GoalState, SpawnEntity, Vec3,
};
use mc_nbt::Tag;
use mc_protocol::codec::Identifier;
use mc_protocol::frame::{Compression, encode_frame};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, BlockChangedAck, BlockUpdate, ChunkHeightmap, ClientCommandAction,
    ClientboundContainerSetContent, ClientboundContainerSetData, ClientboundContainerSetSlot,
    ClientboundKeepAlive, ClientboundOpenScreen, ClientboundPlayerAbilities, ClientboundRespawn,
    ClientboundSetEntityData, ClientboundSetHealth, ClientboundSetHeldSlot,
    ClientboundTakeItemEntity, ConfirmTeleportation, ContainerInput, Direction, EntityAnimation,
    EntityAnimationAction, EntityDataValue, EntityPositionSync, EntityVec3, ForgetLevelChunk,
    GameEvent, GameMode, ITEM_ENTITY_DATA_ITEM_INDEX, ItemStack, LevelChunkWithLight, LightData,
    LightUpdate, LoginPlay, MoveEntityPosRot, MovePlayerFlags, PlayerActionKind, PlayerInfoActions,
    PlayerInfoEntry, PlayerInfoRemove, PlayerInfoUpdate, PositionMoveRotation, RemoveEntities,
    RotateHead, SectionBlockChange, SectionBlocksUpdate, ServerboundAttack,
    ServerboundChangeGameMode, ServerboundChatCommand, ServerboundClientCommand,
    ServerboundContainerClick, ServerboundContainerClose, ServerboundInteract,
    ServerboundKeepAlive, ServerboundMovePlayerPos, ServerboundMovePlayerPosRot,
    ServerboundMovePlayerRot, ServerboundMovePlayerStatusOnly, ServerboundPlaceRecipe,
    ServerboundPlayerAction, ServerboundRecipeBookChangeSettings, ServerboundRecipeBookSeenRecipe,
    ServerboundSetCarriedItem, ServerboundUseItem, ServerboundUseItemOn, SetCenterChunk,
    SetEntityMotion, SynchronizePlayerPosition, pack_section_pos, pack_section_relative_pos,
    unpack_block_pos,
};
use mc_world::light::{
    ChunkLight, LightCache, LightWorkspace, apply_block_change_to_light, compute_chunk_light_in,
};
use mc_world::wire::{client_heightmaps, encode_chunk_data, encode_chunk_light};
use mc_world::{BlockRegistry, BlockStateId, Chunk, ChunkPos, FurnaceBlockEntity, FurnaceSlot};
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
const DEFAULT_SEA_LEVEL: i32 = 63;
const PLAYER_ENTITY_TYPE_ID: i32 = 155;
const SERVER_ENTITY_ID_START: i32 = 1_000_000;
pub(crate) const ENTITY_TICK_PERIOD: Duration = Duration::from_millis(50);
const ENTITY_MOVE_SEND_INTERVAL_TICKS: u64 = 1;
const SURVIVAL_MINING_FALLBACK_TIME: Duration = Duration::from_millis(200);
const CRAFTING_MENU_TYPE_ID: i32 = 12;
const CRAFTING_MENU_SLOT_COUNT: usize = 46;
const FURNACE_MENU_TYPE_ID: i32 = 14;
const FURNACE_CONTAINER_ID_MIN: i32 = 1;
const FURNACE_CONTAINER_ID_MAX: i32 = 100;
const FURNACE_MENU_SLOT_COUNT: usize = 39;
const FURNACE_FUEL_TICKS: i16 = 1600;
const DEFAULT_FURNACE_COOK_TICKS: i16 = 200;
const DEFAULT_FOOD_USE_DURATION: Duration = Duration::from_millis(1_600);

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
    UpdateEntityData(ServerEntitySnapshot),
    MoveEntityRelative(ServerEntityMove),
    TakeItemEntity {
        item_entity_id: i32,
        player_entity_id: i32,
        amount: i32,
    },
    DespawnEntity(ServerEntitySnapshot),
    AnimatePlayer {
        entity_id: i32,
    },
    FurnaceSlots {
        position: mc_world::BlockPos,
        slots: [ItemStack; 3],
    },
    FurnaceData {
        position: mc_world::BlockPos,
        changed: Vec<(i16, i16)>,
    },
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
    item_stack: Option<EntityItemStack>,
}

#[derive(Debug, Clone)]
struct ServerEntityMove {
    id: EntityId,
    delta: Vec3,
    rotation: mc_entity::Rotation,
    on_ground: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct HerdSpawn {
    chunk: (i32, i32),
    slot: u8,
    entity_type_id: i32,
    entity_type_name: String,
    position: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CommandPermissions {
    op: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct SurvivalState {
    health: f32,
    food: i32,
    saturation: f32,
    exhaustion: f32,
}

impl SurvivalState {
    const MAX_HEALTH: f32 = 20.0;
    const MAX_FOOD: i32 = 20;
    const EXHAUSTION_STEP: f32 = 4.0;
    const HEALTH_TICK_PERIOD: u32 = 4;

    const FULL: Self = Self {
        health: Self::MAX_HEALTH,
        food: Self::MAX_FOOD,
        saturation: 5.0,
        exhaustion: 0.0,
    };

    const fn as_packet(self) -> ClientboundSetHealth {
        ClientboundSetHealth {
            health: self.health,
            food: self.food,
            saturation: self.saturation,
        }
    }

    fn apply_damage(&mut self, amount: f32) {
        self.health = (self.health - amount.max(0.0)).clamp(0.0, Self::MAX_HEALTH);
    }

    fn heal(&mut self, amount: f32) {
        self.health = (self.health + amount.max(0.0)).clamp(0.0, Self::MAX_HEALTH);
    }

    fn is_dead(self) -> bool {
        self.health <= 0.0
    }

    fn add_food(&mut self, food: i32, saturation: f32) {
        self.food = (self.food + food).clamp(0, Self::MAX_FOOD);
        self.saturation = (self.saturation + saturation.max(0.0)).clamp(0.0, self.food as f32);
    }

    fn add_exhaustion(&mut self, amount: f32) {
        self.exhaustion = (self.exhaustion + amount.max(0.0)).max(0.0);
        while self.exhaustion >= Self::EXHAUSTION_STEP {
            self.exhaustion -= Self::EXHAUSTION_STEP;
            if self.saturation > 0.0 {
                self.saturation = (self.saturation - 1.0).max(0.0);
            } else if self.food > 0 {
                self.food -= 1;
            }
        }
    }

    fn tick_health(&mut self, tick: u32) -> bool {
        if self.is_dead() || !tick.is_multiple_of(Self::HEALTH_TICK_PERIOD) {
            return false;
        }
        let before = *self;
        if self.food >= 18 && self.health < Self::MAX_HEALTH {
            self.heal(1.0);
            self.add_exhaustion(6.0);
        } else if self.food == 0 {
            self.apply_damage(1.0);
        }
        *self != before
    }
}

impl CommandPermissions {
    fn for_local_dev_profile(_profile: &LoggedInProfile) -> Self {
        Self { op: true }
    }

    const fn can_change_game_mode(self) -> bool {
        self.op
    }
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

#[derive(Debug, Clone)]
struct FurnaceViewer {
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
    spawned_entity_chunks: HashSet<(i32, i32)>,
    furnace_viewers: HashMap<mc_world::BlockPos, HashMap<SessionId, FurnaceViewer>>,
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
            spawned_entity_chunks: HashSet::new(),
            furnace_viewers: HashMap::new(),
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
        for viewers in inner.furnace_viewers.values_mut() {
            viewers.remove(&id);
        }
        inner
            .furnace_viewers
            .retain(|_, viewers| !viewers.is_empty());
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

    fn register_furnace_viewer(&self, id: SessionId, position: mc_world::BlockPos) {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        let Some(tx) = inner.sessions.get(&id).map(|session| session.tx.clone()) else {
            return;
        };
        inner
            .furnace_viewers
            .entry(position)
            .or_default()
            .insert(id, FurnaceViewer { tx });
    }

    fn unregister_furnace_viewer(&self, id: SessionId, position: mc_world::BlockPos) {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        if let Some(viewers) = inner.furnace_viewers.get_mut(&position) {
            viewers.remove(&id);
            if viewers.is_empty() {
                inner.furnace_viewers.remove(&position);
            }
        }
    }

    fn furnace_slot_dispatches(
        &self,
        position: mc_world::BlockPos,
        except: SessionId,
        slots: [ItemStack; 3],
    ) -> Vec<VisibilityDispatch> {
        self.furnace_dispatches(
            position,
            except,
            OutboundCommand::FurnaceSlots { position, slots },
        )
    }

    fn furnace_data_dispatches(
        &self,
        position: mc_world::BlockPos,
        except: SessionId,
        changed: Vec<(i16, i16)>,
    ) -> Vec<VisibilityDispatch> {
        self.furnace_dispatches(
            position,
            except,
            OutboundCommand::FurnaceData { position, changed },
        )
    }

    fn furnace_dispatches(
        &self,
        position: mc_world::BlockPos,
        except: SessionId,
        command: OutboundCommand,
    ) -> Vec<VisibilityDispatch> {
        let inner = self.inner.lock().expect("session registry poisoned");
        inner
            .furnace_viewers
            .get(&position)
            .into_iter()
            .flat_map(|viewers| viewers.iter())
            .filter(|&(&id, _)| id != except)
            .map(|(&id, viewer)| VisibilityDispatch {
                recipient: SessionRecipient {
                    id,
                    tx: viewer.tx.clone(),
                },
                command: command.clone(),
            })
            .collect()
    }

    fn is_furnace_tick_owner(&self, position: mc_world::BlockPos, id: SessionId) -> bool {
        let inner = self.inner.lock().expect("session registry poisoned");
        inner
            .furnace_viewers
            .get(&position)
            .and_then(|viewers| viewers.keys().min().copied())
            == Some(id)
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

    fn ensure_chunk_herd(
        &self,
        chunk: (i32, i32),
        spawns: &[HerdSpawn],
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        if !inner.spawned_entity_chunks.insert(chunk) {
            return Vec::new();
        }
        for spawn in spawns {
            debug_assert_eq!(spawn.chunk, chunk);
            let mut entity = SpawnEntity::new(
                spawn.entity_type_id,
                spawn.entity_type_name.clone(),
                spawn.position,
            );
            entity.uuid = Some(herd_uuid(spawn.chunk, spawn.slot));
            entity.goal = GoalState::Wander {
                speed: 0.8,
                period_ticks: 80,
            };
            let id = inner.entities.spawn(entity);
            inner.last_sent_entity_positions.insert(id, spawn.position);
        }
        debug!(
            cx = chunk.0,
            cz = chunk.1,
            entities = spawns.len(),
            "spawned passive entity herd"
        );
        refresh_visibility_locked(&mut inner)
    }

    fn spawn_item_drop(
        &self,
        entity_type_id: i32,
        position: Vec3,
        stack: EntityItemStack,
    ) -> Vec<VisibilityDispatch> {
        if stack.is_empty() {
            return Vec::new();
        }
        let mut inner = self.inner.lock().expect("session registry poisoned");
        let mut entity = SpawnEntity::new(entity_type_id, "minecraft:item", position);
        entity.item_stack = Some(stack);
        entity.velocity = Vec3::new(0.0, 0.1, 0.0);
        let id = inner.entities.spawn(entity);
        inner.last_sent_entity_positions.insert(id, position);
        refresh_visibility_locked(&mut inner)
    }

    fn nearby_item_entities(&self, position: Vec3, radius: f64) -> Vec<ServerEntitySnapshot> {
        let radius_sq = radius * radius;
        let inner = self.inner.lock().expect("session registry poisoned");
        inner
            .entities
            .snapshots()
            .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
            .filter(|entity| entity.item_stack.is_some())
            .filter(|entity| distance_sq(entity.position, position) <= radius_sq)
            .map(server_entity_snapshot_from)
            .collect()
    }

    fn server_entity_snapshot(&self, entity_id: EntityId) -> Option<ServerEntitySnapshot> {
        let inner = self.inner.lock().expect("session registry poisoned");
        inner
            .entities
            .snapshot(entity_id)
            .map(server_entity_snapshot_from)
    }

    fn damage_server_entity(
        &self,
        entity_id: EntityId,
        amount: f32,
    ) -> Option<mc_entity::EntityDamage> {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        inner.entities.damage(entity_id, amount)
    }

    fn update_item_stack(
        &self,
        entity_id: EntityId,
        stack: EntityItemStack,
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        if !inner.entities.set_item_stack(entity_id, Some(stack)) {
            return Vec::new();
        }
        let Some(snapshot) = inner
            .entities
            .snapshot(entity_id)
            .map(server_entity_snapshot_from)
        else {
            return Vec::new();
        };
        visible_entity_observers_locked(&inner, entity_id)
            .into_iter()
            .filter_map(|observer_id| {
                let observer = inner.sessions.get(&observer_id)?;
                Some(VisibilityDispatch {
                    recipient: SessionRecipient {
                        id: observer_id,
                        tx: observer.tx.clone(),
                    },
                    command: OutboundCommand::UpdateEntityData(snapshot.clone()),
                })
            })
            .collect()
    }

    fn remove_server_entity(
        &self,
        entity_id: EntityId,
    ) -> Option<(ServerEntitySnapshot, Vec<VisibilityDispatch>)> {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        let snapshot = inner
            .entities
            .remove(entity_id)
            .map(server_entity_snapshot_from)?;
        inner.last_sent_entity_positions.remove(&entity_id);

        let mut dispatches = Vec::new();
        for (&observer_id, observer) in &mut inner.sessions {
            if observer.visible_entities.remove(&entity_id) {
                dispatches.push(VisibilityDispatch {
                    recipient: SessionRecipient {
                        id: observer_id,
                        tx: observer.tx.clone(),
                    },
                    command: OutboundCommand::DespawnEntity(snapshot.clone()),
                });
            }
        }
        Some((snapshot, dispatches))
    }

    fn remove_picked_item(
        &self,
        entity_id: EntityId,
        collector_session: SessionId,
        amount: i32,
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        let Some(snapshot) = inner
            .entities
            .remove(entity_id)
            .map(server_entity_snapshot_from)
        else {
            return Vec::new();
        };
        inner.last_sent_entity_positions.remove(&entity_id);
        let collector_entity_id = inner
            .sessions
            .get(&collector_session)
            .map(|session| session.entity_id)
            .unwrap_or_default();
        let mut dispatches = Vec::new();
        for (&observer_id, observer) in &mut inner.sessions {
            if observer.visible_entities.remove(&entity_id) {
                let recipient = SessionRecipient {
                    id: observer_id,
                    tx: observer.tx.clone(),
                };
                dispatches.push(VisibilityDispatch {
                    recipient: recipient.clone(),
                    command: OutboundCommand::TakeItemEntity {
                        item_entity_id: entity_id.0,
                        player_entity_id: collector_entity_id,
                        amount,
                    },
                });
                dispatches.push(VisibilityDispatch {
                    recipient,
                    command: OutboundCommand::DespawnEntity(snapshot.clone()),
                });
            }
        }
        dispatches
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
        item_stack: entity.item_stack,
    }
}

fn server_entity_chunk_pos(entity: &ServerEntitySnapshot) -> (i32, i32) {
    chunk_pos_from_coords(entity.position.x, entity.position.z)
}

fn distance_sq(a: Vec3, b: Vec3) -> f64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    let dz = a.z - b.z;
    dx * dx + dy * dy + dz * dz
}

fn player_eye_position(pose: PlayerPose) -> Vec3 {
    Vec3::new(pose.x, pose.y + 1.62, pose.z)
}

fn block_center(position: i64) -> Vec3 {
    let (x, y, z) = unpack_block_pos(position);
    Vec3::new(x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5)
}

fn within_block_reach(pose: PlayerPose, position: i64, game_mode: GameMode) -> bool {
    let max = if game_mode == GameMode::Creative {
        6.0
    } else {
        5.0
    };
    distance_sq(player_eye_position(pose), block_center(position)) <= max * max
}

fn within_entity_reach(pose: PlayerPose, position: Vec3, game_mode: GameMode) -> bool {
    let max = if game_mode == GameMode::Creative {
        6.0
    } else {
        4.0
    };
    distance_sq(player_eye_position(pose), position) <= max * max
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
    passive_herd_surface: Option<mc_world::BlockStateId>,
    passive_herd_water: Option<mc_world::BlockStateId>,
    passive_herd_passable: Arc<Vec<BlockStateId>>,
    passive_spawn_rules: Arc<mc_data::biomes::BiomeSpawnRules>,
    entity_types: Arc<mc_data::entity_types::EntityTypeRegistry>,
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
    herd_spawns: Vec<HerdSpawn>,
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
    let mut storage = world.lock().await;
    let chunk = storage.get_chunk_mut(ChunkPos { x: 0, z: 0 }).ok()??;
    spawn_y_from_chunk(chunk, config.block_light.as_deref())
}

fn spawn_y_from_chunk(chunk: &mut Chunk, table: Option<&BlockLightTable>) -> Option<f64> {
    if let Some(top) = chunk.highest_opaque_y(0, 0) {
        return Some((top + 2) as f64);
    }
    let table = table?;
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
        max_players: config.max_players.min(i32::MAX as u32) as i32,
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
        is_flat: false,
        death_location: None,
        portal_cooldown: 0,
        sea_level: DEFAULT_SEA_LEVEL,
        enforces_secure_chat: false,
    };
    let respawn = ClientboundRespawn {
        dimension_type_id: login.dimension_type_id,
        dimension_name: login.dimension_name.clone(),
        hashed_seed: login.hashed_seed,
        game_mode: login.game_mode,
        previous_game_mode: login.previous_game_mode,
        is_debug: login.is_debug,
        is_flat: login.is_flat,
        death_location: None,
        portal_cooldown: login.portal_cooldown,
        sea_level: login.sea_level,
        data_to_keep: 0,
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

    let passive_herd_surface = mc_data::Identifier::parse("minecraft:grass_block")
        .ok()
        .and_then(|id| config.blocks.block(&id).map(|block| block.default));
    let passive_herd_water = mc_data::Identifier::parse("minecraft:water")
        .ok()
        .and_then(|id| config.blocks.block(&id).map(|block| block.default));
    let passive_herd_passable = Arc::new(passive_entity_passable_blocks(&config.blocks));
    let mut light_cache = LightCache::new();
    let mut chunk_stream = config.world.as_ref().and_then(|world| {
        let biomes = data.registry("worldgen/biome")?;
        Some(ChunkStreamState::new(
            Arc::clone(world),
            Arc::new(biomes.clone()),
            config.block_light.as_ref().map(Arc::clone),
            passive_herd_surface,
            passive_herd_water,
            Arc::clone(&passive_herd_passable),
            Arc::clone(&config.biome_spawns),
            Arc::clone(&config.entity_types),
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

        // 6. Seed an empty server-authoritative player inventory. Test
        //    and dev-only inventory mutation goes through explicit
        //    debug commands; normal survival no longer gets a starter kit.
        let initial_inventory = PlayerInventory::empty();
        write_packet(writer, &ClientboundSetHeldSlot { slot: 0 }, compression).await?;
        write_packet(
            writer,
            &ClientboundContainerSetContent {
                container_id: 0,
                state_id: 1,
                items: initial_inventory.as_wire_list(),
                carried_item: ItemStack::EMPTY,
            },
            compression,
        )
        .await?;

        let mut recipes = match mc_data::recipes::load_recipes(
            config.data.root().join("data/minecraft/recipe"),
        ) {
            Ok(recipes) => recipes,
            Err(err) => {
                warn!(error = %err, "recipe data load failed; crafting disabled");
                Vec::new()
            }
        };
        if recipes.is_empty() {
            recipes = fallback_crafting_recipes();
        }

        // 7. Play loop. Runs until the connection drops or the client
        //    misses a heartbeat by more than `KEEPALIVE_TIMEOUT`. The
        //    interaction state passes the M5.d/M5.e/M6.f break/place
        //    handlers everything they need to mutate the world and emit
        //    relight + container packets back to the client.
        let mut interaction = config.world.as_ref().map(|world| InteractionState {
            world: Arc::clone(world),
            blocks: Arc::clone(&config.blocks),
            block_light: config.block_light.as_ref().map(Arc::clone),
            water: passive_herd_water,
            sessions: Arc::clone(&sessions),
            session_id,
            workspace: LightWorkspace::new(),
            light_cache: std::mem::take(&mut light_cache),
            compression,
            selected_hotbar_slot: 0,
            inventory: initial_inventory,
            carried_item: ItemStack::EMPTY,
            inventory_state_id: 1,
            items: Arc::clone(&config.items),
            entity_types: Arc::clone(&config.entity_types),
            item_to_block: ItemToBlockTable::build(&config.items, &config.blocks),
            tags: Arc::clone(&config.tags),
            recipes,
            next_container_id: FURNACE_CONTAINER_ID_MIN,
            active_container: None,
            pending_break: None,
            pending_use: None,
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
            initial_pose,
            respawn,
            CommandPermissions::for_local_dev_profile(profile),
            SurvivalState::FULL,
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
    water: Option<mc_world::BlockStateId>,
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
    /// Server-authoritative cursor stack for vanilla container clicks.
    carried_item: ItemStack,
    /// M6.e: per-vanilla, the server bumps this counter on every
    /// inventory mutation it ships to the client; the client uses
    /// it to detect desyncs. Starts at 1 (after the seed
    /// ContainerSetContent on login).
    inventory_state_id: i32,
    items: Arc<ItemRegistry>,
    entity_types: Arc<EntityTypeRegistry>,
    /// Registry-derived item→default-block resolver. Built once from
    /// vanilla item/block registries at construction time.
    item_to_block: ItemToBlockTable,
    tags: Arc<TagsData>,
    recipes: Vec<mc_data::recipes::Recipe>,
    next_container_id: i32,
    active_container: Option<ActiveContainer>,
    pending_break: Option<PendingBreak>,
    pending_use: Option<PendingUse>,
}

#[derive(Debug, Clone)]
enum ActiveContainer {
    CraftingTable(CraftingTableWindow),
    Furnace(FurnaceWindow),
}

impl ActiveContainer {
    fn container_id(&self) -> i32 {
        match self {
            Self::CraftingTable(window) => window.container_id,
            Self::Furnace(window) => window.container_id,
        }
    }
}

#[derive(Debug, Clone)]
struct CraftingTableWindow {
    container_id: i32,
    state_id: i32,
    input: [ItemStack; 9],
    result: ItemStack,
}

impl CraftingTableWindow {
    fn new(container_id: i32) -> Self {
        Self {
            container_id,
            state_id: 1,
            input: std::array::from_fn(|_| ItemStack::EMPTY),
            result: ItemStack::EMPTY,
        }
    }
}

#[derive(Debug, Clone)]
struct FurnaceWindow {
    container_id: i32,
    position: mc_world::BlockPos,
    state_id: i32,
}

impl FurnaceWindow {
    fn new(position: mc_world::BlockPos, container_id: i32) -> Self {
        Self {
            container_id,
            position,
            state_id: 1,
        }
    }
}

#[derive(Debug, Clone)]
struct PendingBreak {
    position: i64,
    direction: Direction,
    started_at: Instant,
    required_time: Duration,
    held_hotbar_slot: u8,
    held_item_id: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
struct PendingUse {
    started_at: Instant,
    required_time: Duration,
    held_hotbar_slot: u8,
    held_item_id: u32,
    rule: FoodRule,
}

#[derive(Debug, Clone, Copy)]
struct MiningRule {
    block_path_contains: &'static [&'static str],
    base_time: Duration,
    tool_suffix: Option<&'static str>,
}

const FALLBACK_MINING_RULES: &[MiningRule] = &[
    MiningRule {
        block_path_contains: &["stone", "ore", "deepslate", "brick"],
        base_time: Duration::from_millis(1_500),
        tool_suffix: Some("_pickaxe"),
    },
    MiningRule {
        block_path_contains: &["log", "wood", "planks"],
        base_time: Duration::from_millis(900),
        tool_suffix: Some("_axe"),
    },
    MiningRule {
        block_path_contains: &["dirt", "sand", "gravel", "clay", "snow"],
        base_time: Duration::from_millis(600),
        tool_suffix: Some("_shovel"),
    },
    MiningRule {
        block_path_contains: &["leaves", "grass", "flower"],
        base_time: SURVIVAL_MINING_FALLBACK_TIME,
        tool_suffix: None,
    },
];

const UNKNOWN_BLOCK_MINING_RULE: MiningRule = MiningRule {
    block_path_contains: &[],
    base_time: Duration::from_millis(800),
    tool_suffix: None,
};

#[derive(Debug, Clone, Copy, PartialEq)]
struct FoodRule {
    item: &'static str,
    food: i32,
    saturation: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ArmorStats {
    armor: f32,
    toughness: f32,
}

const FALLBACK_FOOD_RULES: &[FoodRule] = &[
    FoodRule {
        item: "minecraft:apple",
        food: 4,
        saturation: 2.4,
    },
    FoodRule {
        item: "minecraft:bread",
        food: 5,
        saturation: 6.0,
    },
];

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

    fn set_hotbar(&mut self, hotbar_slot: u8, stack: ItemStack) {
        self.slots[Self::HOTBAR_BASE + hotbar_slot as usize] = stack;
    }

    fn merge_stack(&mut self, mut stack: ItemStack) -> (ItemStack, Vec<(usize, ItemStack)>) {
        let mut changed = Vec::new();
        if stack.is_empty() {
            return (ItemStack::EMPTY, changed);
        }

        for slot in 9..=44 {
            let current = &mut self.slots[slot];
            if current.is_empty()
                || current.item_id != stack.item_id
                || current.damage != stack.damage
                || current.count >= 64
            {
                continue;
            }
            let moved = (64 - current.count).min(stack.count);
            current.count += moved;
            stack.count -= moved;
            changed.push((slot, current.clone()));
            if stack.count <= 0 {
                return (ItemStack::EMPTY, changed);
            }
        }

        for slot in 9..=44 {
            if !self.slots[slot].is_empty() {
                continue;
            }
            let moved = stack.count.min(64);
            let mut moved_stack = stack.clone();
            moved_stack.count = moved;
            self.slots[slot] = moved_stack;
            stack.count -= moved;
            changed.push((slot, self.slots[slot].clone()));
            if stack.count <= 0 {
                return (ItemStack::EMPTY, changed);
            }
        }

        (stack, changed)
    }

    fn merge_pickup_stack(&mut self, mut stack: ItemStack) -> (ItemStack, Vec<(usize, ItemStack)>) {
        let mut changed = Vec::new();
        if stack.is_empty() {
            return (ItemStack::EMPTY, changed);
        }

        for slot in 9..=44 {
            let current = &mut self.slots[slot];
            if current.is_empty()
                || current.item_id != stack.item_id
                || current.damage != stack.damage
                || current.count >= 64
            {
                continue;
            }
            let moved = (64 - current.count).min(stack.count);
            current.count += moved;
            stack.count -= moved;
            changed.push((slot, current.clone()));
            if stack.count <= 0 {
                return (ItemStack::EMPTY, changed);
            }
        }

        for slot in 36..=44 {
            if !self.slots[slot].is_empty() {
                continue;
            }
            let moved = stack.count.min(64);
            let mut moved_stack = stack.clone();
            moved_stack.count = moved;
            self.slots[slot] = moved_stack;
            stack.count -= moved;
            changed.push((slot, self.slots[slot].clone()));
            if stack.count <= 0 {
                return (ItemStack::EMPTY, changed);
            }
        }

        for slot in 9..=35 {
            if !self.slots[slot].is_empty() {
                continue;
            }
            let moved = stack.count.min(64);
            let mut moved_stack = stack.clone();
            moved_stack.count = moved;
            self.slots[slot] = moved_stack;
            stack.count -= moved;
            changed.push((slot, self.slots[slot].clone()));
            if stack.count <= 0 {
                return (ItemStack::EMPTY, changed);
            }
        }

        (stack, changed)
    }

    fn as_wire_list(&self) -> Vec<ItemStack> {
        self.slots.to_vec()
    }

    fn merge_stack_into_ranges(
        &mut self,
        mut stack: ItemStack,
        ranges: &[std::ops::RangeInclusive<usize>],
        max_stack: i32,
    ) -> ItemStack {
        if stack.is_empty() {
            return ItemStack::EMPTY;
        }

        for range in ranges {
            for slot in range.clone() {
                let current = &mut self.slots[slot];
                if !can_stack(current, &stack) || current.count >= max_stack {
                    continue;
                }
                let moved = (max_stack - current.count).min(stack.count);
                current.count += moved;
                stack.count -= moved;
                if stack.count <= 0 {
                    return ItemStack::EMPTY;
                }
            }
        }

        for range in ranges {
            for slot in range.clone() {
                if !self.slots[slot].is_empty() {
                    continue;
                }
                let moved = stack.count.min(max_stack);
                let mut moved_stack = stack.clone();
                moved_stack.count = moved;
                self.slots[slot] = moved_stack;
                stack.count -= moved;
                if stack.count <= 0 {
                    return ItemStack::EMPTY;
                }
            }
        }

        stack
    }
}

fn can_stack(left: &ItemStack, right: &ItemStack) -> bool {
    !left.is_empty()
        && !right.is_empty()
        && left.item_id == right.item_id
        && left.damage == right.damage
}

fn item_max_stack(items: &ItemRegistry, stack: &ItemStack) -> i32 {
    if stack.is_empty() || stack.damage.is_some() {
        return 1;
    }
    let Some(name) = items.name_of(stack.item_id) else {
        return 64;
    };
    let path = name.path();
    if max_tool_damage_for_path(path).is_some()
        || matches!(
            path,
            "shield"
                | "bow"
                | "crossbow"
                | "trident"
                | "fishing_rod"
                | "shears"
                | "flint_and_steel"
        )
        || path.ends_with("_helmet")
        || path.ends_with("_chestplate")
        || path.ends_with("_leggings")
        || path.ends_with("_boots")
    {
        1
    } else {
        64
    }
}

fn armor_slot_for_kind(kind: mc_data::armor::ArmorSlot) -> usize {
    match kind {
        mc_data::armor::ArmorSlot::Head => 5,
        mc_data::armor::ArmorSlot::Chest => 6,
        mc_data::armor::ArmorSlot::Legs => 7,
        mc_data::armor::ArmorSlot::Feet => 8,
    }
}

fn armor_entry_for_item(
    items: &ItemRegistry,
    item_id: u32,
) -> Option<&'static mc_data::armor::ArmorEntry> {
    items
        .name_of(item_id)
        .and_then(|name| mc_data::armor::builtin().entry(name))
}

fn equipped_armor_stats(items: &ItemRegistry, inventory: &PlayerInventory) -> ArmorStats {
    let mut total = ArmorStats {
        armor: 0.0,
        toughness: 0.0,
    };
    for slot in 5..=8 {
        let stack = &inventory.slots[slot];
        if stack.is_empty() {
            continue;
        }
        if let Some(entry) = armor_entry_for_item(items, stack.item_id) {
            total.armor += entry.armor;
            total.toughness += entry.toughness;
        }
    }
    total
}

fn armor_reduced_damage(amount: f32, stats: ArmorStats) -> f32 {
    let damage = amount.max(0.0);
    let toughness = 2.0 + stats.toughness / 4.0;
    let real_armor = (stats.armor - damage / toughness)
        .clamp(stats.armor * 0.2, 20.0)
        .max(0.0);
    damage * (1.0 - real_armor / 25.0)
}

fn survival_damage_after_armor(state: Option<&InteractionState>, amount: f32) -> f32 {
    let Some(state) = state else {
        return amount.max(0.0);
    };
    armor_reduced_damage(amount, equipped_armor_stats(&state.items, &state.inventory))
}

fn damage_equipped_armor(state: &mut InteractionState) -> Vec<(usize, ItemStack)> {
    let mut changed = Vec::new();
    for slot in 5..=8 {
        let stack = &mut state.inventory.slots[slot];
        if stack.is_empty() {
            continue;
        }
        let Some(entry) = armor_entry_for_item(&state.items, stack.item_id) else {
            continue;
        };
        let next_damage = stack.damage.unwrap_or(0) + 1;
        if next_damage >= entry.max_damage {
            *stack = ItemStack::EMPTY;
        } else {
            stack.damage = Some(next_damage);
        }
        changed.push((slot, stack.clone()));
    }
    changed
}

/// Item→default-block-state lookup for items whose identifier also
/// names a registered block.
#[derive(Debug, Clone, Default)]
struct ItemToBlockTable {
    entries: Vec<(u32, mc_world::BlockStateId)>,
}

impl ItemToBlockTable {
    fn build(items: &ItemRegistry, blocks: &mc_world::BlockRegistry) -> Self {
        let entries = items
            .iter()
            .filter_map(|(item_name, item_pid)| {
                blocks
                    .block(item_name)
                    .map(|block| (item_pid, block.default))
            })
            .collect();
        Self { entries }
    }

    fn resolve(&self, item_id: u32) -> Option<mc_world::BlockStateId> {
        self.entries
            .iter()
            .find_map(|(id, state)| (*id == item_id).then_some(*state))
    }
}

fn fallback_crafting_recipes() -> Vec<mc_data::recipes::Recipe> {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, ShapedRecipe,
        ShapelessRecipe, SmeltingRecipe,
    };

    let item =
        |id: &str| IngredientAlternative::Item(Identifier::parse(id).expect("static identifier"));
    let tag =
        |id: &str| IngredientAlternative::Tag(Identifier::parse(id).expect("static identifier"));
    let ingredient = |alternatives: Vec<IngredientAlternative>| Ingredient { alternatives };
    let shaped = |id: &str,
                  pattern: Vec<&str>,
                  key: Vec<(char, Vec<IngredientAlternative>)>,
                  result: &str,
                  count: u32| {
        Recipe {
            id: Identifier::parse(id).expect("static identifier"),
            kind: RecipeKind::Shaped(ShapedRecipe {
                pattern: pattern.into_iter().map(str::to_string).collect(),
                key: key
                    .into_iter()
                    .map(|(ch, alternatives)| (ch, ingredient(alternatives)))
                    .collect(),
            }),
            result: RecipeResult {
                item: Identifier::parse(result).expect("static identifier"),
                count,
            },
        }
    };
    let smelting = |id: &str, input: &str, result: &str| Recipe {
        id: Identifier::parse(id).expect("static identifier"),
        kind: RecipeKind::Smelting(SmeltingRecipe {
            ingredient: ingredient(vec![item(input)]),
            cooking_time: 200,
        }),
        result: RecipeResult {
            item: Identifier::parse(result).expect("static identifier"),
            count: 1,
        },
    };

    let mut recipes = vec![
        shaped(
            "minecraft:torch",
            vec!["X", "#"],
            vec![
                (
                    'X',
                    vec![item("minecraft:coal"), item("minecraft:charcoal")],
                ),
                ('#', vec![item("minecraft:stick")]),
            ],
            "minecraft:torch",
            4,
        ),
        Recipe {
            id: Identifier::parse("minecraft:oak_planks").expect("static identifier"),
            kind: RecipeKind::Shapeless(ShapelessRecipe {
                ingredients: vec![ingredient(vec![tag("minecraft:oak_logs")])],
            }),
            result: RecipeResult {
                item: Identifier::parse("minecraft:oak_planks").expect("static identifier"),
                count: 4,
            },
        },
        shaped(
            "minecraft:stick",
            vec!["#", "#"],
            vec![('#', vec![tag("minecraft:planks")])],
            "minecraft:stick",
            4,
        ),
        shaped(
            "minecraft:crafting_table",
            vec!["##", "##"],
            vec![('#', vec![tag("minecraft:planks")])],
            "minecraft:crafting_table",
            1,
        ),
        shaped(
            "minecraft:wooden_pickaxe",
            vec!["###", " X ", " X "],
            vec![
                ('#', vec![tag("minecraft:planks")]),
                ('X', vec![item("minecraft:stick")]),
            ],
            "minecraft:wooden_pickaxe",
            1,
        ),
        shaped(
            "minecraft:wooden_axe",
            vec!["##", "#X", " X"],
            vec![
                ('#', vec![tag("minecraft:planks")]),
                ('X', vec![item("minecraft:stick")]),
            ],
            "minecraft:wooden_axe",
            1,
        ),
        shaped(
            "minecraft:wooden_shovel",
            vec!["#", "X", "X"],
            vec![
                ('#', vec![tag("minecraft:planks")]),
                ('X', vec![item("minecraft:stick")]),
            ],
            "minecraft:wooden_shovel",
            1,
        ),
        shaped(
            "minecraft:wooden_sword",
            vec!["#", "#", "X"],
            vec![
                ('#', vec![tag("minecraft:planks")]),
                ('X', vec![item("minecraft:stick")]),
            ],
            "minecraft:wooden_sword",
            1,
        ),
        shaped(
            "minecraft:stone_pickaxe",
            vec!["###", " X ", " X "],
            vec![
                ('#', vec![item("minecraft:cobblestone")]),
                ('X', vec![item("minecraft:stick")]),
            ],
            "minecraft:stone_pickaxe",
            1,
        ),
        shaped(
            "minecraft:stone_axe",
            vec!["##", "#X", " X"],
            vec![
                ('#', vec![item("minecraft:cobblestone")]),
                ('X', vec![item("minecraft:stick")]),
            ],
            "minecraft:stone_axe",
            1,
        ),
        shaped(
            "minecraft:stone_shovel",
            vec!["#", "X", "X"],
            vec![
                ('#', vec![item("minecraft:cobblestone")]),
                ('X', vec![item("minecraft:stick")]),
            ],
            "minecraft:stone_shovel",
            1,
        ),
        shaped(
            "minecraft:stone_sword",
            vec!["#", "#", "X"],
            vec![
                ('#', vec![item("minecraft:cobblestone")]),
                ('X', vec![item("minecraft:stick")]),
            ],
            "minecraft:stone_sword",
            1,
        ),
        shaped(
            "minecraft:furnace",
            vec!["###", "# #", "###"],
            vec![('#', vec![item("minecraft:cobblestone")])],
            "minecraft:furnace",
            1,
        ),
    ];
    recipes.extend([
        smelting(
            "minecraft:iron_ingot_from_smelting_raw_iron",
            "minecraft:raw_iron",
            "minecraft:iron_ingot",
        ),
        smelting(
            "minecraft:gold_ingot_from_smelting_raw_gold",
            "minecraft:raw_gold",
            "minecraft:gold_ingot",
        ),
        smelting(
            "minecraft:copper_ingot_from_smelting_raw_copper",
            "minecraft:raw_copper",
            "minecraft:copper_ingot",
        ),
        smelting(
            "minecraft:cooked_beef",
            "minecraft:beef",
            "minecraft:cooked_beef",
        ),
        smelting(
            "minecraft:cooked_porkchop",
            "minecraft:porkchop",
            "minecraft:cooked_porkchop",
        ),
        smelting(
            "minecraft:cooked_chicken",
            "minecraft:chicken",
            "minecraft:cooked_chicken",
        ),
        smelting(
            "minecraft:baked_potato",
            "minecraft:potato",
            "minecraft:baked_potato",
        ),
    ]);
    recipes
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

fn passive_chunk_spawns(chunk: (i32, i32)) -> bool {
    if chunk == (0, 0) {
        return true;
    }
    let h = herd_hash(chunk, 0, 0x4845_5244);
    h.is_multiple_of(9)
}

fn herd_hash(chunk: (i32, i32), slot: u8, salt: u64) -> u64 {
    let mut h = salt;
    h ^= (chunk.0 as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h = h.rotate_left(23);
    h ^= (chunk.1 as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h = h.rotate_left(17);
    h ^= (slot as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h.wrapping_mul(0x94D0_49BB_1331_11EB) ^ (h >> 31)
}

fn herd_uuid(chunk: (i32, i32), slot: u8) -> uuid::Uuid {
    let hi = herd_hash(chunk, slot, 0x434F_575F_4845_5244);
    let lo = herd_hash(chunk, slot, 0x5041_5353_4956_4500);
    uuid::Uuid::from_u128(((hi as u128) << 64) | lo as u128)
}

fn plan_passive_herd(
    chunk: &Chunk,
    land_surface: Option<mc_world::BlockStateId>,
    water: Option<mc_world::BlockStateId>,
    passable: &[BlockStateId],
    rules: &mc_data::biomes::BiomeSpawnRules,
    entity_types: &mc_data::entity_types::EntityTypeRegistry,
) -> Vec<HerdSpawn> {
    let chunk_pos = (chunk.pos.x, chunk.pos.z);
    if !passive_chunk_spawns(chunk_pos) {
        return Vec::new();
    }
    let mut spawns = Vec::new();
    if let Some(surface) = land_surface {
        plan_group_spawns(
            chunk,
            surface,
            passable,
            "creature",
            rules,
            entity_types,
            &mut spawns,
        );
    }
    if let Some(water) = water {
        plan_water_group_spawns(
            chunk,
            water,
            "water_ambient",
            rules,
            entity_types,
            &mut spawns,
        );
        plan_water_group_spawns(
            chunk,
            water,
            "water_creature",
            rules,
            entity_types,
            &mut spawns,
        );
    }
    spawns
}

fn plan_group_spawns(
    chunk: &Chunk,
    surface: mc_world::BlockStateId,
    passable: &[BlockStateId],
    group: &str,
    rules: &mc_data::biomes::BiomeSpawnRules,
    entity_types: &mc_data::entity_types::EntityTypeRegistry,
    out: &mut Vec<HerdSpawn>,
) {
    let chunk_pos = (chunk.pos.x, chunk.pos.z);
    let slot_base = out.len() as u8;
    let h = herd_hash(chunk_pos, slot_base, 0x5350_4157_4E00_0000);
    let Some((lx, y, lz)) = herd_spawn_surface(chunk, surface, passable, h) else {
        return;
    };
    let Some(biome) = chunk_biome_at(chunk, lx, y, lz) else {
        return;
    };
    let Some(entry) = choose_biome_spawn(rules.entries(biome, group), chunk_pos, slot_base) else {
        return;
    };
    let Some(entity_type_id) = entity_types
        .id_of(&entry.entity_type)
        .and_then(|id| i32::try_from(id).ok())
    else {
        return;
    };
    let count = herd_entry_count(entry, chunk_pos, slot_base).min(6);
    for i in 0..count {
        let slot = slot_base + i as u8;
        let offset = herd_hash(chunk_pos, slot, 0x4F46_4653_4554_0000);
        out.push(HerdSpawn {
            chunk: chunk_pos,
            slot,
            entity_type_id,
            entity_type_name: entry.entity_type.as_str().to_string(),
            position: Vec3::new(
                f64::from(chunk.pos.x * 16 + i32::from(lx)) + 0.35 + (offset & 3) as f64 * 0.1,
                f64::from(y + 1),
                f64::from(chunk.pos.z * 16 + i32::from(lz))
                    + 0.35
                    + ((offset >> 2) & 3) as f64 * 0.1,
            ),
        });
    }
}

fn plan_water_group_spawns(
    chunk: &Chunk,
    water: mc_world::BlockStateId,
    group: &str,
    rules: &mc_data::biomes::BiomeSpawnRules,
    entity_types: &mc_data::entity_types::EntityTypeRegistry,
    out: &mut Vec<HerdSpawn>,
) {
    let chunk_pos = (chunk.pos.x, chunk.pos.z);
    let slot_base = out.len() as u8;
    let h = herd_hash(chunk_pos, slot_base, 0x5741_5445_5200_0000);
    let lx = 3 + (h as u8 % 10);
    let lz = 3 + ((h >> 8) as u8 % 10);
    if chunk.get_block(lx, DEFAULT_SEA_LEVEL, lz) != Some(water) {
        return;
    }
    let Some(biome) = chunk_biome_at(chunk, lx, DEFAULT_SEA_LEVEL, lz) else {
        return;
    };
    let Some(entry) = choose_biome_spawn(rules.entries(biome, group), chunk_pos, slot_base) else {
        return;
    };
    let Some(entity_type_id) = entity_types
        .id_of(&entry.entity_type)
        .and_then(|id| i32::try_from(id).ok())
    else {
        return;
    };
    let count = herd_entry_count(entry, chunk_pos, slot_base).min(6);
    for i in 0..count {
        let slot = slot_base + i as u8;
        out.push(HerdSpawn {
            chunk: chunk_pos,
            slot,
            entity_type_id,
            entity_type_name: entry.entity_type.as_str().to_string(),
            position: Vec3::new(
                f64::from(chunk.pos.x * 16 + i32::from(lx)) + 0.5,
                f64::from(DEFAULT_SEA_LEVEL - 2),
                f64::from(chunk.pos.z * 16 + i32::from(lz)) + 0.5,
            ),
        });
    }
}

fn choose_biome_spawn(
    entries: &[mc_data::biomes::BiomeSpawnEntry],
    chunk: (i32, i32),
    slot: u8,
) -> Option<&mc_data::biomes::BiomeSpawnEntry> {
    let total: u32 = entries.iter().map(|entry| entry.weight).sum();
    if total == 0 {
        return None;
    }
    let mut pick = (herd_hash(chunk, slot, 0x5745_4947_4854_0000) % u64::from(total)) as u32;
    for entry in entries {
        if pick < entry.weight {
            return Some(entry);
        }
        pick -= entry.weight;
    }
    entries.last()
}

fn herd_entry_count(
    entry: &mc_data::biomes::BiomeSpawnEntry,
    chunk: (i32, i32),
    slot: u8,
) -> usize {
    let min = entry.min_count.min(entry.max_count).max(1);
    let max = entry.max_count.max(min);
    let span = max - min + 1;
    (min + (herd_hash(chunk, slot, 0x434F_554E_5400_0000) as u32 % span)) as usize
}

fn chunk_biome_at(chunk: &Chunk, lx: u8, y: i32, lz: u8) -> Option<&mc_data::Identifier> {
    let section = ((y - mc_world::MIN_Y) / 16).clamp(0, mc_world::SECTION_COUNT as i32 - 1);
    let section = chunk.biomes.get(section as usize)?;
    let local_y = (y - mc_world::MIN_Y).rem_euclid(16) as u8 / 4;
    Some(section.get(lx / 4, local_y, lz / 4))
}

fn herd_surface_y(chunk: &Chunk, lx: u8, lz: u8, surface: mc_world::BlockStateId) -> Option<i32> {
    if let Some(y) = chunk.highest_opaque_y(lx, lz)
        && chunk.get_block(lx, y, lz) == Some(surface)
    {
        return Some(y);
    }
    (mc_world::MIN_Y..mc_world::MAX_Y)
        .rev()
        .find(|&y| chunk.get_block(lx, y, lz) == Some(surface))
}

fn herd_spawn_surface(
    chunk: &Chunk,
    surface: BlockStateId,
    passable: &[BlockStateId],
    h: u64,
) -> Option<(u8, i32, u8)> {
    for attempt in 0..100u64 {
        let candidate = h.wrapping_add(attempt.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let lx = 3 + (candidate as u8 % 10);
        let lz = 3 + ((candidate >> 8) as u8 % 10);
        let Some(y) = herd_surface_y(chunk, lx, lz, surface) else {
            continue;
        };
        if herd_spawn_clearance(chunk, lx, y + 1, lz, passable) {
            return Some((lx, y, lz));
        }
    }
    None
}

fn herd_spawn_clearance(
    chunk: &Chunk,
    lx: u8,
    spawn_y: i32,
    lz: u8,
    passable: &[BlockStateId],
) -> bool {
    (spawn_y..=spawn_y + 1).all(|y| {
        chunk
            .get_block(lx, y, lz)
            .is_some_and(|state| passable.contains(&state))
    })
}

pub(crate) fn passive_entity_passable_blocks(blocks: &BlockRegistry) -> Vec<BlockStateId> {
    const PASSABLE_BLOCKS: &[&str] = &[
        "minecraft:air",
        "minecraft:short_grass",
        "minecraft:tall_grass",
        "minecraft:fern",
        "minecraft:large_fern",
        "minecraft:dandelion",
        "minecraft:poppy",
        "minecraft:sugar_cane",
    ];

    PASSABLE_BLOCKS
        .iter()
        .filter_map(|name| {
            blocks.block(&mc_data::Identifier::parse(*name).expect("static identifier"))
        })
        .flat_map(|block| block.states.iter().copied())
        .collect()
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
        passive_herd_surface: Option<mc_world::BlockStateId>,
        passive_herd_water: Option<mc_world::BlockStateId>,
        passive_herd_passable: Arc<Vec<BlockStateId>>,
        passive_spawn_rules: Arc<mc_data::biomes::BiomeSpawnRules>,
        entity_types: Arc<mc_data::entity_types::EntityTypeRegistry>,
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
            passive_herd_surface,
            passive_herd_water,
            passive_herd_passable,
            passive_spawn_rules,
            entity_types,
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
            let passive_herd_surface = self.passive_herd_surface;
            let passive_herd_water = self.passive_herd_water;
            let passive_herd_passable = Arc::clone(&self.passive_herd_passable);
            let passive_spawn_rules = Arc::clone(&self.passive_spawn_rules);
            let entity_types = Arc::clone(&self.entity_types);
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
                    passive_herd_surface,
                    passive_herd_water,
                    passive_herd_passable,
                    passive_spawn_rules,
                    entity_types,
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
                let mut visibility = self.sessions.mark_loaded(self.session_id, (cx, cz));
                visibility.extend(
                    self.sessions
                        .ensure_chunk_herd((cx, cz), &prepared.herd_spawns),
                );
                dispatch_visibility_commands(visibility);
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

#[allow(clippy::too_many_arguments)]
async fn prepare_chunk_request(
    request: ChunkRequest,
    world: WorldHandle,
    biomes: Arc<Registry>,
    block_light: Option<Arc<BlockLightTable>>,
    passive_herd_surface: Option<mc_world::BlockStateId>,
    passive_herd_water: Option<mc_world::BlockStateId>,
    passive_herd_passable: Arc<Vec<BlockStateId>>,
    passive_spawn_rules: Arc<mc_data::biomes::BiomeSpawnRules>,
    entity_types: Arc<mc_data::entity_types::EntityTypeRegistry>,
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
                    passive_herd_surface,
                    passive_herd_water,
                    passive_herd_passable.as_ref(),
                    passive_spawn_rules.as_ref(),
                    entity_types.as_ref(),
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
                passive_herd_surface,
                passive_herd_water,
                passive_herd_passable.as_ref(),
                passive_spawn_rules.as_ref(),
                entity_types.as_ref(),
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
    herd_spawns: Vec<HerdSpawn>,
    timing: ChunkBuildTiming,
}

#[allow(clippy::too_many_arguments)]
fn build_chunk_packet(
    centre: &Chunk,
    neighbourhood: &[[Option<Arc<Chunk>>; 3]; 3],
    biomes: &Registry,
    block_light: Option<&BlockLightTable>,
    passive_herd_surface: Option<mc_world::BlockStateId>,
    passive_herd_water: Option<mc_world::BlockStateId>,
    passive_herd_passable: &[BlockStateId],
    passive_spawn_rules: &mc_data::biomes::BiomeSpawnRules,
    entity_types: &mc_data::entity_types::EntityTypeRegistry,
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
    let herd_spawns = plan_passive_herd(
        centre,
        passive_herd_surface,
        passive_herd_water,
        passive_herd_passable,
        passive_spawn_rules,
        entity_types,
    );
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
        herd_spawns,
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
        herd_spawns: built.herd_spawns,
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

async fn write_block_ack<W>(
    writer: &mut W,
    compression: Compression,
    sequence: i32,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(writer, &BlockChangedAck { sequence }, compression).await
}

fn held_item_id(state: &InteractionState) -> Option<u32> {
    let held = state.inventory.held(state.selected_hotbar_slot);
    (!held.is_empty()).then_some(held.item_id)
}

fn pending_break_matches(
    state: &InteractionState,
    pending: &PendingBreak,
    action: &ServerboundPlayerAction,
) -> bool {
    pending.position == action.position
        && pending.direction == action.direction
        && pending.held_hotbar_slot == state.selected_hotbar_slot
        && pending.held_item_id == held_item_id(state)
}

fn pending_break_is_complete(pending: &PendingBreak, now: Instant) -> bool {
    now.duration_since(pending.started_at) >= pending.required_time
}

fn fallback_mining_rule(block_path: &str) -> MiningRule {
    FALLBACK_MINING_RULES
        .iter()
        .copied()
        .find(|rule| {
            rule.block_path_contains
                .iter()
                .any(|needle| block_path.contains(needle))
        })
        .unwrap_or(UNKNOWN_BLOCK_MINING_RULE)
}

fn tool_speed_divisor(tool_path: Option<&str>, required_suffix: Option<&str>) -> u64 {
    let Some(tool_path) = tool_path else {
        return 1;
    };
    if required_suffix.is_some_and(|suffix| !tool_path.ends_with(suffix)) {
        return 1;
    }

    if tool_path.starts_with("golden_") {
        10
    } else if tool_path.starts_with("netherite_") {
        8
    } else if tool_path.starts_with("diamond_") {
        6
    } else if tool_path.starts_with("iron_") {
        4
    } else if tool_path.starts_with("stone_") {
        3
    } else if tool_path.starts_with("wooden_") {
        2
    } else {
        1
    }
}

fn fallback_mining_time(block_path: &str, tool_path: Option<&str>) -> Duration {
    let rule = fallback_mining_rule(block_path);
    let divisor = tool_speed_divisor(tool_path, rule.tool_suffix);
    Duration::from_millis((rule.base_time.as_millis() as u64 / divisor).max(100))
}

fn mining_time_for_block(
    blocks: &BlockRegistry,
    items: &ItemRegistry,
    block_state: BlockStateId,
    held_item_id: Option<u32>,
) -> Duration {
    let Some(block_state) = blocks.by_id(block_state) else {
        return UNKNOWN_BLOCK_MINING_RULE.base_time;
    };
    let tool_path = held_item_id
        .and_then(|id| items.name_of(id))
        .map(mc_data::Identifier::path);
    fallback_mining_time(block_state.block.id.path(), tool_path)
}

async fn mining_time_for_target(state: &InteractionState, position: i64) -> Duration {
    let (x, y, z) = unpack_block_pos(position);
    let block_state = {
        let mut storage = state.world.lock().await;
        storage
            .get_block(mc_world::BlockPos { x, y, z })
            .map_err(|err| {
                warn!(error = %err, x, y, z, "mining target read failed; using fallback timing");
            })
            .ok()
            .flatten()
    };

    block_state.map_or(UNKNOWN_BLOCK_MINING_RULE.base_time, |block_state| {
        mining_time_for_block(
            &state.blocks,
            &state.items,
            block_state,
            held_item_id(state),
        )
    })
}

fn item_entity_type_id(entity_types: &EntityTypeRegistry) -> Option<i32> {
    let item = mc_data::Identifier::parse("minecraft:item").expect("static identifier");
    entity_types
        .id_of(&item)
        .and_then(|id| i32::try_from(id).ok())
}

fn passive_mob_drop_stack(state: &InteractionState, entity_type: &str) -> Option<ItemStack> {
    let entity = Identifier::parse(entity_type.to_string()).ok()?;
    let item = mc_data::loot::builtin().entity_drop(&entity)?;
    let item_id = state.items.id_of(item)?;
    Some(ItemStack::new(item_id, 1))
}

fn block_drop_stack(state: &InteractionState, block_state: BlockStateId) -> Option<ItemStack> {
    let block = state.blocks.by_id(block_state)?;
    let item = mc_data::loot::builtin()
        .block_drop(&block.block.id)
        .unwrap_or(&block.block.id);
    let item_id = state.items.id_of(item)?;
    Some(ItemStack::new(item_id, 1))
}

fn food_rule_for_item(item: &mc_data::Identifier) -> Option<FoodRule> {
    FALLBACK_FOOD_RULES
        .iter()
        .copied()
        .find(|rule| item.as_str() == rule.item)
}

fn held_food_use(state: &InteractionState) -> Option<(u32, FoodRule)> {
    let held = state.inventory.held(state.selected_hotbar_slot);
    if held.is_empty() {
        return None;
    }
    let rule = state
        .items
        .name_of(held.item_id)
        .and_then(food_rule_for_item)?;
    Some((held.item_id, rule))
}

fn pending_use_matches(state: &InteractionState, pending: &PendingUse) -> bool {
    pending.held_hotbar_slot == state.selected_hotbar_slot
        && state.inventory.held(pending.held_hotbar_slot).item_id == pending.held_item_id
}

fn pending_use_is_complete(pending: &PendingUse, now: Instant) -> bool {
    now.duration_since(pending.started_at) >= pending.required_time
}

fn entity_item_stack(stack: ItemStack) -> EntityItemStack {
    EntityItemStack::new(stack.item_id, stack.count)
}

fn held_attack_damage(state: &InteractionState) -> f32 {
    let held = state.inventory.held(state.selected_hotbar_slot);
    let Some(path) = (!held.is_empty())
        .then(|| state.items.name_of(held.item_id))
        .flatten()
        .map(|id| id.path())
    else {
        return 2.0;
    };
    if path.ends_with("_sword") {
        8.0
    } else if path.ends_with("_axe") {
        7.0
    } else if path.ends_with("_pickaxe") || path.ends_with("_shovel") {
        4.0
    } else {
        2.0
    }
}

fn is_durability_tool_path(path: &str) -> bool {
    path.ends_with("_axe")
        || path.ends_with("_hoe")
        || path.ends_with("_pickaxe")
        || path.ends_with("_shovel")
        || path.ends_with("_sword")
}

fn max_tool_damage_for_path(path: &str) -> Option<i32> {
    if !is_durability_tool_path(path) {
        return None;
    }
    let max = if path.starts_with("wooden_") {
        59
    } else if path.starts_with("stone_") {
        131
    } else if path.starts_with("iron_") {
        250
    } else if path.starts_with("diamond_") {
        1561
    } else if path.starts_with("golden_") {
        32
    } else if path.starts_with("netherite_") {
        2031
    } else {
        return None;
    };
    Some(max)
}

fn damage_held_tool_stack(state: &mut InteractionState) -> Option<(usize, ItemStack)> {
    let hotbar_slot = state.selected_hotbar_slot;
    let wire_slot = PlayerInventory::HOTBAR_BASE + hotbar_slot as usize;
    let item_path = {
        let held = state.inventory.held(hotbar_slot);
        if held.is_empty() {
            return None;
        }
        state.items.name_of(held.item_id)?.path().to_owned()
    };
    let max_damage = max_tool_damage_for_path(&item_path)?;

    let held = state.inventory.held_mut(hotbar_slot);
    let new_damage = held.damage.unwrap_or(0).saturating_add(1);
    if new_damage >= max_damage {
        *held = ItemStack::EMPTY;
    } else {
        held.damage = Some(new_damage);
    }
    Some((wire_slot, held.clone()))
}

async fn damage_held_tool_after_mining<W>(
    state: &mut InteractionState,
    writer: &mut W,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if let Some(changed) = damage_held_tool_stack(state) {
        write_inventory_slot_updates(state, writer, vec![changed]).await?;
    }
    Ok(())
}

fn recipe_ingredients(
    recipe: &mc_data::recipes::Recipe,
) -> Option<Vec<&mc_data::recipes::Ingredient>> {
    match &recipe.kind {
        mc_data::recipes::RecipeKind::Shapeless(shapeless) => {
            Some(shapeless.ingredients.iter().collect())
        }
        mc_data::recipes::RecipeKind::Shaped(shaped) => {
            let mut ingredients = Vec::new();
            for row in &shaped.pattern {
                for ch in row.chars().filter(|ch| *ch != ' ') {
                    ingredients.push(shaped.key.get(&ch)?);
                }
            }
            Some(ingredients)
        }
        mc_data::recipes::RecipeKind::Smelting(_) => None,
    }
}

fn matching_ingredient_slot(
    state: &InteractionState,
    available: &[i32; 46],
    ingredient: &mc_data::recipes::Ingredient,
) -> Option<usize> {
    for (slot, available_count) in available.iter().enumerate().take(45).skip(9) {
        let current = &state.inventory.slots[slot];
        if *available_count > 0
            && ingredient_accepts_item(&state.items, &state.tags, current.item_id, ingredient)
        {
            return Some(slot);
        }
    }
    None
}

fn ingredient_accepts_item(
    items: &ItemRegistry,
    tags: &TagsData,
    item_id: u32,
    ingredient: &mc_data::recipes::Ingredient,
) -> bool {
    ingredient
        .alternatives
        .iter()
        .any(|alternative| ingredient_alternative_accepts_item(items, tags, item_id, alternative))
}

fn ingredient_alternative_accepts_item(
    items: &ItemRegistry,
    tags: &TagsData,
    item_id: u32,
    alternative: &mc_data::recipes::IngredientAlternative,
) -> bool {
    match alternative {
        mc_data::recipes::IngredientAlternative::Item(item) => items.id_of(item) == Some(item_id),
        mc_data::recipes::IngredientAlternative::Tag(tag) => {
            let item_registry = Identifier::parse("minecraft:item").expect("static identifier");
            tags.registries
                .get(&item_registry)
                .and_then(|item_tags| item_tags.get(tag))
                .is_some_and(|entries| {
                    entries
                        .iter()
                        .any(|entry| u32::try_from(*entry).ok() == Some(item_id))
                })
        }
    }
}

fn inventory_has_room_for_output(state: &InteractionState, item_id: u32, count: i32) -> bool {
    let mut remaining = count;
    for slot in 9..=44 {
        let current = &state.inventory.slots[slot];
        if current.is_empty() {
            remaining -= remaining.min(64);
        } else if current.item_id == item_id && current.damage.is_none() && current.count < 64 {
            remaining -= remaining.min(64 - current.count);
        }
        if remaining <= 0 {
            return true;
        }
    }
    false
}

fn craft_recipe_once(
    state: &mut InteractionState,
    recipe: &mc_data::recipes::Recipe,
) -> Option<Vec<(usize, ItemStack)>> {
    let ingredients = recipe_ingredients(recipe)?;
    if ingredients.is_empty() {
        return None;
    }
    let output_item_id = state.items.id_of(&recipe.result.item)?;
    let output_count = i32::try_from(recipe.result.count).ok()?;
    if output_count <= 0 || !inventory_has_room_for_output(state, output_item_id, output_count) {
        return None;
    }

    let mut available = std::array::from_fn(|slot| state.inventory.slots[slot].count.max(0));
    let mut consumed_slots = Vec::with_capacity(ingredients.len());
    for ingredient in ingredients {
        let slot = matching_ingredient_slot(state, &available, ingredient)?;
        available[slot] -= 1;
        consumed_slots.push(slot);
    }

    let mut changed = BTreeMap::new();
    for slot in consumed_slots {
        let current = &mut state.inventory.slots[slot];
        current.count -= 1;
        if current.count <= 0 {
            *current = ItemStack::EMPTY;
        }
        changed.insert(slot, current.clone());
    }

    let (remaining, output_changed) = state
        .inventory
        .merge_stack(ItemStack::new(output_item_id, output_count));
    if !remaining.is_empty() {
        return None;
    }
    for (slot, stack) in output_changed {
        changed.insert(slot, stack);
    }
    Some(changed.into_iter().collect())
}

fn craft_recipe(
    state: &mut InteractionState,
    recipe: &mc_data::recipes::Recipe,
    use_max_items: bool,
) -> Option<Vec<(usize, ItemStack)>> {
    if !use_max_items {
        return craft_recipe_once(state, recipe);
    }

    let mut all_changed = BTreeMap::new();
    while let Some(changed) = craft_recipe_once(state, recipe) {
        for (slot, stack) in changed {
            all_changed.insert(slot, stack);
        }
    }
    (!all_changed.is_empty()).then(|| all_changed.into_iter().collect())
}

fn is_furnace_state(state: &InteractionState, block_state: mc_world::BlockStateId) -> bool {
    state
        .blocks
        .by_id(block_state)
        .is_some_and(|block_state| block_state.block.id.as_str() == "minecraft:furnace")
}

fn is_crafting_table_state(state: &InteractionState, block_state: mc_world::BlockStateId) -> bool {
    state
        .blocks
        .by_id(block_state)
        .is_some_and(|block_state| block_state.block.id.as_str() == "minecraft:crafting_table")
}

fn find_smelting_recipe_for_item(
    state: &InteractionState,
    item_id: u32,
) -> Option<mc_data::recipes::Recipe> {
    state.recipes.iter().find_map(|recipe| {
        let mc_data::recipes::RecipeKind::Smelting(smelting) = &recipe.kind else {
            return None;
        };
        ingredient_accepts_item(&state.items, &state.tags, item_id, &smelting.ingredient)
            .then(|| recipe.clone())
    })
}

fn is_fuel_item(state: &InteractionState, item_id: u32) -> bool {
    let coal = state
        .items
        .id_of(&Identifier::parse("minecraft:coal").expect("static identifier"));
    let charcoal = state
        .items
        .id_of(&Identifier::parse("minecraft:charcoal").expect("static identifier"));
    Some(item_id) == coal || Some(item_id) == charcoal
}

fn furnace_menu_title_nbt() -> Vec<u8> {
    let mut out = Vec::new();
    mc_nbt::write_network(
        &mut out,
        &Tag::Compound(vec![(
            "text".to_string(),
            Tag::String("Furnace".to_string()),
        )]),
    )
    .expect("static text component is valid NBT");
    out
}

fn crafting_menu_title_nbt() -> Vec<u8> {
    let mut out = Vec::new();
    mc_nbt::write_network(
        &mut out,
        &Tag::Compound(vec![(
            "text".to_string(),
            Tag::String("Crafting".to_string()),
        )]),
    )
    .expect("static text component is valid NBT");
    out
}

fn next_container_id(state: &mut InteractionState) -> i32 {
    let id = state.next_container_id;
    state.next_container_id += 1;
    if state.next_container_id > FURNACE_CONTAINER_ID_MAX {
        state.next_container_id = FURNACE_CONTAINER_ID_MIN;
    }
    id
}

fn store_active_container(state: &mut InteractionState) {
    match state.active_container.take() {
        Some(ActiveContainer::Furnace(window)) => {
            state
                .sessions
                .unregister_furnace_viewer(state.session_id, window.position);
        }
        Some(ActiveContainer::CraftingTable(window)) => {
            for stack in window.input {
                let (remaining, _) = state.inventory.merge_stack(stack);
                if !remaining.is_empty() {
                    debug!(
                        item_id = remaining.item_id,
                        count = remaining.count,
                        "dropping crafting remainder because inventory is full"
                    );
                }
            }
        }
        None => {}
    }
}

fn crafting_player_slot(menu_slot: usize) -> Option<usize> {
    match menu_slot {
        10..=36 => Some(9 + (menu_slot - 10)),
        37..=45 => Some(36 + (menu_slot - 37)),
        _ => None,
    }
}

fn shaped_recipe_matches(
    state: &InteractionState,
    input: &[ItemStack; 9],
    shaped: &mc_data::recipes::ShapedRecipe,
) -> bool {
    let height = shaped.pattern.len();
    let width = shaped
        .pattern
        .iter()
        .map(|row| row.chars().count())
        .max()
        .unwrap_or(0);
    if height == 0 || width == 0 || height > 3 || width > 3 {
        return false;
    }

    for top in 0..=(3 - height) {
        'left: for left in 0..=(3 - width) {
            for row in 0..3 {
                for col in 0..3 {
                    let stack = &input[row * 3 + col];
                    let ingredient =
                        if row >= top && row < top + height && col >= left && col < left + width {
                            shaped
                                .pattern
                                .get(row - top)
                                .and_then(|pattern_row| pattern_row.chars().nth(col - left))
                                .filter(|ch| *ch != ' ')
                                .and_then(|ch| shaped.key.get(&ch))
                        } else {
                            None
                        };
                    match ingredient {
                        Some(ingredient)
                            if !stack.is_empty()
                                && ingredient_accepts_item(
                                    &state.items,
                                    &state.tags,
                                    stack.item_id,
                                    ingredient,
                                ) => {}
                        None if stack.is_empty() => {}
                        _ => continue 'left,
                    }
                }
            }
            return true;
        }
    }
    false
}

fn shapeless_recipe_matches(
    state: &InteractionState,
    input: &[ItemStack; 9],
    shapeless: &mc_data::recipes::ShapelessRecipe,
) -> bool {
    let stacks: Vec<_> = input.iter().filter(|stack| !stack.is_empty()).collect();
    if stacks.len() != shapeless.ingredients.len() {
        return false;
    }
    let mut used = vec![false; shapeless.ingredients.len()];
    for stack in stacks {
        let Some((idx, _)) = shapeless
            .ingredients
            .iter()
            .enumerate()
            .find(|(idx, ingredient)| {
                !used[*idx]
                    && ingredient_accepts_item(&state.items, &state.tags, stack.item_id, ingredient)
            })
        else {
            return false;
        };
        used[idx] = true;
    }
    true
}

fn crafting_recipe_matches(
    state: &InteractionState,
    input: &[ItemStack; 9],
    recipe: &mc_data::recipes::Recipe,
) -> bool {
    match &recipe.kind {
        mc_data::recipes::RecipeKind::Shaped(shaped) => shaped_recipe_matches(state, input, shaped),
        mc_data::recipes::RecipeKind::Shapeless(shapeless) => {
            shapeless_recipe_matches(state, input, shapeless)
        }
        mc_data::recipes::RecipeKind::Smelting(_) => false,
    }
}

fn crafting_result_from_input(state: &InteractionState, input: &[ItemStack; 9]) -> ItemStack {
    state
        .recipes
        .iter()
        .find(|recipe| crafting_recipe_matches(state, input, recipe))
        .and_then(|recipe| {
            let item_id = state.items.id_of(&recipe.result.item)?;
            let count = i32::try_from(recipe.result.count).ok()?;
            (count > 0).then(|| ItemStack::new(item_id, count))
        })
        .unwrap_or(ItemStack::EMPTY)
}

fn refresh_crafting_result(state: &InteractionState, window: &mut CraftingTableWindow) {
    window.result = crafting_result_from_input(state, &window.input);
}

fn crafting_wire_items(
    window: &CraftingTableWindow,
    inventory: &PlayerInventory,
) -> Vec<ItemStack> {
    let mut items = Vec::with_capacity(CRAFTING_MENU_SLOT_COUNT);
    items.push(window.result.clone());
    items.extend(window.input.iter().cloned());
    items.extend((9..=35).map(|slot| inventory.slots[slot].clone()));
    items.extend((36..=44).map(|slot| inventory.slots[slot].clone()));
    items
}

async fn write_crafting_content<W>(
    state: &mut InteractionState,
    writer: &mut W,
    window: &CraftingTableWindow,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &ClientboundContainerSetContent {
            container_id: window.container_id,
            state_id: window.state_id,
            items: crafting_wire_items(window, &state.inventory),
            carried_item: state.carried_item.clone(),
        },
        state.compression,
    )
    .await
}

fn crafting_menu_stack(
    window: &CraftingTableWindow,
    inventory: &PlayerInventory,
    menu_slot: usize,
) -> Option<ItemStack> {
    match menu_slot {
        0 => Some(window.result.clone()),
        1..=9 => Some(window.input[menu_slot - 1].clone()),
        _ => crafting_player_slot(menu_slot).map(|slot| inventory.slots[slot].clone()),
    }
}

fn set_crafting_menu_stack(
    window: &mut CraftingTableWindow,
    inventory: &mut PlayerInventory,
    menu_slot: usize,
    stack: ItemStack,
) -> bool {
    match menu_slot {
        1..=9 => {
            window.input[menu_slot - 1] = stack;
            true
        }
        _ => {
            let Some(slot) = crafting_player_slot(menu_slot) else {
                return false;
            };
            inventory.slots[slot] = stack;
            true
        }
    }
}

fn crafting_remainder_for_item(state: &InteractionState, item_id: u32) -> Option<ItemStack> {
    let name = state.items.name_of(item_id)?;
    let bucket = Identifier::parse("minecraft:bucket").expect("static identifier");
    if name.path().ends_with("_bucket") || name.as_str() == "minecraft:milk_bucket" {
        state
            .items
            .id_of(&bucket)
            .map(|bucket_id| ItemStack::new(bucket_id, 1))
    } else {
        None
    }
}

fn consume_crafting_ingredients(state: &mut InteractionState, window: &mut CraftingTableWindow) {
    let consumed: Vec<_> = window
        .input
        .iter()
        .map(|stack| (!stack.is_empty()).then_some(stack.item_id))
        .collect();
    for (idx, item_id) in consumed.into_iter().enumerate() {
        let Some(item_id) = item_id else {
            continue;
        };
        window.input[idx].count -= 1;
        if window.input[idx].count <= 0 {
            window.input[idx] =
                crafting_remainder_for_item(state, item_id).unwrap_or(ItemStack::EMPTY);
        } else if let Some(remainder) = crafting_remainder_for_item(state, item_id) {
            let (remaining, _) = state.inventory.merge_stack(remainder);
            if !remaining.is_empty() {
                debug!(
                    item_id = remaining.item_id,
                    count = remaining.count,
                    "dropping crafting remainder because inventory is full"
                );
            }
        }
    }
    refresh_crafting_result(state, window);
}

fn take_crafting_result(state: &mut InteractionState, window: &mut CraftingTableWindow) -> bool {
    let result = window.result.clone();
    if result.is_empty() {
        return false;
    }
    let max_stack = item_max_stack(&state.items, &result);
    if state.carried_item.is_empty() {
        state.carried_item = result;
        consume_crafting_ingredients(state, window);
        return true;
    }
    if can_stack(&state.carried_item, &result)
        && state.carried_item.count + result.count <= max_stack
    {
        state.carried_item.count += result.count;
        consume_crafting_ingredients(state, window);
        return true;
    }
    false
}

fn apply_crafting_pickup_click(
    state: &mut InteractionState,
    window: &mut CraftingTableWindow,
    menu_slot: usize,
    button: i8,
) -> bool {
    if menu_slot >= CRAFTING_MENU_SLOT_COUNT || !(button == 0 || button == 1) {
        return false;
    }
    if menu_slot == 0 {
        return take_crafting_result(state, window);
    }
    let Some(slot_stack) = crafting_menu_stack(window, &state.inventory, menu_slot) else {
        return false;
    };
    let cursor = state.carried_item.clone();
    let max_stack = if cursor.is_empty() {
        item_max_stack(&state.items, &slot_stack)
    } else {
        item_max_stack(&state.items, &cursor)
    };

    let changed = if button == 0 {
        if cursor.is_empty() {
            if slot_stack.is_empty() {
                false
            } else {
                state.carried_item = slot_stack;
                set_crafting_menu_stack(window, &mut state.inventory, menu_slot, ItemStack::EMPTY)
            }
        } else if slot_stack.is_empty() {
            state.carried_item = ItemStack::EMPTY;
            set_crafting_menu_stack(window, &mut state.inventory, menu_slot, cursor)
        } else if can_stack(&slot_stack, &cursor) && slot_stack.count < max_stack {
            let moved = (max_stack - slot_stack.count).min(cursor.count);
            let mut new_slot = slot_stack;
            new_slot.count += moved;
            state.carried_item.count -= moved;
            if state.carried_item.count <= 0 {
                state.carried_item = ItemStack::EMPTY;
            }
            set_crafting_menu_stack(window, &mut state.inventory, menu_slot, new_slot)
        } else {
            state.carried_item = slot_stack;
            set_crafting_menu_stack(window, &mut state.inventory, menu_slot, cursor)
        }
    } else if cursor.is_empty() {
        if slot_stack.is_empty() {
            false
        } else {
            let moved = (slot_stack.count + 1) / 2;
            let mut new_cursor = slot_stack.clone();
            new_cursor.count = moved;
            let mut remaining = slot_stack;
            remaining.count -= moved;
            if remaining.count <= 0 {
                remaining = ItemStack::EMPTY;
            }
            state.carried_item = new_cursor;
            set_crafting_menu_stack(window, &mut state.inventory, menu_slot, remaining)
        }
    } else if slot_stack.is_empty() {
        let mut one = cursor;
        one.count = 1;
        decrement_cursor(&mut state.carried_item);
        set_crafting_menu_stack(window, &mut state.inventory, menu_slot, one)
    } else if can_stack(&slot_stack, &cursor) && slot_stack.count < max_stack {
        let mut new_slot = slot_stack;
        new_slot.count += 1;
        decrement_cursor(&mut state.carried_item);
        set_crafting_menu_stack(window, &mut state.inventory, menu_slot, new_slot)
    } else {
        false
    };
    if changed {
        refresh_crafting_result(state, window);
    }
    changed
}

fn apply_crafting_quick_move_click(
    state: &mut InteractionState,
    window: &mut CraftingTableWindow,
    menu_slot: usize,
) -> bool {
    if menu_slot >= CRAFTING_MENU_SLOT_COUNT {
        return false;
    }
    if menu_slot == 0 {
        let result = window.result.clone();
        if result.is_empty() {
            return false;
        }
        let (remaining, _) = state.inventory.merge_stack(result.clone());
        if !remaining.is_empty() {
            return false;
        }
        consume_crafting_ingredients(state, window);
        return true;
    }
    let Some(player_slot) = crafting_player_slot(menu_slot) else {
        return false;
    };
    let original = state.inventory.slots[player_slot].clone();
    if original.is_empty() {
        return false;
    }
    state.inventory.slots[player_slot] = ItemStack::EMPTY;
    let remaining = state.inventory.merge_stack_into_ranges(
        original.clone(),
        &[9..=35, 36..=44],
        item_max_stack(&state.items, &original),
    );
    state.inventory.slots[player_slot] = remaining;
    state.inventory.slots[player_slot] != original
}

async fn open_crafting_table_container<W>(
    state: &mut InteractionState,
    writer: &mut W,
    sequence: i32,
    x: i32,
    y: i32,
    z: i32,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let position = mc_world::BlockPos { x, y, z };
    let clicked = {
        let mut storage = state.world.lock().await;
        storage.get_block(position).map_err(|err| {
            warn!(error = %err, x, y, z, "crafting table use target read failed");
            err
        })?
    };
    if !clicked.is_some_and(|block_state| is_crafting_table_state(state, block_state)) {
        return Ok(false);
    }

    store_active_container(state);
    let mut window = CraftingTableWindow::new(next_container_id(state));
    refresh_crafting_result(state, &mut window);
    write_packet(
        writer,
        &ClientboundOpenScreen {
            container_id: window.container_id,
            menu_type: CRAFTING_MENU_TYPE_ID,
            title_nbt: crafting_menu_title_nbt(),
        },
        state.compression,
    )
    .await?;
    write_crafting_content(state, writer, &window).await?;
    state.active_container = Some(ActiveContainer::CraftingTable(window));
    write_block_ack(writer, state.compression, sequence).await?;
    Ok(true)
}

async fn handle_crafting_container_click<W>(
    state: &mut InteractionState,
    writer: &mut W,
    mut window: CraftingTableWindow,
    packet: ServerboundContainerClick,
) -> Result<CraftingTableWindow, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if packet.state_id != window.state_id {
        write_crafting_content(state, writer, &window).await?;
        return Ok(window);
    }
    let changed = match packet.container_input {
        ContainerInput::Pickup if packet.slot_num >= 0 => apply_crafting_pickup_click(
            state,
            &mut window,
            packet.slot_num as usize,
            packet.button_num,
        ),
        ContainerInput::QuickMove if packet.slot_num >= 0 && packet.button_num == 0 => {
            apply_crafting_quick_move_click(state, &mut window, packet.slot_num as usize)
        }
        _ => false,
    };
    if changed {
        window.state_id = window.state_id.wrapping_add(1);
    }
    write_crafting_content(state, writer, &window).await?;
    Ok(window)
}

fn furnace_slot_to_stack(slot: &FurnaceSlot) -> ItemStack {
    if slot.is_empty() {
        ItemStack::EMPTY
    } else {
        ItemStack {
            count: slot.count,
            item_id: slot.item_id,
            damage: slot.damage,
        }
    }
}

fn stack_to_furnace_slot(stack: &ItemStack) -> FurnaceSlot {
    if stack.is_empty() {
        FurnaceSlot::EMPTY
    } else {
        FurnaceSlot {
            count: stack.count,
            item_id: stack.item_id,
            damage: stack.damage,
        }
    }
}

fn furnace_data_values(furnace: &FurnaceBlockEntity) -> [(i16, i16); 4] {
    [
        (0, furnace.burn_remaining),
        (1, furnace.burn_total),
        (2, furnace.cook_progress),
        (3, furnace.cook_total),
    ]
}

fn furnace_wire_items(furnace: &FurnaceBlockEntity, inventory: &PlayerInventory) -> Vec<ItemStack> {
    let mut items = Vec::with_capacity(FURNACE_MENU_SLOT_COUNT);
    items.extend(furnace.slots.iter().map(furnace_slot_to_stack));
    items.extend((9..=35).map(|slot| inventory.slots[slot].clone()));
    items.extend((36..=44).map(|slot| inventory.slots[slot].clone()));
    items
}

fn furnace_slot_stacks(furnace: &FurnaceBlockEntity) -> [ItemStack; 3] {
    std::array::from_fn(|slot| furnace_slot_to_stack(&furnace.slots[slot]))
}

async fn write_furnace_data<W>(
    writer: &mut W,
    compression: Compression,
    window: &FurnaceWindow,
    furnace: &FurnaceBlockEntity,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    for (id, value) in furnace_data_values(furnace) {
        write_packet(
            writer,
            &ClientboundContainerSetData {
                container_id: window.container_id,
                id,
                value,
            },
            compression,
        )
        .await?;
    }
    Ok(())
}

async fn write_furnace_data_changes<W>(
    writer: &mut W,
    compression: Compression,
    window: &FurnaceWindow,
    changed: &[(i16, i16)],
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    for &(id, value) in changed {
        write_packet(
            writer,
            &ClientboundContainerSetData {
                container_id: window.container_id,
                id,
                value,
            },
            compression,
        )
        .await?;
    }
    Ok(())
}

async fn write_furnace_content<W>(
    state: &InteractionState,
    writer: &mut W,
    window: &FurnaceWindow,
    furnace: &FurnaceBlockEntity,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &ClientboundContainerSetContent {
            container_id: window.container_id,
            state_id: window.state_id,
            items: furnace_wire_items(furnace, &state.inventory),
            carried_item: state.carried_item.clone(),
        },
        state.compression,
    )
    .await
}

async fn open_furnace_container<W>(
    state: &mut InteractionState,
    writer: &mut W,
    sequence: i32,
    x: i32,
    y: i32,
    z: i32,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let position = mc_world::BlockPos { x, y, z };
    let clicked = {
        let mut storage = state.world.lock().await;
        storage
            .get_block(position)
            .map_err(|err| {
                warn!(error = %err, x, y, z, "furnace use target read failed");
                err
            })
            .ok()
            .flatten()
    };
    if !clicked.is_some_and(|block_state| is_furnace_state(state, block_state)) {
        return Ok(false);
    }

    store_active_container(state);
    let container_id = next_container_id(state);
    let window = FurnaceWindow::new(position, container_id);
    let furnace = {
        let mut storage = state.world.lock().await;
        storage.furnace_block_entity(position).map_err(|err| {
            warn!(error = %err, x, y, z, "furnace state read failed");
            err
        })?
    }
    .unwrap_or_default();
    write_packet(
        writer,
        &ClientboundOpenScreen {
            container_id,
            menu_type: FURNACE_MENU_TYPE_ID,
            title_nbt: furnace_menu_title_nbt(),
        },
        state.compression,
    )
    .await?;
    write_furnace_content(state, writer, &window, &furnace).await?;
    write_furnace_data(writer, state.compression, &window, &furnace).await?;
    state
        .sessions
        .register_furnace_viewer(state.session_id, window.position);
    state.active_container = Some(ActiveContainer::Furnace(window));
    write_block_ack(writer, state.compression, sequence).await?;
    Ok(true)
}

async fn handle_place_recipe<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    survival_state: SurvivalState,
    packet: ServerboundPlaceRecipe,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    state.pending_break = None;
    state.pending_use = None;
    if packet.container_id != 0 {
        debug!(
            container_id = packet.container_id,
            "place recipe ignored for non-player container"
        );
        return Ok(());
    }
    if game_mode == GameMode::Survival && survival_state.is_dead() {
        debug!(
            recipe = packet.recipe_display_id,
            "place recipe ignored for dead player"
        );
        return Ok(());
    }
    let Some(recipe) = packet
        .recipe_display_id
        .try_into()
        .ok()
        .and_then(|idx: usize| state.recipes.get(idx).cloned())
    else {
        debug!(
            recipe = packet.recipe_display_id,
            "place recipe ignored: unknown recipe display id"
        );
        return Ok(());
    };

    if let Some(changed) = craft_recipe(state, &recipe, packet.use_max_items) {
        write_inventory_slot_updates(state, writer, changed).await?;
    } else {
        debug!(recipe = %recipe.id, "place recipe ignored: missing ingredients or output space");
    }
    Ok(())
}

async fn write_inventory_slot_updates<W>(
    state: &mut InteractionState,
    writer: &mut W,
    changed: Vec<(usize, ItemStack)>,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    for (slot, item_stack) in changed {
        state.inventory_state_id = state.inventory_state_id.wrapping_add(1);
        write_packet(
            writer,
            &ClientboundContainerSetSlot {
                container_id: 0,
                state_id: state.inventory_state_id,
                slot: slot as i16,
                item_stack,
            },
            state.compression,
        )
        .await?;
    }
    Ok(())
}

async fn write_inventory_content<W>(
    state: &mut InteractionState,
    writer: &mut W,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    state.inventory_state_id = state.inventory_state_id.wrapping_add(1);
    write_packet(
        writer,
        &ClientboundContainerSetContent {
            container_id: 0,
            state_id: state.inventory_state_id,
            items: state.inventory.as_wire_list(),
            carried_item: state.carried_item.clone(),
        },
        state.compression,
    )
    .await
}

fn take_from_slot(slot: &mut ItemStack, count: i32) -> ItemStack {
    if slot.is_empty() || count <= 0 {
        return ItemStack::EMPTY;
    }
    let moved = slot.count.min(count);
    let mut out = slot.clone();
    out.count = moved;
    slot.count -= moved;
    if slot.count <= 0 {
        *slot = ItemStack::EMPTY;
    }
    out
}

fn decrement_cursor(cursor: &mut ItemStack) {
    cursor.count -= 1;
    if cursor.count <= 0 {
        *cursor = ItemStack::EMPTY;
    }
}

fn apply_pickup_click(state: &mut InteractionState, slot: usize, button: i8) -> bool {
    if slot >= state.inventory.slots.len() || !(button == 0 || button == 1) {
        return false;
    }

    let slot_stack = state.inventory.slots[slot].clone();
    let cursor = state.carried_item.clone();
    if button == 0 {
        if cursor.is_empty() {
            if slot_stack.is_empty() {
                return false;
            }
            state.carried_item = std::mem::take(&mut state.inventory.slots[slot]);
        } else if slot_stack.is_empty() {
            if !can_place_in_player_slot(state, slot, &cursor) {
                return false;
            }
            state.inventory.slots[slot] = std::mem::take(&mut state.carried_item);
        } else if can_stack(&slot_stack, &cursor)
            && can_place_in_player_slot(state, slot, &cursor)
            && slot_stack.count < item_max_stack(&state.items, &cursor)
        {
            let max_stack = item_max_stack(&state.items, &cursor);
            let moved =
                (max_stack - state.inventory.slots[slot].count).min(state.carried_item.count);
            if moved <= 0 {
                return false;
            }
            state.inventory.slots[slot].count += moved;
            state.carried_item.count -= moved;
            if state.carried_item.count <= 0 {
                state.carried_item = ItemStack::EMPTY;
            }
        } else {
            if !can_place_in_player_slot(state, slot, &cursor) {
                return false;
            }
            state.inventory.slots[slot] = cursor;
            state.carried_item = slot_stack;
        }
        return true;
    }

    if cursor.is_empty() {
        if slot_stack.is_empty() {
            return false;
        }
        let moved = (slot_stack.count + 1) / 2;
        state.carried_item = take_from_slot(&mut state.inventory.slots[slot], moved);
    } else if slot_stack.is_empty() {
        if !can_place_in_player_slot(state, slot, &cursor) {
            return false;
        }
        let mut one = cursor;
        one.count = 1;
        state.inventory.slots[slot] = one;
        decrement_cursor(&mut state.carried_item);
    } else if can_stack(&slot_stack, &cursor)
        && can_place_in_player_slot(state, slot, &cursor)
        && slot_stack.count < item_max_stack(&state.items, &cursor)
    {
        state.inventory.slots[slot].count += 1;
        decrement_cursor(&mut state.carried_item);
    } else {
        return false;
    }
    true
}

fn can_place_in_player_slot(state: &InteractionState, slot: usize, stack: &ItemStack) -> bool {
    if stack.is_empty() {
        return true;
    }
    match slot {
        5..=8 => armor_entry_for_item(&state.items, stack.item_id)
            .is_some_and(|entry| armor_slot_for_kind(entry.slot) == slot),
        _ => true,
    }
}

fn apply_quick_move_click(state: &mut InteractionState, slot: usize) -> bool {
    if slot >= state.inventory.slots.len() || state.inventory.slots[slot].is_empty() {
        return false;
    }

    let original = state.inventory.slots[slot].clone();
    let max_stack = item_max_stack(&state.items, &original);
    if !(5..=8).contains(&slot)
        && let Some(entry) = armor_entry_for_item(&state.items, original.item_id)
    {
        let armor_slot = armor_slot_for_kind(entry.slot);
        if state.inventory.slots[armor_slot].is_empty() {
            let mut equipped = original.clone();
            equipped.count = 1;
            state.inventory.slots[armor_slot] = equipped;
            if original.count <= 1 {
                state.inventory.slots[slot] = ItemStack::EMPTY;
            } else {
                state.inventory.slots[slot].count -= 1;
            }
            return true;
        }
    }

    state.inventory.slots[slot] = ItemStack::EMPTY;
    let remaining = if (36..=44).contains(&slot) {
        state
            .inventory
            .merge_stack_into_ranges(original.clone(), &[9..=35], max_stack)
    } else {
        state
            .inventory
            .merge_stack_into_ranges(original.clone(), &[36..=44, 9..=35], max_stack)
    };
    state.inventory.slots[slot] = remaining;
    state.inventory.slots[slot] != original
}

fn furnace_player_slot(menu_slot: usize) -> Option<usize> {
    match menu_slot {
        3..=29 => Some(9 + (menu_slot - 3)),
        30..=38 => Some(36 + (menu_slot - 30)),
        _ => None,
    }
}

fn furnace_menu_stack(
    furnace: &FurnaceBlockEntity,
    inventory: &PlayerInventory,
    menu_slot: usize,
) -> Option<ItemStack> {
    match menu_slot {
        0..=2 => Some(furnace_slot_to_stack(&furnace.slots[menu_slot])),
        _ => furnace_player_slot(menu_slot).map(|slot| inventory.slots[slot].clone()),
    }
}

fn set_furnace_menu_stack(
    furnace: &mut FurnaceBlockEntity,
    inventory: &mut PlayerInventory,
    menu_slot: usize,
    stack: ItemStack,
) -> bool {
    match menu_slot {
        0..=2 => {
            furnace.slots[menu_slot] = stack_to_furnace_slot(&stack);
            true
        }
        _ => {
            let Some(slot) = furnace_player_slot(menu_slot) else {
                return false;
            };
            inventory.slots[slot] = stack;
            true
        }
    }
}

fn can_place_in_furnace_menu_slot(
    state: &InteractionState,
    menu_slot: usize,
    stack: &ItemStack,
) -> bool {
    if stack.is_empty() {
        return true;
    }
    match menu_slot {
        0 => find_smelting_recipe_for_item(state, stack.item_id).is_some(),
        1 => is_fuel_item(state, stack.item_id),
        2 => false,
        3..=38 => true,
        _ => false,
    }
}

fn apply_furnace_pickup_click(
    state: &mut InteractionState,
    furnace: &mut FurnaceBlockEntity,
    menu_slot: usize,
    button: i8,
) -> bool {
    if menu_slot >= FURNACE_MENU_SLOT_COUNT || !(button == 0 || button == 1) {
        return false;
    }
    let Some(slot_stack) = furnace_menu_stack(furnace, &state.inventory, menu_slot) else {
        return false;
    };
    let cursor = state.carried_item.clone();
    let max_stack = if cursor.is_empty() {
        item_max_stack(&state.items, &slot_stack)
    } else {
        item_max_stack(&state.items, &cursor)
    };

    if menu_slot == 2 && !slot_stack.is_empty() {
        if cursor.is_empty() {
            state.carried_item = slot_stack;
            return set_furnace_menu_stack(
                furnace,
                &mut state.inventory,
                menu_slot,
                ItemStack::EMPTY,
            );
        }
        if can_stack(&cursor, &slot_stack) && cursor.count < max_stack {
            let moved = (max_stack - state.carried_item.count).min(slot_stack.count);
            state.carried_item.count += moved;
            let mut remaining = slot_stack;
            remaining.count -= moved;
            if remaining.count <= 0 {
                remaining = ItemStack::EMPTY;
            }
            return set_furnace_menu_stack(furnace, &mut state.inventory, menu_slot, remaining);
        }
        return false;
    }

    if button == 0 {
        if cursor.is_empty() {
            if slot_stack.is_empty() {
                return false;
            }
            state.carried_item = slot_stack;
            set_furnace_menu_stack(furnace, &mut state.inventory, menu_slot, ItemStack::EMPTY)
        } else if slot_stack.is_empty() {
            if !can_place_in_furnace_menu_slot(state, menu_slot, &cursor) {
                return false;
            }
            state.carried_item = ItemStack::EMPTY;
            set_furnace_menu_stack(furnace, &mut state.inventory, menu_slot, cursor)
        } else if can_stack(&slot_stack, &cursor)
            && can_place_in_furnace_menu_slot(state, menu_slot, &cursor)
            && slot_stack.count < max_stack
        {
            let moved = (max_stack - slot_stack.count).min(cursor.count);
            let mut new_slot = slot_stack;
            new_slot.count += moved;
            state.carried_item.count -= moved;
            if state.carried_item.count <= 0 {
                state.carried_item = ItemStack::EMPTY;
            }
            set_furnace_menu_stack(furnace, &mut state.inventory, menu_slot, new_slot)
        } else {
            if !can_place_in_furnace_menu_slot(state, menu_slot, &cursor) {
                return false;
            }
            state.carried_item = slot_stack;
            set_furnace_menu_stack(furnace, &mut state.inventory, menu_slot, cursor)
        }
    } else if cursor.is_empty() {
        if slot_stack.is_empty() {
            return false;
        }
        let moved = (slot_stack.count + 1) / 2;
        let mut new_cursor = slot_stack.clone();
        new_cursor.count = moved;
        let mut remaining = slot_stack;
        remaining.count -= moved;
        if remaining.count <= 0 {
            remaining = ItemStack::EMPTY;
        }
        state.carried_item = new_cursor;
        set_furnace_menu_stack(furnace, &mut state.inventory, menu_slot, remaining)
    } else if slot_stack.is_empty() {
        if !can_place_in_furnace_menu_slot(state, menu_slot, &cursor) {
            return false;
        }
        let mut one = cursor;
        one.count = 1;
        decrement_cursor(&mut state.carried_item);
        set_furnace_menu_stack(furnace, &mut state.inventory, menu_slot, one)
    } else if can_stack(&slot_stack, &cursor)
        && can_place_in_furnace_menu_slot(state, menu_slot, &cursor)
        && slot_stack.count < max_stack
    {
        let mut new_slot = slot_stack;
        new_slot.count += 1;
        decrement_cursor(&mut state.carried_item);
        set_furnace_menu_stack(furnace, &mut state.inventory, menu_slot, new_slot)
    } else {
        false
    }
}

fn merge_stack_into_furnace_slot(
    state: &InteractionState,
    furnace: &mut FurnaceBlockEntity,
    menu_slot: usize,
    stack: ItemStack,
) -> ItemStack {
    if stack.is_empty() || !can_place_in_furnace_menu_slot(state, menu_slot, &stack) {
        return stack;
    }
    let target = &mut furnace.slots[menu_slot];
    let max_stack = item_max_stack(&state.items, &stack);
    if target.is_empty() {
        let moved = stack.count.min(max_stack);
        let mut moved_stack = stack.clone();
        moved_stack.count = moved;
        *target = stack_to_furnace_slot(&moved_stack);
        let mut remaining = stack;
        remaining.count -= moved;
        if remaining.count <= 0 {
            ItemStack::EMPTY
        } else {
            remaining
        }
    } else if can_stack(&furnace_slot_to_stack(target), &stack) && target.count < max_stack {
        let moved = (max_stack - target.count).min(stack.count);
        target.count += moved;
        let mut remaining = stack;
        remaining.count -= moved;
        if remaining.count <= 0 {
            ItemStack::EMPTY
        } else {
            remaining
        }
    } else {
        stack
    }
}

fn apply_furnace_quick_move_click(
    state: &mut InteractionState,
    furnace: &mut FurnaceBlockEntity,
    menu_slot: usize,
) -> bool {
    if menu_slot >= FURNACE_MENU_SLOT_COUNT {
        return false;
    }
    match menu_slot {
        0..=2 => {
            let original = furnace_slot_to_stack(&furnace.slots[menu_slot]);
            if original.is_empty() {
                return false;
            }
            let (remaining, _) = state.inventory.merge_stack(original.clone());
            furnace.slots[menu_slot] = stack_to_furnace_slot(&remaining);
            remaining != original
        }
        _ => {
            let Some(player_slot) = furnace_player_slot(menu_slot) else {
                return false;
            };
            let original = state.inventory.slots[player_slot].clone();
            if original.is_empty() {
                return false;
            }
            let target = if find_smelting_recipe_for_item(state, original.item_id).is_some() {
                Some(0)
            } else if is_fuel_item(state, original.item_id) {
                Some(1)
            } else {
                None
            };
            let Some(target) = target else {
                return false;
            };
            state.inventory.slots[player_slot] = ItemStack::EMPTY;
            let remaining = merge_stack_into_furnace_slot(state, furnace, target, original.clone());
            state.inventory.slots[player_slot] = remaining;
            state.inventory.slots[player_slot] != original
        }
    }
}

async fn handle_furnace_container_click<W>(
    state: &mut InteractionState,
    writer: &mut W,
    mut window: FurnaceWindow,
    packet: ServerboundContainerClick,
) -> Result<FurnaceWindow, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let mut furnace = {
        let mut storage = state.world.lock().await;
        storage
            .furnace_block_entity(window.position)
            .map_err(|err| {
                warn!(error = %err, ?window.position, "furnace state read failed");
                err
            })?
    }
    .unwrap_or_default();
    if packet.state_id != window.state_id {
        write_furnace_content(state, writer, &window, &furnace).await?;
        return Ok(window);
    }
    let changed = match packet.container_input {
        ContainerInput::Pickup if packet.slot_num >= 0 => apply_furnace_pickup_click(
            state,
            &mut furnace,
            packet.slot_num as usize,
            packet.button_num,
        ),
        ContainerInput::QuickMove if packet.slot_num >= 0 && packet.button_num == 0 => {
            apply_furnace_quick_move_click(state, &mut furnace, packet.slot_num as usize)
        }
        _ => false,
    };
    if changed {
        window.state_id = window.state_id.wrapping_add(1);
        let mut storage = state.world.lock().await;
        storage
            .set_furnace_block_entity(window.position, furnace.clone())
            .map_err(|err| {
                warn!(error = %err, ?window.position, "furnace state write failed");
                err
            })?;
        dispatch_visibility_commands(state.sessions.furnace_slot_dispatches(
            window.position,
            state.session_id,
            furnace_slot_stacks(&furnace),
        ));
    }
    write_furnace_content(state, writer, &window, &furnace).await?;
    Ok(window)
}

fn furnace_output_room(furnace: &FurnaceBlockEntity, item_id: u32, count: i32) -> bool {
    let output = furnace_slot_to_stack(&furnace.slots[2]);
    output.is_empty()
        || output.item_id == item_id && output.damage.is_none() && output.count + count <= 64
}

fn add_furnace_output(furnace: &mut FurnaceBlockEntity, item_id: u32, count: i32) {
    if furnace.slots[2].is_empty() {
        furnace.slots[2] = stack_to_furnace_slot(&ItemStack::new(item_id, count));
    } else {
        furnace.slots[2].count += count;
    }
}

fn decrement_furnace_slot(stack: &mut FurnaceSlot) {
    stack.count -= 1;
    if stack.count <= 0 {
        *stack = FurnaceSlot::EMPTY;
    }
}

fn tick_furnace(
    state: &InteractionState,
    furnace: &mut FurnaceBlockEntity,
) -> (bool, Vec<(i16, i16)>) {
    let before_slots = furnace.slots.clone();
    let before_data = furnace_data_values(furnace);

    if furnace.burn_remaining > 0 {
        furnace.burn_remaining -= 1;
    }

    let input = furnace_slot_to_stack(&furnace.slots[0]);
    let recipe = (!input.is_empty())
        .then(|| find_smelting_recipe_for_item(state, input.item_id))
        .flatten();
    let Some(recipe) = recipe else {
        furnace.cook_progress = 0;
        let changed_data = changed_furnace_data(before_data, furnace_data_values(furnace));
        return (furnace.slots != before_slots, changed_data);
    };
    let Some(output_item_id) = state.items.id_of(&recipe.result.item) else {
        furnace.cook_progress = 0;
        let changed_data = changed_furnace_data(before_data, furnace_data_values(furnace));
        return (furnace.slots != before_slots, changed_data);
    };
    let output_count = i32::try_from(recipe.result.count).unwrap_or(0);
    let cooking_time = match &recipe.kind {
        mc_data::recipes::RecipeKind::Smelting(smelting) => smelting.cooking_time,
        _ => DEFAULT_FURNACE_COOK_TICKS as u32,
    };
    furnace.cook_total = i16::try_from(cooking_time)
        .unwrap_or(DEFAULT_FURNACE_COOK_TICKS)
        .max(1);

    if output_count <= 0 || !furnace_output_room(furnace, output_item_id, output_count) {
        furnace.cook_progress = 0;
        let changed_data = changed_furnace_data(before_data, furnace_data_values(furnace));
        return (furnace.slots != before_slots, changed_data);
    }

    if furnace.burn_remaining <= 0
        && !furnace.slots[1].is_empty()
        && is_fuel_item(state, furnace.slots[1].item_id)
    {
        decrement_furnace_slot(&mut furnace.slots[1]);
        furnace.burn_total = FURNACE_FUEL_TICKS;
        furnace.burn_remaining = FURNACE_FUEL_TICKS;
    }

    if furnace.burn_remaining > 0 {
        furnace.cook_progress += 1;
        if furnace.cook_progress >= furnace.cook_total {
            decrement_furnace_slot(&mut furnace.slots[0]);
            add_furnace_output(furnace, output_item_id, output_count);
            furnace.cook_progress = 0;
        }
    } else {
        furnace.cook_progress = 0;
    }

    let changed_data = changed_furnace_data(before_data, furnace_data_values(furnace));
    (furnace.slots != before_slots, changed_data)
}

fn changed_furnace_data(before: [(i16, i16); 4], after: [(i16, i16); 4]) -> Vec<(i16, i16)> {
    before
        .into_iter()
        .zip(after)
        .filter_map(|(before, after)| (before != after).then_some(after))
        .collect()
}

async fn tick_active_container<W>(
    state: &mut InteractionState,
    writer: &mut W,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some(active) = state.active_container.take() else {
        return Ok(());
    };
    match active {
        ActiveContainer::Furnace(mut window) => {
            if !state
                .sessions
                .is_furnace_tick_owner(window.position, state.session_id)
            {
                state.active_container = Some(ActiveContainer::Furnace(window));
                return Ok(());
            }
            let mut furnace = {
                let mut storage = state.world.lock().await;
                storage
                    .furnace_block_entity(window.position)
                    .map_err(|err| {
                        warn!(error = %err, ?window.position, "furnace state read failed");
                        err
                    })?
            }
            .unwrap_or_default();
            let (slots_changed, data_changed) = tick_furnace(state, &mut furnace);
            if slots_changed {
                window.state_id = window.state_id.wrapping_add(1);
                write_furnace_content(state, writer, &window, &furnace).await?;
            }
            if !data_changed.is_empty() {
                write_furnace_data_changes(writer, state.compression, &window, &data_changed)
                    .await?;
            }
            if slots_changed || !data_changed.is_empty() {
                let mut storage = state.world.lock().await;
                storage
                    .set_furnace_block_entity(window.position, furnace.clone())
                    .map_err(|err| {
                        warn!(error = %err, ?window.position, "furnace state write failed");
                        err
                    })?;
            }
            if slots_changed {
                dispatch_visibility_commands(state.sessions.furnace_slot_dispatches(
                    window.position,
                    state.session_id,
                    furnace_slot_stacks(&furnace),
                ));
            }
            if !data_changed.is_empty() {
                dispatch_visibility_commands(state.sessions.furnace_data_dispatches(
                    window.position,
                    state.session_id,
                    data_changed,
                ));
            }
            state.active_container = Some(ActiveContainer::Furnace(window));
        }
        ActiveContainer::CraftingTable(window) => {
            state.active_container = Some(ActiveContainer::CraftingTable(window));
        }
    }
    Ok(())
}

async fn handle_container_click<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    survival_state: SurvivalState,
    packet: ServerboundContainerClick,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    state.pending_break = None;
    state.pending_use = None;
    if game_mode == GameMode::Spectator
        || matches!(game_mode, GameMode::Survival | GameMode::Adventure) && survival_state.is_dead()
    {
        if packet.container_id == 0 {
            write_inventory_content(state, writer).await?;
        } else if let Some(active) = state.active_container.take() {
            match active {
                ActiveContainer::Furnace(window) => {
                    let furnace = {
                        let mut storage = state.world.lock().await;
                        storage
                            .furnace_block_entity(window.position)
                            .map_err(|err| {
                                warn!(error = %err, ?window.position, "furnace state read failed");
                                err
                            })?
                    }
                    .unwrap_or_default();
                    write_furnace_content(state, writer, &window, &furnace).await?;
                    state.active_container = Some(ActiveContainer::Furnace(window));
                }
                ActiveContainer::CraftingTable(window) => {
                    write_crafting_content(state, writer, &window).await?;
                    state.active_container = Some(ActiveContainer::CraftingTable(window));
                }
            }
        }
        return Ok(());
    }

    if packet.container_id != 0 {
        let Some(active) = state.active_container.take() else {
            write_inventory_content(state, writer).await?;
            return Ok(());
        };
        match active {
            ActiveContainer::CraftingTable(crafting)
                if crafting.container_id == packet.container_id =>
            {
                let crafting =
                    handle_crafting_container_click(state, writer, crafting, packet).await?;
                state.active_container = Some(ActiveContainer::CraftingTable(crafting));
            }
            ActiveContainer::Furnace(furnace) if furnace.container_id == packet.container_id => {
                let furnace =
                    handle_furnace_container_click(state, writer, furnace, packet).await?;
                state.active_container = Some(ActiveContainer::Furnace(furnace));
            }
            other => {
                debug!(
                    container_id = packet.container_id,
                    active_id = other.container_id(),
                    "container click for inactive container ignored"
                );
                state.active_container = Some(other);
            }
        }
        return Ok(());
    }

    if packet.state_id != state.inventory_state_id {
        debug!(
            client_state = packet.state_id,
            server_state = state.inventory_state_id,
            "container click resynced stale state"
        );
        write_inventory_content(state, writer).await?;
        return Ok(());
    }

    let changed = match packet.container_input {
        ContainerInput::Pickup if packet.slot_num >= 0 => {
            apply_pickup_click(state, packet.slot_num as usize, packet.button_num)
        }
        ContainerInput::QuickMove if packet.slot_num >= 0 && packet.button_num == 0 => {
            apply_quick_move_click(state, packet.slot_num as usize)
        }
        _ => false,
    };

    if !changed {
        debug!(
            slot = packet.slot_num,
            button = packet.button_num,
            input = ?packet.container_input,
            "container click unsupported or no-op; resyncing"
        );
    }
    write_inventory_content(state, writer).await
}

async fn pickup_nearby_items<W>(
    state: &mut InteractionState,
    writer: &mut W,
    player_pose: PlayerPose,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let player_position = Vec3::new(player_pose.x, player_pose.y, player_pose.z);
    let candidates = state.sessions.nearby_item_entities(player_position, 2.25);
    for entity in candidates {
        let Some(stack) = entity.item_stack else {
            continue;
        };
        let original_count = stack.count;
        let (remaining, changed) = state
            .inventory
            .merge_pickup_stack(ItemStack::new(stack.item_id, stack.count));
        if changed.is_empty() {
            continue;
        }
        write_inventory_slot_updates(state, writer, changed).await?;
        if remaining.is_empty() {
            dispatch_visibility_commands(state.sessions.remove_picked_item(
                entity.id,
                state.session_id,
                original_count,
            ));
        } else {
            dispatch_visibility_commands(
                state
                    .sessions
                    .update_item_stack(entity.id, entity_item_stack(remaining)),
            );
        }
    }
    Ok(())
}

async fn handle_interact(
    state: &mut InteractionState,
    packet: ServerboundInteract,
) -> Result<(), ConnectionError> {
    state.pending_break = None;
    debug!(entity_id = packet.entity_id, "entity interaction ignored");
    Ok(())
}

async fn handle_attack<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    survival_state: SurvivalState,
    player_pose: PlayerPose,
    packet: ServerboundAttack,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    state.pending_break = None;
    if game_mode == GameMode::Survival && survival_state.is_dead() {
        debug!(
            entity_id = packet.entity_id,
            "entity attack ignored for dead player"
        );
        return Ok(());
    }
    if matches!(game_mode, GameMode::Spectator) {
        debug!(
            entity_id = packet.entity_id,
            "entity attack ignored in spectator"
        );
        return Ok(());
    }

    let entity_id = EntityId(packet.entity_id);
    let Some(entity) = state.sessions.server_entity_snapshot(entity_id) else {
        debug!(
            entity_id = packet.entity_id,
            "entity attack ignored for unknown entity"
        );
        return Ok(());
    };
    if game_mode == GameMode::Survival
        && !within_entity_reach(player_pose, entity.position, game_mode)
    {
        debug!(
            entity_id = packet.entity_id,
            "survival entity attack ignored: target out of reach"
        );
        return Ok(());
    }
    if entity.item_stack.is_some() {
        return Ok(());
    }
    dispatch_visibility_commands(state.sessions.broadcast_player_animation(state.session_id));

    let Some(damage) = state
        .sessions
        .damage_server_entity(entity_id, held_attack_damage(state))
    else {
        debug!(
            entity_id = packet.entity_id,
            "entity attack ignored for non-living entity"
        );
        return Ok(());
    };
    if !damage.killed {
        debug!(
            entity_id = packet.entity_id,
            health = damage.snapshot.health,
            "entity attack damaged target"
        );
        return Ok(());
    }

    let Some((entity, despawn)) = state.sessions.remove_server_entity(entity_id) else {
        debug!(
            entity_id = packet.entity_id,
            "killed entity disappeared before despawn"
        );
        return Ok(());
    };
    dispatch_visibility_commands(despawn);

    if let (Some(drop), Some(entity_type_id)) = (
        passive_mob_drop_stack(state, &entity.type_name),
        item_entity_type_id(&state.entity_types),
    ) {
        dispatch_visibility_commands(state.sessions.spawn_item_drop(
            entity_type_id,
            entity.position,
            entity_item_stack(drop),
        ));
        pickup_nearby_items(state, writer, player_pose).await?;
    }
    Ok(())
}

async fn complete_block_break<W>(
    state: &mut InteractionState,
    writer: &mut W,
    sequence: i32,
    position: i64,
    drop_items: bool,
    player_pose: PlayerPose,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let (x, y, z) = unpack_block_pos(position);
    let air = air_state_id(&state.blocks);
    let replacement = break_replacement_state(state, x, y, z, air).await;

    dispatch_visibility_commands(state.sessions.broadcast_player_animation(state.session_id));
    let prev = apply_block_edit(state, writer, sequence, x, y, z, replacement).await?;
    let changed = prev.is_some();
    if drop_items
        && let (Some(prev), Some(entity_type_id)) = (prev, item_entity_type_id(&state.entity_types))
        && let Some(drop) = block_drop_stack(state, prev)
    {
        dispatch_visibility_commands(state.sessions.spawn_item_drop(
            entity_type_id,
            Vec3::new(x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5),
            entity_item_stack(drop),
        ));
        pickup_nearby_items(state, writer, player_pose).await?;
    }
    Ok(changed)
}

/// M5.d/M22.b: handle serverbound block-destroy actions. Creative keeps the
/// historical instant edit path; survival now requires a server-timed start/stop
/// pair before the shared mutation back-half can run.
async fn handle_player_action<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    survival_state: SurvivalState,
    player_pose: PlayerPose,
    action: ServerboundPlayerAction,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let is_destroy = matches!(
        action.action,
        PlayerActionKind::StartDestroyBlock
            | PlayerActionKind::AbortDestroyBlock
            | PlayerActionKind::StopDestroyBlock
    );
    if matches!(action.action, PlayerActionKind::ReleaseUseItem) {
        state.pending_use = None;
        return write_block_ack(writer, state.compression, action.sequence).await;
    }
    if !is_destroy {
        // DROP_*, RELEASE_USE_ITEM, SWAP_ITEM_WITH_OFFHAND, STAB —
        // all out of scope for M5. Ack so the client doesn't hang
        // on a prediction.
        debug!(
            action = ?action.action,
            sequence = action.sequence,
            "non-destroy player action ignored"
        );
        write_block_ack(writer, state.compression, action.sequence).await?;
        return Ok(());
    }

    if game_mode == GameMode::Survival && survival_state.is_dead() {
        state.pending_break = None;
        state.pending_use = None;
        debug!(
            sequence = action.sequence,
            "survival block break ignored for dead player"
        );
        return write_block_ack(writer, state.compression, action.sequence).await;
    }

    if game_mode == GameMode::Survival
        && !within_block_reach(player_pose, action.position, game_mode)
    {
        state.pending_break = None;
        state.pending_use = None;
        debug!(
            sequence = action.sequence,
            "survival block break ignored: target out of reach"
        );
        return write_block_ack(writer, state.compression, action.sequence).await;
    }

    match game_mode {
        GameMode::Creative => {
            state.pending_break = None;
            state.pending_use = None;
            if matches!(action.action, PlayerActionKind::AbortDestroyBlock) {
                return write_block_ack(writer, state.compression, action.sequence).await;
            }
            complete_block_break(
                state,
                writer,
                action.sequence,
                action.position,
                false,
                player_pose,
            )
            .await
            .map(|_| ())
        }
        GameMode::Survival => match action.action {
            PlayerActionKind::StartDestroyBlock => {
                let required_time = mining_time_for_target(state, action.position).await;
                state.pending_break = Some(PendingBreak {
                    position: action.position,
                    direction: action.direction,
                    started_at: Instant::now(),
                    required_time,
                    held_hotbar_slot: state.selected_hotbar_slot,
                    held_item_id: held_item_id(state),
                });
                write_block_ack(writer, state.compression, action.sequence).await
            }
            PlayerActionKind::AbortDestroyBlock => {
                state.pending_break = None;
                write_block_ack(writer, state.compression, action.sequence).await
            }
            PlayerActionKind::StopDestroyBlock => {
                let can_complete = state.pending_break.as_ref().is_some_and(|pending| {
                    pending_break_matches(state, pending, &action)
                        && pending_break_is_complete(pending, Instant::now())
                });
                state.pending_break = None;
                if can_complete {
                    let changed = complete_block_break(
                        state,
                        writer,
                        action.sequence,
                        action.position,
                        true,
                        player_pose,
                    )
                    .await?;
                    if changed {
                        damage_held_tool_after_mining(state, writer).await
                    } else {
                        Ok(())
                    }
                } else {
                    debug!(
                        sequence = action.sequence,
                        "survival block break rejected before completion"
                    );
                    write_block_ack(writer, state.compression, action.sequence).await
                }
            }
            _ => write_block_ack(writer, state.compression, action.sequence).await,
        },
        GameMode::Adventure | GameMode::Spectator => {
            state.pending_break = None;
            state.pending_use = None;
            debug!(
                mode = ?game_mode,
                sequence = action.sequence,
                "block break denied outside survival/creative"
            );
            write_block_ack(writer, state.compression, action.sequence).await
        }
    }
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
) -> Result<Option<mc_world::BlockStateId>, ConnectionError>
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
                return Ok(None);
            }
        }
    };

    // Absent chunk or no-op edit: ack the sequence so the client
    // doesn't roll back forever, but skip the BlockUpdate /
    // LightUpdate ripple.
    let Some(prev) = prev else {
        write_packet(writer, &BlockChangedAck { sequence }, state.compression).await?;
        return Ok(None);
    };
    if prev == new_state {
        write_packet(writer, &BlockChangedAck { sequence }, state.compression).await?;
        return Ok(None);
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
    //    burst populated it) or tests constructed a chunkless config.
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
    Ok(Some(prev))
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

async fn break_replacement_state(
    state: &InteractionState,
    x: i32,
    y: i32,
    z: i32,
    air: mc_world::BlockStateId,
) -> mc_world::BlockStateId {
    let Some(water) = state.water else {
        return air;
    };
    let mut storage = state.world.lock().await;
    let neighbours = [
        (x, y + 1, z),
        (x + 1, y, z),
        (x - 1, y, z),
        (x, y, z + 1),
        (x, y, z - 1),
    ];
    let neighbour_states = neighbours.map(|(x, y, z)| {
        storage
            .get_block(mc_world::BlockPos { x, y, z })
            .ok()
            .flatten()
    });
    break_replacement_from_neighbours(neighbour_states, air, water)
}

fn break_replacement_from_neighbours(
    neighbours: [Option<mc_world::BlockStateId>; 5],
    air: mc_world::BlockStateId,
    water: mc_world::BlockStateId,
) -> mc_world::BlockStateId {
    if neighbours.into_iter().any(|state| state == Some(water)) {
        water
    } else {
        air
    }
}

/// M6.f/M23 follow-up: handle a serverbound `UseItemOn`. Resolves the placed
/// block via the player's currently-held hotbar slot through the item→block
/// table. Drops the placement silently (still acking) if the held stack is
/// empty, if the held item has no block mapping (e.g. food, tool), or if the
/// target cell is non-air. On a successful placement decrements the held stack's
/// count and emits `ContainerSetSlot` so the client sees the new count.
async fn handle_use_item_on<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    survival_state: SurvivalState,
    player_pose: PlayerPose,
    action: ServerboundUseItemOn,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    state.pending_use = None;
    if game_mode == GameMode::Survival && survival_state.is_dead() {
        debug!(
            sequence = action.sequence,
            "survival block placement ignored for dead player"
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

    if !matches!(game_mode, GameMode::Creative | GameMode::Survival) {
        debug!(
            mode = ?game_mode,
            sequence = action.sequence,
            "block placement denied outside creative/survival"
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

    if game_mode == GameMode::Survival
        && !within_block_reach(player_pose, action.position, game_mode)
    {
        debug!(
            sequence = action.sequence,
            "survival block placement ignored: target out of reach"
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

    let (cx, cy, cz) = unpack_block_pos(action.position);
    if open_crafting_table_container(state, writer, action.sequence, cx, cy, cz).await? {
        return Ok(());
    }
    if open_furnace_container(state, writer, action.sequence, cx, cy, cz).await? {
        return Ok(());
    }

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

    let _ = apply_block_edit(state, writer, action.sequence, tx, ty, tz, placed_state).await?;
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

async fn handle_use_item<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    survival_state: &mut SurvivalState,
    action: ServerboundUseItem,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if game_mode != GameMode::Survival
        || action.hand != mc_protocol::packets::play::InteractionHand::MainHand
    {
        return write_block_ack(writer, state.compression, action.sequence).await;
    }
    if survival_state.is_dead() || survival_state.food >= SurvivalState::MAX_FOOD {
        return write_block_ack(writer, state.compression, action.sequence).await;
    }

    let Some((held_item_id, rule)) = held_food_use(state) else {
        return write_block_ack(writer, state.compression, action.sequence).await;
    };

    state.pending_break = None;
    state.pending_use = Some(PendingUse {
        started_at: Instant::now(),
        required_time: DEFAULT_FOOD_USE_DURATION,
        held_hotbar_slot: state.selected_hotbar_slot,
        held_item_id,
        rule,
    });
    write_block_ack(writer, state.compression, action.sequence).await
}

async fn complete_food_use<W>(
    state: &mut InteractionState,
    writer: &mut W,
    survival_state: &mut SurvivalState,
    pending: PendingUse,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if survival_state.is_dead()
        || survival_state.food >= SurvivalState::MAX_FOOD
        || !pending_use_matches(state, &pending)
    {
        return Ok(());
    }

    survival_state.add_food(pending.rule.food, pending.rule.saturation);
    let held_slot = state.selected_hotbar_slot;
    {
        let held = state.inventory.held_mut(held_slot);
        held.count = held.count.saturating_sub(1);
        if held.count <= 0 {
            *held = ItemStack::EMPTY;
        }
    }
    let slot = PlayerInventory::HOTBAR_BASE + held_slot as usize;
    let slot_value = state.inventory.held(held_slot).clone();
    write_inventory_slot_updates(state, writer, vec![(slot, slot_value)]).await?;
    write_packet(writer, &survival_state.as_packet(), state.compression).await
}

async fn tick_pending_use<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    survival_state: &mut SurvivalState,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if game_mode != GameMode::Survival {
        state.pending_use = None;
        return Ok(());
    }
    let Some(pending) = state.pending_use else {
        return Ok(());
    };
    if survival_state.is_dead() || !pending_use_matches(state, &pending) {
        state.pending_use = None;
        return Ok(());
    }
    if pending_use_is_complete(&pending, Instant::now()) {
        state.pending_use = None;
        complete_food_use(state, writer, survival_state, pending).await?;
    }
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
    send_entity_data(writer, compression, entity).await?;
    send_entity_move(writer, compression, entity).await
}

async fn send_entity_data<W>(
    writer: &mut W,
    compression: Compression,
    entity: &ServerEntitySnapshot,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some(stack) = entity.item_stack else {
        return Ok(());
    };
    write_packet(
        writer,
        &ClientboundSetEntityData {
            entity_id: entity.id.0,
            values: vec![EntityDataValue::ItemStack {
                index: ITEM_ENTITY_DATA_ITEM_INDEX,
                stack: ItemStack::new(stack.item_id, stack.count),
            }],
        },
        compression,
    )
    .await
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

async fn send_take_item_entity<W>(
    writer: &mut W,
    compression: Compression,
    item_entity_id: i32,
    player_entity_id: i32,
    amount: i32,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &ClientboundTakeItemEntity {
            item_entity_id,
            player_entity_id,
            amount,
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
    respawn_pose: PlayerPose,
    respawn: ClientboundRespawn,
    permissions: CommandPermissions,
    mut survival_state: SurvivalState,
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
    let mut survival_ticker = interval(Duration::from_secs(1));
    survival_ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    survival_ticker.tick().await;
    let mut furnace_ticker = interval(ENTITY_TICK_PERIOD);
    furnace_ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    furnace_ticker.tick().await;

    let mut next_id: i64 = 0;
    let mut last_response_at = Instant::now();
    let mut pending_id: Option<i64> = None;
    let mut survival_tick: u32 = 0;
    let mut game_mode = GameMode::Survival;
    write_packet(writer, &survival_state.as_packet(), compression).await?;

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
                    Some(OutboundCommand::UpdateEntityData(entity)) => {
                        send_entity_data(writer, compression, &entity).await?;
                    }
                    Some(OutboundCommand::MoveEntityRelative(movement)) => {
                        send_entity_relative_move(writer, compression, &movement).await?;
                    }
                    Some(OutboundCommand::TakeItemEntity {
                        item_entity_id,
                        player_entity_id,
                        amount,
                    }) => {
                        send_take_item_entity(
                            writer,
                            compression,
                            item_entity_id,
                            player_entity_id,
                            amount,
                        ).await?;
                    }
                    Some(OutboundCommand::DespawnEntity(entity)) => {
                        send_entity_despawn(writer, compression, &entity).await?;
                    }
                    Some(OutboundCommand::AnimatePlayer { entity_id }) => {
                        send_player_animation(writer, compression, entity_id).await?;
                    }
                    Some(OutboundCommand::FurnaceSlots { position, slots }) => {
                        if let Some(state) = interaction.as_deref_mut()
                            && let Some(ActiveContainer::Furnace(mut window)) = state.active_container.take()
                        {
                            if window.position == position {
                                window.state_id = window.state_id.wrapping_add(1);
                                for (slot, item_stack) in slots.into_iter().enumerate() {
                                    write_packet(
                                        writer,
                                        &ClientboundContainerSetSlot {
                                            container_id: window.container_id,
                                            state_id: window.state_id,
                                            slot: slot as i16,
                                            item_stack,
                                        },
                                        compression,
                                    )
                                    .await?;
                                }
                            }
                            state.active_container = Some(ActiveContainer::Furnace(window));
                        }
                    }
                    Some(OutboundCommand::FurnaceData { position, changed }) => {
                        if let Some(state) = interaction.as_deref_mut()
                            && let Some(ActiveContainer::Furnace(window)) = state.active_container.as_ref()
                            && window.position == position
                        {
                            write_furnace_data_changes(writer, compression, window, &changed).await?;
                        }
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
            _ = survival_ticker.tick() => {
                if game_mode == GameMode::Survival {
                    survival_tick = survival_tick.wrapping_add(1);
                    if survival_state.tick_health(survival_tick) {
                        if survival_state.is_dead()
                            && let Some(state) = interaction.as_deref_mut()
                        {
                            state.pending_break = None;
                            state.pending_use = None;
                        }
                        write_packet(writer, &survival_state.as_packet(), compression).await?;
                    }
                }
            }
            _ = furnace_ticker.tick() => {
                if let Some(state) = interaction.as_deref_mut() {
                    tick_active_container(state, writer).await?;
                    tick_pending_use(state, writer, game_mode, &mut survival_state).await?;
                }
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
                        handle_player_action(
                            state,
                            writer,
                            game_mode,
                            survival_state,
                            player_pose,
                            action,
                        )
                        .await?;
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
                        handle_use_item_on(
                            state,
                            writer,
                            game_mode,
                            survival_state,
                            player_pose,
                            use_on,
                        )
                        .await?;
                    } else {
                        debug!(
                            sequence = use_on.sequence,
                            "UseItemOn ignored — no world configured"
                        );
                    }
                } else if frame.id == ServerboundUseItem::ID {
                    let mut body = frame.body;
                    let use_item = ServerboundUseItem::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        handle_use_item(state, writer, game_mode, &mut survival_state, use_item)
                            .await?;
                    } else {
                        debug!(
                            sequence = use_item.sequence,
                            "UseItem ignored — no world configured"
                        );
                    }
                } else if frame.id == ServerboundAttack::ID {
                    let mut body = frame.body;
                    let attack = ServerboundAttack::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        handle_attack(state, writer, game_mode, survival_state, player_pose, attack)
                            .await?;
                    } else {
                        debug!(
                            entity_id = attack.entity_id,
                            "Attack ignored — no world configured"
                        );
                    }
                } else if frame.id == ServerboundInteract::ID {
                    let mut body = frame.body;
                    let interact = ServerboundInteract::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        handle_interact(state, interact).await?;
                    } else {
                        debug!(
                            entity_id = interact.entity_id,
                            "Interact ignored — no world configured"
                        );
                    }
                } else if frame.id == ServerboundPlaceRecipe::ID {
                    let mut body = frame.body;
                    let recipe = ServerboundPlaceRecipe::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        handle_place_recipe(state, writer, game_mode, survival_state, recipe).await?;
                    } else {
                        debug!(
                            recipe = recipe.recipe_display_id,
                            "PlaceRecipe ignored — no world configured"
                        );
                    }
                } else if frame.id == ServerboundContainerClick::ID {
                    let mut body = frame.body;
                    let click = ServerboundContainerClick::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        handle_container_click(state, writer, game_mode, survival_state, click).await?;
                    } else {
                        debug!(
                            container_id = click.container_id,
                            slot = click.slot_num,
                            "ContainerClick ignored — no world configured"
                        );
                    }
                } else if frame.id == ServerboundContainerClose::ID {
                    let mut body = frame.body;
                    let close = ServerboundContainerClose::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        let should_store = state
                                .active_container
                                .as_ref()
                                .is_some_and(|active| active.container_id() == close.container_id);
                        if should_store {
                            store_active_container(state);
                        }
                    }
                    debug!(container_id = close.container_id, "container close acknowledged");
                } else if frame.id == ServerboundRecipeBookChangeSettings::ID {
                    let mut body = frame.body;
                    let settings = ServerboundRecipeBookChangeSettings::decode(&mut body)?;
                    debug!(
                        book_type = ?settings.book_type,
                        open = settings.is_open,
                        filtering = settings.is_filtering,
                        "recipe book settings noted"
                    );
                } else if frame.id == ServerboundRecipeBookSeenRecipe::ID {
                    let mut body = frame.body;
                    let seen = ServerboundRecipeBookSeenRecipe::decode(&mut body)?;
                    debug!(recipe = seen.recipe_display_id, "recipe book seen recipe noted");
                } else if frame.id == ServerboundSetCarriedItem::ID {
                    let mut body = frame.body;
                    let pick = ServerboundSetCarriedItem::decode(&mut body)?;
                    let slot = pick.slot.clamp(0, 8) as u8;
                    if let Some(state) = interaction.as_deref_mut() {
                        state.pending_break = None;
                        state.pending_use = None;
                        state.selected_hotbar_slot = slot;
                        debug!(slot, "hotbar selection updated");
                    }
                } else if frame.id == ServerboundClientCommand::ID {
                    let mut body = frame.body;
                    let command = ServerboundClientCommand::decode(&mut body)?;
                    handle_client_command(
                        writer,
                        compression,
                        interaction.as_deref_mut(),
                        &mut player_pose,
                        respawn_pose,
                        &mut survival_state,
                        &respawn,
                        command,
                    )
                    .await?;
                    dispatch_visibility_commands(sessions.update_pose(session_id, player_pose));
                } else if frame.id == ServerboundChatCommand::ID {
                    let mut body = frame.body;
                    let command = ServerboundChatCommand::decode(&mut body)?;
                    if let Some(mode) = parse_gamemode_command(&command.command) {
                        if let Some(state) = interaction.as_deref_mut() {
                            state.pending_break = None;
                            state.pending_use = None;
                        }
                        apply_game_mode(writer, compression, &mut game_mode, mode, permissions).await?;
                    } else if let Some(command) = parse_debug_command(&command.command) {
                        apply_debug_command(
                            writer,
                            compression,
                            &mut survival_state,
                            interaction.as_deref_mut(),
                            command,
                            permissions,
                        ).await?;
                    } else {
                        debug!(command = %command.command, "unsupported command ignored");
                    }
                } else if frame.id == ServerboundChangeGameMode::ID {
                    let mut body = frame.body;
                    let command = ServerboundChangeGameMode::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        state.pending_break = None;
                        state.pending_use = None;
                    }
                    apply_game_mode(writer, compression, &mut game_mode, command.mode, permissions).await?;
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

fn parse_gamemode_command(command: &str) -> Option<GameMode> {
    let mut parts = command.split_whitespace();
    let name = parts.next()?;
    if name != "gamemode" && name != "defaultgamemode" {
        return None;
    }
    let mode = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    parse_game_mode(mode)
}

fn parse_game_mode(mode: &str) -> Option<GameMode> {
    match mode {
        "0" | "survival" | "s" => Some(GameMode::Survival),
        "1" | "creative" | "c" => Some(GameMode::Creative),
        "2" | "adventure" | "a" => Some(GameMode::Adventure),
        "3" | "spectator" | "sp" => Some(GameMode::Spectator),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SurvivalCommand {
    Damage(f32),
    Heal(f32),
    Feed { food: i32, saturation: f32 },
    Exhaust(f32),
}

#[derive(Debug, Clone, PartialEq)]
enum DebugCommand {
    Survival(SurvivalCommand),
    Give {
        item: mc_data::Identifier,
        count: i32,
        hotbar_slot: u8,
    },
}

fn parse_debug_command(command: &str) -> Option<DebugCommand> {
    let rest = command.strip_prefix("debug ")?;
    if let Some(survival) = rest.strip_prefix("survival ") {
        return parse_survival_command(survival).map(DebugCommand::Survival);
    }

    let mut parts = rest.split_whitespace();
    let name = parts.next()?;
    if name != "give" {
        return None;
    }
    let item = mc_data::Identifier::parse(parts.next()?.to_string()).ok()?;
    let count = parts.next().unwrap_or("1").parse().ok()?;
    let hotbar_slot = parts.next().unwrap_or("0").parse::<i32>().ok()?;
    if parts.next().is_some() || !(0..=8).contains(&hotbar_slot) {
        return None;
    }
    Some(DebugCommand::Give {
        item,
        count,
        hotbar_slot: hotbar_slot as u8,
    })
}

fn parse_survival_command(command: &str) -> Option<SurvivalCommand> {
    let mut parts = command.split_whitespace();
    let name = parts.next()?;
    match name {
        "damage" => {
            let amount = parts.next()?.parse().ok()?;
            parts
                .next()
                .is_none()
                .then_some(SurvivalCommand::Damage(amount))
        }
        "heal" => {
            let amount = parts.next().unwrap_or("20").parse().ok()?;
            parts
                .next()
                .is_none()
                .then_some(SurvivalCommand::Heal(amount))
        }
        "feed" => {
            let food = parts.next().unwrap_or("20").parse().ok()?;
            let saturation = parts.next().unwrap_or("5").parse().ok()?;
            parts
                .next()
                .is_none()
                .then_some(SurvivalCommand::Feed { food, saturation })
        }
        "exhaust" => {
            let amount = parts.next()?.parse().ok()?;
            parts
                .next()
                .is_none()
                .then_some(SurvivalCommand::Exhaust(amount))
        }
        _ => None,
    }
}

async fn apply_debug_command<W>(
    writer: &mut W,
    compression: Compression,
    survival_state: &mut SurvivalState,
    mut interaction: Option<&mut InteractionState>,
    command: DebugCommand,
    permissions: CommandPermissions,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if !permissions.op {
        debug!(command = ?command, "debug command denied for non-op player");
        return Ok(());
    }

    match command {
        DebugCommand::Survival(command) => {
            let result = apply_survival_command(
                writer,
                compression,
                survival_state,
                interaction.as_deref_mut(),
                command,
            )
            .await;
            if survival_state.is_dead()
                && let Some(state) = interaction.as_mut()
            {
                state.pending_break = None;
            }
            result
        }
        DebugCommand::Give {
            item,
            count,
            hotbar_slot,
        } => {
            let Some(state) = interaction else {
                debug!(%item, "debug give ignored — no interaction state");
                return Ok(());
            };
            let Some(item_id) = state.items.id_of(&item) else {
                debug!(%item, "debug give ignored — item not in registry");
                return Ok(());
            };
            let stack = if count <= 0 {
                ItemStack::EMPTY
            } else {
                ItemStack::new(item_id, count.min(i32::from(u8::MAX)))
            };
            state.inventory.set_hotbar(hotbar_slot, stack.clone());
            state.inventory_state_id = state.inventory_state_id.wrapping_add(1);
            write_packet(
                writer,
                &ClientboundContainerSetSlot {
                    container_id: 0,
                    state_id: state.inventory_state_id,
                    slot: (PlayerInventory::HOTBAR_BASE + hotbar_slot as usize) as i16,
                    item_stack: stack,
                },
                compression,
            )
            .await
        }
    }
}

async fn apply_survival_command<W>(
    writer: &mut W,
    compression: Compression,
    state: &mut SurvivalState,
    mut interaction: Option<&mut InteractionState>,
    command: SurvivalCommand,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let mut armor_changed = Vec::new();
    match command {
        SurvivalCommand::Damage(amount) => {
            state.apply_damage(survival_damage_after_armor(interaction.as_deref(), amount));
            if amount > 0.0
                && let Some(interaction) = interaction.as_deref_mut()
            {
                armor_changed = damage_equipped_armor(interaction);
            }
        }
        SurvivalCommand::Heal(amount) => state.heal(amount),
        SurvivalCommand::Feed { food, saturation } => state.add_food(food, saturation),
        SurvivalCommand::Exhaust(amount) => state.add_exhaustion(amount),
    }
    if state.is_dead() {
        debug!("player survival state reached death threshold");
    }
    write_packet(writer, &state.as_packet(), compression).await?;
    if !armor_changed.is_empty()
        && let Some(interaction) = interaction
    {
        write_inventory_slot_updates(interaction, writer, armor_changed).await?;
    }
    Ok(())
}

async fn apply_game_mode<W>(
    writer: &mut W,
    compression: Compression,
    current: &mut GameMode,
    requested: GameMode,
    permissions: CommandPermissions,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if !permissions.can_change_game_mode() {
        debug!(mode = ?requested, "gamemode change denied for non-op player");
        return Ok(());
    }
    if *current == requested {
        return Ok(());
    }
    *current = requested;
    write_packet(
        writer,
        &GameEvent {
            event: GameEvent::EVENT_CHANGE_GAME_MODE,
            value: requested.id() as f32,
        },
        compression,
    )
    .await?;
    write_packet(writer, &player_abilities_for_mode(requested), compression).await
}

#[allow(clippy::too_many_arguments)]
async fn handle_client_command<W>(
    writer: &mut W,
    compression: Compression,
    interaction: Option<&mut InteractionState>,
    player_pose: &mut PlayerPose,
    respawn_pose: PlayerPose,
    survival_state: &mut SurvivalState,
    respawn: &ClientboundRespawn,
    command: ServerboundClientCommand,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    match command.action {
        ClientCommandAction::PerformRespawn => {
            if !survival_state.is_dead() {
                return Ok(());
            }
            *survival_state = SurvivalState::FULL;
            if let Some(state) = interaction {
                state.pending_break = None;
            }
            *player_pose = respawn_pose;
            write_packet(writer, respawn, compression).await?;
            write_packet(
                writer,
                &SynchronizePlayerPosition {
                    teleport_id: 2,
                    x: player_pose.x,
                    y: player_pose.y,
                    z: player_pose.z,
                    dx: 0.0,
                    dy: 0.0,
                    dz: 0.0,
                    yaw: player_pose.yaw,
                    pitch: player_pose.pitch,
                    relative_flags: 0,
                },
                compression,
            )
            .await?;
            write_packet(writer, &survival_state.as_packet(), compression).await
        }
        ClientCommandAction::RequestStats | ClientCommandAction::RequestGameruleValues => {
            debug!(action = ?command.action, "client command ignored");
            Ok(())
        }
    }
}

fn player_abilities_for_mode(mode: GameMode) -> ClientboundPlayerAbilities {
    match mode {
        GameMode::Creative => ClientboundPlayerAbilities {
            invulnerable: true,
            flying: false,
            can_fly: true,
            instabuild: true,
            flying_speed: 0.05,
            walking_speed: 0.1,
        },
        GameMode::Spectator => ClientboundPlayerAbilities {
            invulnerable: true,
            flying: true,
            can_fly: true,
            instabuild: false,
            flying_speed: 0.05,
            walking_speed: 0.1,
        },
        GameMode::Survival | GameMode::Adventure => ClientboundPlayerAbilities {
            invulnerable: false,
            flying: false,
            can_fly: false,
            instabuild: false,
            flying_speed: 0.05,
            walking_speed: 0.1,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_tick_cadence_matches_vanilla_cow_tracking() {
        assert_eq!(ENTITY_TICK_PERIOD, Duration::from_millis(50));
        assert_eq!(mc_physics::TICK_SECONDS, 0.05);
        assert_eq!(ENTITY_MOVE_SEND_INTERVAL_TICKS, 1);
    }

    #[test]
    fn gamemode_command_parses_names_and_numeric_modes() {
        assert_eq!(
            parse_gamemode_command("gamemode survival"),
            Some(GameMode::Survival)
        );
        assert_eq!(
            parse_gamemode_command("gamemode creative"),
            Some(GameMode::Creative)
        );
        assert_eq!(
            parse_gamemode_command("gamemode adventure"),
            Some(GameMode::Adventure)
        );
        assert_eq!(
            parse_gamemode_command("gamemode spectator"),
            Some(GameMode::Spectator)
        );
        assert_eq!(
            parse_gamemode_command("gamemode 1"),
            Some(GameMode::Creative)
        );
    }

    #[test]
    fn gamemode_command_rejects_unknown_or_extra_args() {
        assert_eq!(parse_gamemode_command("time set day"), None);
        assert_eq!(parse_gamemode_command("gamemode nope"), None);
        assert_eq!(parse_gamemode_command("gamemode creative other"), None);
    }

    #[test]
    fn debug_commands_parse_survival_mutations_and_give() {
        assert_eq!(
            parse_debug_command("debug survival damage 7.5"),
            Some(DebugCommand::Survival(SurvivalCommand::Damage(7.5)))
        );
        assert_eq!(
            parse_debug_command("debug survival heal"),
            Some(DebugCommand::Survival(SurvivalCommand::Heal(20.0)))
        );
        assert_eq!(
            parse_debug_command("debug survival feed 2 0.5"),
            Some(DebugCommand::Survival(SurvivalCommand::Feed {
                food: 2,
                saturation: 0.5
            }))
        );
        assert_eq!(
            parse_debug_command("debug survival exhaust 4"),
            Some(DebugCommand::Survival(SurvivalCommand::Exhaust(4.0)))
        );
        assert_eq!(
            parse_debug_command("debug give minecraft:dirt 64 1"),
            Some(DebugCommand::Give {
                item: mc_data::Identifier::parse("minecraft:dirt").unwrap(),
                count: 64,
                hotbar_slot: 1,
            })
        );
        assert_eq!(parse_debug_command("damage 7.5"), None);
        assert_eq!(parse_debug_command("debug survival damage bad"), None);
    }

    #[test]
    fn local_dev_profiles_are_op_capable_for_now() {
        let profile = LoggedInProfile {
            uuid: uuid::Uuid::nil(),
            name: "op_probe".to_string(),
        };

        let permissions = CommandPermissions::for_local_dev_profile(&profile);

        assert!(permissions.can_change_game_mode());
    }

    #[test]
    fn item_to_block_table_is_registry_derived() {
        use std::collections::BTreeMap;

        use mc_data::blocks::{BlockReport, BlockStateReport};
        use mc_data::items::ItemReport;

        let items = ItemRegistry::from_report(&[
            ItemReport {
                id: mc_data::Identifier::parse("minecraft:dirt").unwrap(),
                protocol_id: 42,
            },
            ItemReport {
                id: mc_data::Identifier::parse("minecraft:apple").unwrap(),
                protocol_id: 43,
            },
        ]);
        let blocks = mc_world::BlockRegistry::from_report(&[
            BlockReport {
                id: mc_data::Identifier::parse("minecraft:air").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 0,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: mc_data::Identifier::parse("minecraft:dirt").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 1,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
        ])
        .unwrap();

        let table = ItemToBlockTable::build(&items, &blocks);

        assert_eq!(table.resolve(42), Some(mc_world::BlockStateId(1)));
        assert_eq!(table.resolve(43), None);
    }

    #[test]
    fn inventory_merge_prefers_existing_stacks_then_empty_slots() {
        let mut inventory = PlayerInventory::empty();
        inventory.slots[10] = ItemStack::new(42, 63);

        let (remaining, changed) = inventory.merge_stack(ItemStack::new(42, 3));

        assert!(remaining.is_empty());
        assert_eq!(inventory.slots[10], ItemStack::new(42, 64));
        assert_eq!(inventory.slots[9], ItemStack::new(42, 2));
        assert_eq!(changed.len(), 2);
    }

    #[test]
    fn inventory_merge_keeps_different_damage_components_separate() {
        let mut inventory = PlayerInventory::empty();
        inventory.slots[10] = ItemStack::new(42, 1).with_damage(1);

        let (remaining, changed) = inventory.merge_stack(ItemStack::new(42, 1).with_damage(2));

        assert!(remaining.is_empty());
        assert_eq!(inventory.slots[10], ItemStack::new(42, 1).with_damage(1));
        assert_eq!(inventory.slots[9], ItemStack::new(42, 1).with_damage(2));
        assert_eq!(changed, vec![(9, ItemStack::new(42, 1).with_damage(2))]);
    }

    #[test]
    fn pickup_merge_prefers_hotbar_for_new_stacks() {
        let mut inventory = PlayerInventory::empty();

        let (remaining, changed) = inventory.merge_pickup_stack(ItemStack::new(42, 3));

        assert!(remaining.is_empty());
        assert_eq!(inventory.slots[36], ItemStack::new(42, 3));
        assert_eq!(changed, vec![(36, ItemStack::new(42, 3))]);
    }

    #[test]
    fn pickup_merge_prefers_existing_stacks_before_empty_hotbar() {
        let mut inventory = PlayerInventory::empty();
        inventory.slots[10] = ItemStack::new(42, 63);

        let (remaining, changed) = inventory.merge_pickup_stack(ItemStack::new(42, 3));

        assert!(remaining.is_empty());
        assert_eq!(inventory.slots[10], ItemStack::new(42, 64));
        assert_eq!(inventory.slots[36], ItemStack::new(42, 2));
        assert_eq!(changed.len(), 2);
    }

    #[test]
    fn tool_damage_limits_cover_common_fallback_tools() {
        assert_eq!(max_tool_damage_for_path("wooden_pickaxe"), Some(59));
        assert_eq!(max_tool_damage_for_path("stone_axe"), Some(131));
        assert_eq!(max_tool_damage_for_path("iron_shovel"), Some(250));
        assert_eq!(max_tool_damage_for_path("diamond_sword"), Some(1561));
        assert_eq!(max_tool_damage_for_path("golden_hoe"), Some(32));
        assert_eq!(max_tool_damage_for_path("netherite_pickaxe"), Some(2031));
        assert_eq!(max_tool_damage_for_path("apple"), None);
    }

    #[test]
    fn armor_material_rules_match_local_vanilla_basics() {
        let armor = mc_data::armor::builtin();
        let iron_chestplate = armor
            .entry(&mc_data::Identifier::parse("minecraft:iron_chestplate").unwrap())
            .unwrap();
        assert_eq!(iron_chestplate.slot, mc_data::armor::ArmorSlot::Chest);
        assert_eq!(armor_slot_for_kind(iron_chestplate.slot), 6);
        assert_eq!(iron_chestplate.armor, 6.0);
        assert_eq!(iron_chestplate.toughness, 0.0);
        assert_eq!(iron_chestplate.max_damage, 240);

        let diamond_leggings = armor
            .entry(&mc_data::Identifier::parse("minecraft:diamond_leggings").unwrap())
            .unwrap();
        assert_eq!(diamond_leggings.armor, 6.0);
        assert_eq!(diamond_leggings.toughness, 2.0);
        assert_eq!(
            armor.entry(&mc_data::Identifier::parse("minecraft:apple").unwrap()),
            None
        );
    }

    #[test]
    fn armor_reduction_uses_vanilla_combat_rule_shape() {
        let unarmored = armor_reduced_damage(
            10.0,
            ArmorStats {
                armor: 0.0,
                toughness: 0.0,
            },
        );
        assert!((unarmored - 10.0).abs() < f32::EPSILON);

        let iron_chestplate = armor_reduced_damage(
            10.0,
            ArmorStats {
                armor: 6.0,
                toughness: 0.0,
            },
        );
        assert!((iron_chestplate - 9.52).abs() < 0.001);
    }

    #[test]
    fn survival_periodic_tick_regens_and_starves() {
        let mut fed = SurvivalState::FULL;
        fed.apply_damage(2.0);
        assert!(!fed.tick_health(1));
        assert!(fed.tick_health(4));
        assert_eq!(fed.health, 19.0);
        assert!(fed.food < SurvivalState::MAX_FOOD || fed.saturation < 5.0);

        let mut starving = SurvivalState::FULL;
        starving.food = 0;
        starving.saturation = 0.0;
        assert!(starving.tick_health(4));
        assert_eq!(starving.health, 19.0);
    }

    #[test]
    fn reach_validation_uses_player_eye_position() {
        let pose = PlayerPose::new(0.0, 64.0, 0.0);
        assert!(within_block_reach(
            pose,
            pack_block_pos(0, 64, 2),
            GameMode::Survival
        ));
        assert!(!within_block_reach(
            pose,
            pack_block_pos(0, 64, 8),
            GameMode::Survival
        ));
        assert!(within_entity_reach(
            pose,
            Vec3::new(0.0, 65.0, 2.0),
            GameMode::Survival
        ));
        assert!(!within_entity_reach(
            pose,
            Vec3::new(0.0, 65.0, 8.0),
            GameMode::Survival
        ));
    }

    #[test]
    fn survival_block_drops_come_from_repo_loot_data() {
        let id = |value: &str| mc_data::Identifier::parse(value).unwrap();
        let loot = mc_data::loot::builtin();

        assert_eq!(
            loot.block_drop(&id("minecraft:grass_block")),
            Some(&id("minecraft:dirt"))
        );
        assert_eq!(
            loot.block_drop(&id("minecraft:stone")),
            Some(&id("minecraft:cobblestone"))
        );
        assert_eq!(
            loot.block_drop(&id("minecraft:coal_ore")),
            Some(&id("minecraft:coal"))
        );
        assert_eq!(
            loot.block_drop(&id("minecraft:iron_ore")),
            Some(&id("minecraft:raw_iron"))
        );
        assert_eq!(
            loot.block_drop(&id("minecraft:redstone_ore")),
            Some(&id("minecraft:redstone"))
        );
        assert_eq!(
            loot.block_drop(&id("minecraft:oak_leaves")),
            Some(&id("minecraft:apple"))
        );
        assert_eq!(loot.block_drop(&id("minecraft:oak_log")), None);
    }

    #[test]
    fn passive_mob_drops_come_from_repo_loot_data() {
        let id = |value: &str| mc_data::Identifier::parse(value).unwrap();
        let loot = mc_data::loot::builtin();

        assert_eq!(
            loot.entity_drop(&id("minecraft:cow")),
            Some(&id("minecraft:beef"))
        );
        assert_eq!(
            loot.entity_drop(&id("minecraft:pig")),
            Some(&id("minecraft:porkchop"))
        );
        assert_eq!(
            loot.entity_drop(&id("minecraft:chicken")),
            Some(&id("minecraft:chicken"))
        );
        assert_eq!(loot.entity_drop(&id("minecraft:zombie")), None);
    }

    #[test]
    fn recipe_ingredient_matching_resolves_item_tags() {
        use mc_data::items::ItemReport;
        use mc_data::recipes::{Ingredient, IngredientAlternative};

        let oak_log = mc_data::Identifier::parse("minecraft:oak_log").unwrap();
        let birch_log = mc_data::Identifier::parse("minecraft:birch_log").unwrap();
        let apple = mc_data::Identifier::parse("minecraft:apple").unwrap();
        let logs = mc_data::Identifier::parse("minecraft:logs").unwrap();
        let items = ItemRegistry::from_report(&[
            ItemReport {
                id: oak_log,
                protocol_id: 10,
            },
            ItemReport {
                id: birch_log,
                protocol_id: 11,
            },
            ItemReport {
                id: apple,
                protocol_id: 12,
            },
        ]);
        let tags = TagsData {
            registries: BTreeMap::from([(
                mc_data::Identifier::parse("minecraft:item").unwrap(),
                BTreeMap::from([(logs.clone(), vec![10, 11])]),
            )]),
        };
        let ingredient = Ingredient {
            alternatives: vec![IngredientAlternative::Tag(logs)],
        };

        assert!(ingredient_accepts_item(&items, &tags, 10, &ingredient));
        assert!(ingredient_accepts_item(&items, &tags, 11, &ingredient));
        assert!(!ingredient_accepts_item(&items, &tags, 12, &ingredient));
    }

    #[test]
    fn fallback_recipes_include_tag_driven_survival_basics() {
        let recipes = fallback_crafting_recipes();

        assert_eq!(recipes[0].id.as_str(), "minecraft:torch");
        assert_eq!(recipes[1].id.as_str(), "minecraft:oak_planks");
        assert_eq!(recipes[2].id.as_str(), "minecraft:stick");
        assert_eq!(recipes[3].id.as_str(), "minecraft:crafting_table");
        assert!(
            recipes
                .iter()
                .any(|recipe| recipe.id.as_str() == "minecraft:wooden_pickaxe")
        );
        assert!(
            recipes
                .iter()
                .any(|recipe| recipe.id.as_str() == "minecraft:stone_pickaxe")
        );
        assert!(
            recipes
                .iter()
                .any(|recipe| recipe.id.as_str() == "minecraft:furnace")
        );
        assert!(recipes.iter().any(|recipe| matches!(
            (&recipe.kind, recipe.result.item.as_str()),
            (
                mc_data::recipes::RecipeKind::Smelting(_),
                "minecraft:iron_ingot"
            )
        )));

        let mc_data::recipes::RecipeKind::Shapeless(oak_planks) = &recipes[1].kind else {
            panic!("expected shapeless oak planks recipe");
        };
        assert_eq!(
            oak_planks.ingredients[0].alternatives[0],
            mc_data::recipes::IngredientAlternative::Tag(
                mc_data::Identifier::parse("minecraft:oak_logs").unwrap()
            )
        );
    }

    #[test]
    fn durability_tool_detection_covers_fallback_tool_families() {
        assert!(is_durability_tool_path("iron_pickaxe"));
        assert!(is_durability_tool_path("wooden_shovel"));
        assert!(is_durability_tool_path("diamond_axe"));
        assert!(is_durability_tool_path("stone_hoe"));
        assert!(is_durability_tool_path("netherite_sword"));
        assert!(!is_durability_tool_path("apple"));
        assert!(!is_durability_tool_path("oak_planks"));
    }

    #[test]
    fn fallback_food_rules_include_common_edibles() {
        assert_eq!(
            food_rule_for_item(&mc_data::Identifier::parse("minecraft:apple").unwrap()),
            Some(FoodRule {
                item: "minecraft:apple",
                food: 4,
                saturation: 2.4,
            })
        );
        assert_eq!(
            food_rule_for_item(&mc_data::Identifier::parse("minecraft:dirt").unwrap()),
            None
        );
    }

    #[test]
    fn fallback_mining_rules_use_block_family_and_matching_tool() {
        let stone_hand = fallback_mining_time("stone", None);
        let stone_pickaxe = fallback_mining_time("stone", Some("iron_pickaxe"));
        let stone_shovel = fallback_mining_time("stone", Some("iron_shovel"));

        assert!(stone_pickaxe < stone_hand);
        assert_eq!(stone_shovel, stone_hand);
        assert!(
            fallback_mining_time("oak_log", Some("stone_axe"))
                < fallback_mining_time("oak_log", None)
        );
        assert!(
            fallback_mining_time("dirt", Some("wooden_shovel"))
                < fallback_mining_time("dirt", None)
        );
        assert_eq!(
            fallback_mining_time("unknown_custom_block", None),
            Duration::from_millis(800)
        );
    }

    #[test]
    fn passive_spawn_planner_keeps_water_mobs_off_land() {
        use std::collections::BTreeMap;

        let plains = mc_data::Identifier::parse("minecraft:plains").unwrap();
        let ocean = mc_data::Identifier::parse("minecraft:ocean").unwrap();
        let pig = mc_data::Identifier::parse("minecraft:pig").unwrap();
        let cod = mc_data::Identifier::parse("minecraft:cod").unwrap();
        let rules = mc_data::biomes::BiomeSpawnRules::from_entries(BTreeMap::from([(
            plains.clone(),
            BTreeMap::from([
                (
                    "creature".to_string(),
                    vec![mc_data::biomes::BiomeSpawnEntry {
                        entity_type: pig.clone(),
                        min_count: 2,
                        max_count: 2,
                        weight: 1,
                    }],
                ),
                (
                    "water_ambient".to_string(),
                    vec![mc_data::biomes::BiomeSpawnEntry {
                        entity_type: cod.clone(),
                        min_count: 4,
                        max_count: 4,
                        weight: 1,
                    }],
                ),
            ]),
        )]));
        let entity_types = mc_data::entity_types::EntityTypeRegistry::from_report(&[
            mc_data::entity_types::EntityTypeReport {
                id: pig,
                protocol_id: 1,
            },
            mc_data::entity_types::EntityTypeReport {
                id: cod,
                protocol_id: 2,
            },
        ]);
        let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, mc_world::BlockStateId(0), plains);
        let passable = vec![mc_world::BlockStateId(0)];
        let grass = mc_world::BlockStateId(1);
        let water = mc_world::BlockStateId(2);
        for lx in 3..=12 {
            for lz in 3..=12 {
                let _ = chunk.set_block(lx, 64, lz, grass);
            }
        }

        let spawns = plan_passive_herd(
            &chunk,
            Some(grass),
            Some(water),
            &passable,
            &rules,
            &entity_types,
        );

        assert!(!spawns.is_empty());
        assert!(
            spawns
                .iter()
                .all(|spawn| spawn.entity_type_name == "minecraft:pig")
        );
        assert!(spawns.iter().all(|spawn| spawn.position.y == 65.0));
        assert!(spawns.iter().all(|spawn| spawn.entity_type_id == 1));

        let ocean_rules = mc_data::biomes::BiomeSpawnRules::from_entries(BTreeMap::from([(
            ocean.clone(),
            BTreeMap::from([(
                "water_ambient".to_string(),
                vec![mc_data::biomes::BiomeSpawnEntry {
                    entity_type: mc_data::Identifier::parse("minecraft:cod").unwrap(),
                    min_count: 3,
                    max_count: 3,
                    weight: 1,
                }],
            )]),
        )]));
        let mut ocean_chunk =
            Chunk::empty(ChunkPos { x: 0, z: 0 }, mc_world::BlockStateId(0), ocean);
        for lx in 3..=12 {
            for lz in 3..=12 {
                let _ = ocean_chunk.set_block(lx, DEFAULT_SEA_LEVEL, lz, water);
            }
        }

        let spawns = plan_passive_herd(
            &ocean_chunk,
            Some(grass),
            Some(water),
            &passable,
            &ocean_rules,
            &entity_types,
        );

        assert!(
            spawns
                .iter()
                .all(|spawn| spawn.entity_type_name == "minecraft:cod")
        );
        assert!(
            spawns
                .iter()
                .all(|spawn| spawn.position.y < f64::from(DEFAULT_SEA_LEVEL))
        );
    }

    #[test]
    fn creative_and_spectator_modes_grant_client_abilities() {
        let creative = player_abilities_for_mode(GameMode::Creative);
        assert!(creative.invulnerable);
        assert!(creative.can_fly);
        assert!(creative.instabuild);
        assert!(!creative.flying);

        let spectator = player_abilities_for_mode(GameMode::Spectator);
        assert!(spectator.invulnerable);
        assert!(spectator.can_fly);
        assert!(spectator.flying);
        assert!(!spectator.instabuild);
    }

    #[test]
    fn survival_like_modes_revoke_client_abilities() {
        let survival = player_abilities_for_mode(GameMode::Survival);
        assert!(!survival.invulnerable);
        assert!(!survival.can_fly);
        assert!(!survival.instabuild);
        assert!(!survival.flying);

        let adventure = player_abilities_for_mode(GameMode::Adventure);
        assert_eq!(survival, adventure);
    }

    #[test]
    fn full_survival_state_maps_to_health_packet() {
        assert_eq!(
            SurvivalState::FULL.as_packet(),
            ClientboundSetHealth {
                health: 20.0,
                food: 20,
                saturation: 5.0,
            }
        );
    }

    #[test]
    fn survival_damage_heal_and_death_are_clamped() {
        let mut state = SurvivalState::FULL;

        state.apply_damage(7.5);
        assert_eq!(state.health, 12.5);
        assert!(!state.is_dead());

        state.heal(100.0);
        assert_eq!(state.health, SurvivalState::MAX_HEALTH);

        state.apply_damage(100.0);
        assert_eq!(state.health, 0.0);
        assert!(state.is_dead());
    }

    #[test]
    fn survival_exhaustion_drains_saturation_before_food() {
        let mut state = SurvivalState {
            health: 20.0,
            food: 20,
            saturation: 1.0,
            exhaustion: 0.0,
        };

        state.add_exhaustion(4.0);
        assert_eq!(state.saturation, 0.0);
        assert_eq!(state.food, 20);
        assert_eq!(state.exhaustion, 0.0);

        state.add_exhaustion(8.0);
        assert_eq!(state.food, 18);
        assert_eq!(state.saturation, 0.0);
    }

    #[test]
    fn survival_food_addition_clamps_to_food_level() {
        let mut state = SurvivalState {
            health: 20.0,
            food: 18,
            saturation: 1.0,
            exhaustion: 0.0,
        };

        state.add_food(10, 30.0);

        assert_eq!(state.food, 20);
        assert_eq!(state.saturation, 20.0);
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
    fn spawn_y_uses_chunk_heightmap_without_block_light_table() {
        let plains = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, mc_world::BlockStateId(0), plains);
        let top_y = 72;
        chunk
            .highest_opaque
            .set(0, 0, (top_y - mc_world::MIN_Y + 1) as u32);

        assert_eq!(spawn_y_from_chunk(&mut chunk, None), Some(74.0));
    }

    #[test]
    fn underwater_break_refills_target_with_water() {
        let air = mc_world::BlockStateId(0);
        let water = mc_world::BlockStateId(2);
        let stone = mc_world::BlockStateId(1);

        assert_eq!(
            break_replacement_from_neighbours([Some(water), None, None, None, None], air, water),
            water
        );
        assert_eq!(
            break_replacement_from_neighbours([Some(stone), None, None, None, None], air, water),
            air
        );
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
                herd_spawns: Vec::new(),
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
