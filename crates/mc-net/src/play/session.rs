use super::*;
use std::sync::MutexGuard;
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) type SessionId = u64;

#[derive(Debug, Clone)]
pub(super) enum OutboundCommand {
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
    ChestSlots {
        position: mc_world::BlockPos,
        slots: Vec<ItemStack>,
    },
    FurnaceData {
        position: mc_world::BlockPos,
        changed: Vec<(i16, i16)>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutboundLane {
    Reliable,
    Coalescible,
}

impl OutboundCommand {
    fn lane(&self) -> OutboundLane {
        match self {
            Self::MovePlayer(_)
            | Self::MoveEntityRelative(_)
            | Self::AnimatePlayer { .. }
            | Self::BlockDeltas(_)
            | Self::LightUpdates(_) => OutboundLane::Coalescible,
            Self::SpawnPlayer(_)
            | Self::DespawnPlayer(_)
            | Self::SpawnEntity(_)
            | Self::UpdateEntityData(_)
            | Self::TakeItemEntity { .. }
            | Self::DespawnEntity(_)
            | Self::FurnaceSlots { .. }
            | Self::ChestSlots { .. }
            | Self::FurnaceData { .. } => OutboundLane::Reliable,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct OutboundLightUpdate {
    pub(super) pos: ChunkPos,
    pub(super) light: ChunkLight,
    pub(super) wire: LightData,
}

#[derive(Debug, Clone)]
pub(super) struct SessionRecipient {
    pub(super) id: SessionId,
    pub(super) tx: mpsc::Sender<OutboundCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SessionAdmissionError {
    ServerFull { active: usize, max: usize },
    DuplicateProfile { existing_session: SessionId },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct EntityDispatchCounters {
    pub(crate) spawn: u64,
    pub(crate) move_relative: u64,
    pub(crate) data: u64,
    pub(crate) take: u64,
    pub(crate) remove: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SessionPressureSnapshot {
    pub(crate) sessions: usize,
    pub(crate) ticketed_chunks: usize,
    pub(crate) prepared_chunks: usize,
    pub(crate) server_entities: usize,
    pub(crate) furnace_viewer_sets: usize,
    pub(crate) chest_viewer_sets: usize,
    pub(crate) entity_dispatches: EntityDispatchCounters,
    pub(crate) visibility_command_drops: u64,
    pub(crate) reliable_command_retries: u64,
    pub(crate) reliable_command_retries_in_flight: u64,
}

static VISIBILITY_COMMAND_DROPS: AtomicU64 = AtomicU64::new(0);
static RELIABLE_COMMAND_RETRIES: AtomicU64 = AtomicU64::new(0);
static RELIABLE_COMMAND_RETRIES_IN_FLIGHT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub(super) struct ClaimedPickup {
    pub(super) stack: EntityItemStack,
    pub(super) dispatches: Vec<VisibilityDispatch>,
}

pub(super) struct SessionRegistration<'a> {
    pub(super) profile: &'a LoggedInProfile,
    pub(super) center: (i32, i32),
    pub(super) view_distance: i32,
    pub(super) desired: HashSet<(i32, i32)>,
    pub(super) tx: mpsc::Sender<OutboundCommand>,
    pub(super) pose: PlayerPose,
    pub(super) max_sessions: usize,
}

#[derive(Debug, Clone)]
pub(super) struct VisibilityDispatch {
    pub(crate) recipient: SessionRecipient,
    pub(crate) command: OutboundCommand,
}

#[derive(Debug, Clone)]
pub(super) struct PlayerEntitySnapshot {
    pub(super) session_id: SessionId,
    pub(super) entity_id: i32,
    pub(super) uuid: uuid::Uuid,
    pub(super) name: String,
    pub(super) pose: PlayerPose,
}

#[derive(Debug, Clone)]
pub(super) struct ServerEntitySnapshot {
    pub(super) id: EntityId,
    pub(super) uuid: uuid::Uuid,
    pub(super) type_id: i32,
    pub(super) type_name: String,
    pub(super) position: Vec3,
    pub(super) rotation: mc_entity::Rotation,
    pub(super) velocity: Vec3,
    pub(super) on_ground: bool,
    pub(super) item_stack: Option<EntityItemStack>,
    pub(super) experience_value: Option<i32>,
    pub(super) block_state: Option<u32>,
}

#[derive(Debug, Clone)]
pub(super) struct ServerEntityMove {
    pub(super) id: EntityId,
    pub(super) delta: Vec3,
    pub(super) velocity: Vec3,
    pub(super) rotation: mc_entity::Rotation,
    pub(super) on_ground: bool,
    pub(super) send_velocity: bool,
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
    entities_by_chunk: HashMap<(i32, i32), HashSet<EntityId>>,
    entity_chunks: HashMap<EntityId, (i32, i32)>,
    entity_type_aabbs: HashMap<i32, mc_physics::Aabb>,
    last_sent_entity_positions: HashMap<EntityId, Vec3>,
    last_entity_damage_ticks: HashMap<EntityId, u64>,
    item_pickup_ready_ticks: HashMap<EntityId, u64>,
    spawned_entity_chunks: HashSet<(i32, i32)>,
    furnace_viewers: HashMap<mc_world::BlockPos, HashMap<SessionId, FurnaceViewer>>,
    chest_viewers: HashMap<mc_world::BlockPos, HashMap<SessionId, FurnaceViewer>>,
    player_persistence: HashMap<SessionId, Arc<Mutex<PlayerPersistedState>>>,
    entity_dispatches: EntityDispatchCounters,
    world_time: u64,
}

impl Default for SessionRegistryInner {
    fn default() -> Self {
        Self {
            next_id: 0,
            sessions: HashMap::new(),
            tickets: HashMap::new(),
            prepared: HashMap::new(),
            entities: EntityStore::with_next_id(SERVER_ENTITY_ID_START - 1),
            entities_by_chunk: HashMap::new(),
            entity_chunks: HashMap::new(),
            entity_type_aabbs: HashMap::new(),
            last_sent_entity_positions: HashMap::new(),
            last_entity_damage_ticks: HashMap::new(),
            item_pickup_ready_ticks: HashMap::new(),
            spawned_entity_chunks: HashSet::new(),
            furnace_viewers: HashMap::new(),
            chest_viewers: HashMap::new(),
            player_persistence: HashMap::new(),
            entity_dispatches: EntityDispatchCounters::default(),
            world_time: 0,
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

    fn lock_inner(
        &self,
        operation: &'static str,
    ) -> crate::lock_metrics::TimedGuard<MutexGuard<'_, SessionRegistryInner>> {
        crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::SessionRegistry,
            operation,
            Instant::now(),
            self.inner.lock().expect("session registry poisoned"),
        )
    }

    pub(crate) fn set_world_time(&self, world_time: u64) {
        let mut inner = self.lock_inner("set world time");
        inner.world_time = world_time;
    }

    pub(crate) fn advance_world_time(&self, ticks: u64) -> u64 {
        let mut inner = self.lock_inner("advance world time");
        inner.world_time = inner.world_time.wrapping_add(ticks);
        inner.world_time
    }

    pub(crate) fn world_time(&self) -> u64 {
        let inner = self.lock_inner("read world time");
        inner.world_time
    }

    pub(super) fn register_player_persistence(
        &self,
        id: SessionId,
        state: Arc<Mutex<PlayerPersistedState>>,
    ) {
        let mut inner = self.lock_inner("register player persistence");
        inner.player_persistence.insert(id, state);
    }

    pub(crate) fn persisted_player_states(&self) -> Vec<(uuid::Uuid, PlayerPersistedState)> {
        let entries = {
            let inner = self.lock_inner("save-all player persistence entries");
            inner
                .sessions
                .iter()
                .filter_map(|(id, session)| {
                    inner
                        .player_persistence
                        .get(id)
                        .map(|state| (session.uuid, Arc::clone(state)))
                })
                .collect::<Vec<_>>()
        };
        entries
            .into_iter()
            .map(|(uuid, state)| {
                let state = crate::lock_metrics::timed_guard(
                    crate::lock_metrics::LockMetricKind::PlayerPersistence,
                    "save-all player persistence snapshot",
                    Instant::now(),
                    state.lock().expect("player persistence state poisoned"),
                );
                (uuid, (*state).clone())
            })
            .collect()
    }

    #[cfg(test)]
    pub(super) fn register(
        &self,
        profile: &LoggedInProfile,
        center: (i32, i32),
        view_distance: i32,
        desired: HashSet<(i32, i32)>,
        tx: mpsc::Sender<OutboundCommand>,
        pose: PlayerPose,
    ) -> (SessionId, Vec<VisibilityDispatch>) {
        self.try_register(SessionRegistration {
            profile,
            center,
            view_distance,
            desired,
            tx,
            pose,
            max_sessions: usize::MAX,
        })
        .expect("unbounded session registration should not fail")
    }

    pub(super) fn try_register(
        &self,
        registration: SessionRegistration<'_>,
    ) -> Result<(SessionId, Vec<VisibilityDispatch>), SessionAdmissionError> {
        let mut inner = self.lock_inner("register play session");
        if inner.sessions.len() >= registration.max_sessions {
            return Err(SessionAdmissionError::ServerFull {
                active: inner.sessions.len(),
                max: registration.max_sessions,
            });
        }
        if let Some((&existing_session, _)) = inner.sessions.iter().find(|(_, session)| {
            session.uuid == registration.profile.uuid
                || session
                    .name
                    .eq_ignore_ascii_case(&registration.profile.name)
        }) {
            return Err(SessionAdmissionError::DuplicateProfile { existing_session });
        }
        inner.next_id = inner.next_id.wrapping_add(1).max(1);
        let id = inner.next_id;
        let entity_id = i32::try_from(id).unwrap_or(i32::MAX);
        for &chunk in &registration.desired {
            inner.tickets.entry(chunk).or_default().insert(id);
        }
        inner.sessions.insert(
            id,
            PlaySession {
                name: registration.profile.name.clone(),
                uuid: registration.profile.uuid,
                entity_id,
                pose: registration.pose,
                center: registration.center,
                view_distance: registration.view_distance,
                desired: registration.desired,
                loaded: HashSet::new(),
                visible_players: HashSet::new(),
                visible_entities: HashSet::new(),
                tx: registration.tx,
            },
        );
        let dispatches = refresh_visibility_locked(&mut inner);
        debug!(
            session_id = id,
            entity_id,
            player = %registration.profile.name,
            center_cx = registration.center.0,
            center_cz = registration.center.1,
            view_distance = registration.view_distance,
            sessions = inner.sessions.len(),
            tickets = inner.tickets.len(),
            "play session registered"
        );
        Ok((id, dispatches))
    }

    pub(crate) fn active_session_count(&self) -> usize {
        let inner = self.lock_inner("active session count");
        inner.sessions.len()
    }

    pub(crate) fn pressure_snapshot(&self) -> SessionPressureSnapshot {
        let inner = self.lock_inner("runtime pressure snapshot");
        SessionPressureSnapshot {
            sessions: inner.sessions.len(),
            ticketed_chunks: inner.tickets.len(),
            prepared_chunks: inner.prepared.len(),
            server_entities: inner.entities.len(),
            furnace_viewer_sets: inner.furnace_viewers.len(),
            chest_viewer_sets: inner.chest_viewers.len(),
            entity_dispatches: inner.entity_dispatches,
            visibility_command_drops: VISIBILITY_COMMAND_DROPS.load(Ordering::Relaxed),
            reliable_command_retries: RELIABLE_COMMAND_RETRIES.load(Ordering::Relaxed),
            reliable_command_retries_in_flight: RELIABLE_COMMAND_RETRIES_IN_FLIGHT
                .load(Ordering::Relaxed),
        }
    }

    pub(super) fn unregister(&self, id: SessionId) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_inner("unregister play session");
        let Some(session) = inner.sessions.remove(&id) else {
            return Vec::new();
        };
        for viewers in inner.furnace_viewers.values_mut() {
            viewers.remove(&id);
        }
        for viewers in inner.chest_viewers.values_mut() {
            viewers.remove(&id);
        }
        inner.player_persistence.remove(&id);
        inner
            .furnace_viewers
            .retain(|_, viewers| !viewers.is_empty());
        inner.chest_viewers.retain(|_, viewers| !viewers.is_empty());
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

    pub(super) fn register_furnace_viewer(&self, id: SessionId, position: mc_world::BlockPos) {
        let mut inner = self.lock_inner("register furnace viewer");
        let Some(tx) = inner.sessions.get(&id).map(|session| session.tx.clone()) else {
            return;
        };
        inner
            .furnace_viewers
            .entry(position)
            .or_default()
            .insert(id, FurnaceViewer { tx });
    }

    pub(super) fn unregister_furnace_viewer(&self, id: SessionId, position: mc_world::BlockPos) {
        let mut inner = self.lock_inner("unregister furnace viewer");
        if let Some(viewers) = inner.furnace_viewers.get_mut(&position) {
            viewers.remove(&id);
            if viewers.is_empty() {
                inner.furnace_viewers.remove(&position);
            }
        }
    }

    pub(super) fn register_chest_viewer(&self, id: SessionId, position: mc_world::BlockPos) {
        let mut inner = self.lock_inner("register chest viewer");
        let Some(tx) = inner.sessions.get(&id).map(|session| session.tx.clone()) else {
            return;
        };
        inner
            .chest_viewers
            .entry(position)
            .or_default()
            .insert(id, FurnaceViewer { tx });
    }

    pub(super) fn unregister_chest_viewer(&self, id: SessionId, position: mc_world::BlockPos) {
        let mut inner = self.lock_inner("unregister chest viewer");
        if let Some(viewers) = inner.chest_viewers.get_mut(&position) {
            viewers.remove(&id);
            if viewers.is_empty() {
                inner.chest_viewers.remove(&position);
            }
        }
    }

    pub(super) fn chest_slot_dispatches(
        &self,
        position: mc_world::BlockPos,
        except: SessionId,
        slots: Vec<ItemStack>,
    ) -> Vec<VisibilityDispatch> {
        let inner = self.lock_inner("chest slot dispatches");
        inner
            .chest_viewers
            .get(&position)
            .into_iter()
            .flat_map(|viewers| viewers.iter())
            .filter(|&(&id, _)| id != except)
            .map(|(&id, viewer)| VisibilityDispatch {
                recipient: SessionRecipient {
                    id,
                    tx: viewer.tx.clone(),
                },
                command: OutboundCommand::ChestSlots {
                    position,
                    slots: slots.clone(),
                },
            })
            .collect()
    }

    pub(super) fn furnace_slot_dispatches(
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

    pub(super) fn furnace_data_dispatches(
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
        let inner = self.lock_inner("furnace dispatches");
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

    pub(super) fn is_furnace_tick_owner(
        &self,
        position: mc_world::BlockPos,
        id: SessionId,
    ) -> bool {
        let inner = self.lock_inner("furnace tick owner");
        inner
            .furnace_viewers
            .get(&position)
            .and_then(|viewers| viewers.keys().min().copied())
            == Some(id)
    }

    pub(super) fn replace_view(
        &self,
        id: SessionId,
        center: (i32, i32),
        desired: HashSet<(i32, i32)>,
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_inner("replace chunk view");
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

    pub(super) fn mark_loaded(&self, id: SessionId, chunk: (i32, i32)) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_inner("mark chunk loaded");
        if let Some(session) = inner.sessions.get_mut(&id) {
            if session.loaded.insert(chunk) {
                refresh_loaded_chunk_for_session_locked(&mut inner, id, chunk)
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        }
    }

    pub(super) fn mark_unloaded(
        &self,
        id: SessionId,
        chunks: &[(i32, i32)],
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_inner("mark chunks unloaded");
        let mut dispatches = Vec::new();
        if let Some(session) = inner.sessions.get_mut(&id) {
            let mut removed = Vec::new();
            for chunk in chunks {
                if session.loaded.remove(chunk) {
                    removed.push(*chunk);
                }
            }
            for chunk in removed {
                dispatches.extend(refresh_unloaded_chunk_for_session_locked(
                    &mut inner, id, chunk,
                ));
            }
            dispatches
        } else {
            Vec::new()
        }
    }

    pub(super) fn update_pose(&self, id: SessionId, pose: PlayerPose) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_inner("update player pose");
        let old_observers = visible_observers_locked(&inner, id);
        let old_chunk = inner
            .sessions
            .get(&id)
            .map(|session| session.pose.chunk_pos());
        let Some(session) = inner.sessions.get_mut(&id) else {
            return Vec::new();
        };
        session.pose = pose;
        push_entities_from_player_locked(&mut inner, pose);
        let crossed_chunk = old_chunk.is_some_and(|chunk| chunk != pose.chunk_pos());
        let mut dispatches = if crossed_chunk {
            refresh_player_target_visibility_locked(
                &mut inner,
                id,
                old_chunk.expect("old chunk recorded for existing session"),
                pose.chunk_pos(),
            )
        } else {
            Vec::new()
        };
        let new_observers = if crossed_chunk {
            visible_observers_locked(&inner, id)
        } else {
            old_observers.clone()
        };
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

    pub(super) fn broadcast_player_animation(&self, id: SessionId) -> Vec<VisibilityDispatch> {
        let inner = self.lock_inner("broadcast player animation");
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

    pub(super) fn ensure_chunk_herd(
        &self,
        chunk: (i32, i32),
        spawns: &[HerdSpawn],
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_inner("ensure chunk herd");
        if !inner.spawned_entity_chunks.insert(chunk) {
            return Vec::new();
        }
        let mut passive_count = 0_usize;
        let mut hostile_count = 0_usize;
        let mut spawned = Vec::new();
        for spawn in spawns {
            debug_assert_eq!(spawn.chunk, chunk);
            if spawn.hostile {
                if hostile_count >= MAX_HOSTILE_SPAWNS_PER_CHUNK {
                    continue;
                }
            } else if passive_count >= MAX_PASSIVE_SPAWNS_PER_CHUNK {
                continue;
            }
            if !spawn_far_enough_from_players(&inner, spawn.position) {
                continue;
            }
            let uuid = herd_uuid(spawn.chunk, spawn.slot);
            if inner.entities.contains_uuid(uuid) {
                continue;
            }
            let mut entity = SpawnEntity::new(
                spawn.entity_type_id,
                spawn.entity_type_name.clone(),
                spawn.position,
            );
            entity.uuid = Some(uuid);
            apply_entity_facts(&mut entity);
            entity.goal = if spawn.hostile {
                GoalState::Wander {
                    speed: HOSTILE_WANDER_SPEED,
                    period_ticks: 20,
                }
            } else if entity_type_is_aquatic(&entity.type_name) {
                entity.on_ground = false;
                GoalState::AquaticWander {
                    speed: PASSIVE_WANDER_SPEED * 0.9,
                    vertical_speed: 0.18,
                    period_ticks: 45,
                }
            } else {
                GoalState::Wander {
                    speed: PASSIVE_WANDER_SPEED,
                    period_ticks: 80,
                }
            };
            let id = inner.entities.spawn(entity);
            inner
                .entity_type_aabbs
                .entry(spawn.entity_type_id)
                .or_insert_with(|| entity_aabb(&spawn.entity_type_name));
            track_entity_chunk_locked(&mut inner, id, spawn.position);
            inner.last_sent_entity_positions.insert(id, spawn.position);
            spawned.push(id);
            if spawn.hostile {
                hostile_count += 1;
            } else {
                passive_count += 1;
            }
        }
        debug!(
            cx = chunk.0,
            cz = chunk.1,
            entities = spawns.len(),
            "spawned passive entity herd"
        );
        spawned
            .into_iter()
            .flat_map(|id| spawn_entity_visibility_locked(&mut inner, id))
            .collect()
    }

    pub(super) fn spawn_item_drop(
        &self,
        entity_type_id: i32,
        position: Vec3,
        stack: EntityItemStack,
    ) -> Vec<VisibilityDispatch> {
        if stack.is_empty() {
            return Vec::new();
        }
        let mut inner = self.lock_inner("spawn item drop");
        let mut entity = SpawnEntity::new(entity_type_id, "minecraft:item", position);
        entity.item_stack = Some(stack);
        entity.velocity = Vec3::new(0.0, 0.1, 0.0);
        let id = inner.entities.spawn(entity);
        inner
            .entity_type_aabbs
            .entry(entity_type_id)
            .or_insert_with(|| entity_aabb("minecraft:item"));
        track_entity_chunk_locked(&mut inner, id, position);
        inner.last_sent_entity_positions.insert(id, position);
        let ready_tick = inner.world_time.saturating_add(ITEM_PICKUP_DELAY_TICKS);
        inner.item_pickup_ready_ticks.insert(id, ready_tick);
        spawn_entity_visibility_locked(&mut inner, id)
    }

    pub(super) fn spawn_xp_orb(
        &self,
        entity_type_id: i32,
        position: Vec3,
        value: i32,
    ) -> Vec<VisibilityDispatch> {
        if value <= 0 {
            return Vec::new();
        }
        let mut inner = self.lock_inner("spawn xp orb");
        let mut entity = SpawnEntity::new(entity_type_id, "minecraft:experience_orb", position);
        entity.experience_value = Some(value);
        entity.velocity = Vec3::new(0.0, 0.08, 0.0);
        let id = inner.entities.spawn(entity);
        inner
            .entity_type_aabbs
            .entry(entity_type_id)
            .or_insert_with(|| entity_aabb("minecraft:experience_orb"));
        track_entity_chunk_locked(&mut inner, id, position);
        inner.last_sent_entity_positions.insert(id, position);
        spawn_entity_visibility_locked(&mut inner, id)
    }

    pub(super) fn spawn_falling_block(
        &self,
        entity_type_id: i32,
        position: Vec3,
        block_state: mc_world::BlockStateId,
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_inner("spawn falling block");
        let mut entity = SpawnEntity::new(entity_type_id, "minecraft:falling_block", position);
        entity.block_state = Some(block_state.0);
        entity.on_ground = false;
        let id = inner.entities.spawn(entity);
        inner
            .entity_type_aabbs
            .entry(entity_type_id)
            .or_insert_with(|| entity_aabb("minecraft:falling_block"));
        track_entity_chunk_locked(&mut inner, id, position);
        inner.last_sent_entity_positions.insert(id, position);
        spawn_entity_visibility_locked(&mut inner, id)
    }

    pub(super) fn spawn_command_entity(
        &self,
        entity_type_id: i32,
        entity_type_name: String,
        position: Vec3,
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_inner("spawn command entity");
        let mut entity = SpawnEntity::new(entity_type_id, entity_type_name, position);
        apply_entity_facts(&mut entity);
        let aabb = entity_aabb(&entity.type_name);
        let id = inner.entities.spawn(entity);
        inner
            .entity_type_aabbs
            .entry(entity_type_id)
            .or_insert(aabb);
        track_entity_chunk_locked(&mut inner, id, position);
        inner.last_sent_entity_positions.insert(id, position);
        spawn_entity_visibility_locked(&mut inner, id)
    }

    pub(super) fn nearby_item_entities(
        &self,
        position: Vec3,
        radius: f64,
    ) -> Vec<ServerEntitySnapshot> {
        let radius_sq = radius * radius;
        let inner = self.lock_inner("nearby item entities");
        inner
            .entities
            .views()
            .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
            .filter(|entity| entity.item_stack.is_some())
            .filter(|entity| item_pickup_ready_locked(&inner, entity.id))
            .filter(|entity| distance_sq(entity.position, position) <= radius_sq)
            .map(server_entity_snapshot_from_view)
            .collect()
    }

    pub(super) fn nearby_experience_entities(
        &self,
        position: Vec3,
        radius: f64,
    ) -> Vec<ServerEntitySnapshot> {
        let radius_sq = radius * radius;
        let inner = self.lock_inner("nearby experience entities");
        inner
            .entities
            .views()
            .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
            .filter(|entity| entity.experience_value.is_some())
            .filter(|entity| distance_sq(entity.position, position) <= radius_sq)
            .map(server_entity_snapshot_from_view)
            .collect()
    }

    pub(super) fn nearby_hostile_entities(
        &self,
        position: Vec3,
        radius: f64,
    ) -> Vec<ServerEntitySnapshot> {
        let radius_sq = radius * radius;
        let inner = self.lock_inner("nearby hostile entities");
        inner
            .entities
            .views()
            .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
            .filter(|entity| entity.item_stack.is_none())
            .filter(|entity| is_hostile_entity(entity.type_name))
            .filter(|entity| distance_sq(entity.position, position) <= radius_sq)
            .map(server_entity_snapshot_from_view)
            .collect()
    }

    pub(super) fn server_entity_snapshot(
        &self,
        entity_id: EntityId,
    ) -> Option<ServerEntitySnapshot> {
        let inner = self.lock_inner("server entity snapshot");
        inner
            .entities
            .snapshot(entity_id)
            .map(server_entity_snapshot_from)
    }

    pub(super) fn damage_server_entity(
        &self,
        entity_id: EntityId,
        amount: f32,
    ) -> Option<mc_entity::EntityDamage> {
        let mut inner = self.lock_inner("damage server entity");
        let world_time = inner.world_time;
        if inner
            .last_entity_damage_ticks
            .get(&entity_id)
            .is_some_and(|last| world_time.saturating_sub(*last) < ENTITY_HURT_INVULNERABLE_TICKS)
        {
            return None;
        }
        let damage = inner.entities.damage(entity_id, amount)?;
        inner.last_entity_damage_ticks.insert(entity_id, world_time);
        Some(damage)
    }

    pub(super) fn claim_item_pickup(
        &self,
        entity_id: EntityId,
        collector_session: SessionId,
        max_count: i32,
    ) -> Option<ClaimedPickup> {
        if max_count <= 0 {
            return None;
        }
        let mut inner = self.lock_inner("remove dead entity");
        let snapshot = inner.entities.snapshot(entity_id)?;
        if snapshot.lifecycle != EntityLifecycle::Alive {
            return None;
        }
        if !item_pickup_ready_locked(&inner, entity_id) {
            return None;
        }
        let mut stack = snapshot.item_stack?;
        if stack.count <= 0 {
            return None;
        }
        let picked_count = stack.count.min(max_count);
        let picked = EntityItemStack::new(stack.item_id, picked_count);
        stack.count -= picked_count;

        if stack.count <= 0 {
            let snapshot = inner
                .entities
                .remove(entity_id)
                .map(server_entity_snapshot_from)?;
            inner.last_sent_entity_positions.remove(&entity_id);
            inner.last_entity_damage_ticks.remove(&entity_id);
            inner.item_pickup_ready_ticks.remove(&entity_id);
            untrack_entity_chunk_locked(&mut inner, entity_id);
            let dispatches = picked_entity_dispatches_locked(
                &mut inner,
                entity_id,
                collector_session,
                picked_count,
                snapshot,
            );
            Some(ClaimedPickup {
                stack: picked,
                dispatches,
            })
        } else {
            if !inner.entities.set_item_stack(entity_id, Some(stack)) {
                return None;
            }
            let snapshot = inner
                .entities
                .snapshot(entity_id)
                .map(server_entity_snapshot_from)?;
            let dispatches: Vec<VisibilityDispatch> =
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
                    .collect();
            record_entity_dispatches_locked(&mut inner, &dispatches);
            Some(ClaimedPickup {
                stack: picked,
                dispatches,
            })
        }
    }

    pub(super) fn remove_server_entity(
        &self,
        entity_id: EntityId,
    ) -> Option<(ServerEntitySnapshot, Vec<VisibilityDispatch>)> {
        let mut inner = self.lock_inner("despawn entity");
        let snapshot = inner
            .entities
            .remove(entity_id)
            .map(server_entity_snapshot_from)?;
        inner.last_sent_entity_positions.remove(&entity_id);
        inner.last_entity_damage_ticks.remove(&entity_id);
        inner.item_pickup_ready_ticks.remove(&entity_id);
        untrack_entity_chunk_locked(&mut inner, entity_id);

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
        record_entity_dispatches_locked(&mut inner, &dispatches);
        Some((snapshot, dispatches))
    }

    pub(super) fn remove_picked_item(
        &self,
        entity_id: EntityId,
        collector_session: SessionId,
        amount: i32,
    ) -> Option<Vec<VisibilityDispatch>> {
        let mut inner = self.lock_inner("remove picked item entity");
        let snapshot = inner
            .entities
            .remove(entity_id)
            .map(server_entity_snapshot_from)?;
        inner.last_sent_entity_positions.remove(&entity_id);
        inner.last_entity_damage_ticks.remove(&entity_id);
        inner.item_pickup_ready_ticks.remove(&entity_id);
        untrack_entity_chunk_locked(&mut inner, entity_id);
        Some(picked_entity_dispatches_locked(
            &mut inner,
            entity_id,
            collector_session,
            amount,
            snapshot,
        ))
    }

    pub(crate) fn tick_entities_and_collect_physics_queries(
        &self,
        tick: u64,
    ) -> Vec<EntityPhysicsQuery> {
        let mut inner = self.lock_inner("tick entities");
        if inner.entities.is_empty() {
            return Vec::new();
        }
        let active_chunks: HashSet<_> = inner
            .sessions
            .values()
            .flat_map(|session| session.loaded.iter().copied())
            .collect();
        if active_chunks.is_empty() {
            return Vec::new();
        }
        let player_positions: Vec<_> = inner
            .sessions
            .values()
            .map(|session| Vec3::new(session.pose.x, session.pose.y, session.pose.z))
            .collect();
        update_hostile_targets_locked(&mut inner);
        inner.entities.tick_goals(tick);
        let mut candidates: Vec<_> = inner
            .entities
            .views()
            .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
            .filter_map(|entity| {
                let chunk = chunk_pos_from_coords(entity.position.x, entity.position.z);
                if !active_chunks.contains(&chunk)
                    || !entity_is_near_player_chunk(chunk, &player_positions)
                {
                    return None;
                }
                let aabb = inner
                    .entity_type_aabbs
                    .get(&entity.type_id)
                    .copied()
                    .unwrap_or_else(|| entity_aabb(entity.type_name));
                Some((
                    entity_distance_sq_to_players(entity.position, &player_positions),
                    EntityPhysicsQuery {
                        id: entity.id,
                        position: entity.position,
                        velocity: entity.velocity,
                        aabb,
                        on_ground: entity.on_ground,
                    },
                ))
            })
            .collect();
        if candidates.len() > ENTITY_PHYSICS_QUERY_BUDGET_PER_TICK {
            candidates.select_nth_unstable_by(
                ENTITY_PHYSICS_QUERY_BUDGET_PER_TICK,
                |(left, _), (right, _)| left.total_cmp(right),
            );
            candidates.truncate(ENTITY_PHYSICS_QUERY_BUDGET_PER_TICK);
        }
        candidates.into_iter().map(|(_, query)| query).collect()
    }

    pub(crate) fn restore_persisted_entities(
        &self,
        entities: impl IntoIterator<Item = mc_entity::EntitySnapshot>,
    ) -> usize {
        let mut inner = self.lock_inner("restore persisted entities");
        let mut restored = 0;
        for entity in entities {
            let aabb = entity_aabb(&entity.type_name);
            let chunk = server_entity_chunk_pos(&server_entity_snapshot_from(entity.clone()));
            let type_id = entity.type_id;
            let entity_id = entity.id;
            let position = entity.position;
            if inner.entities.insert_snapshot(entity) {
                inner.entity_type_aabbs.entry(type_id).or_insert(aabb);
                track_entity_chunk_locked(&mut inner, entity_id, position);
                inner.spawned_entity_chunks.insert(chunk);
                restored += 1;
            }
        }
        restored
    }

    pub(crate) fn persisted_entity_snapshots(&self) -> Vec<mc_entity::EntitySnapshot> {
        let inner = self.lock_inner("persisted entity snapshots");
        inner
            .entities
            .snapshots()
            .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
            .collect()
    }

    pub(crate) fn apply_entity_physics_and_dispatch(&self, tick: u64, steps: &[EntityPhysicsStep]) {
        let mut inner = self.lock_inner("apply entity physics");
        if inner.entities.is_empty() {
            return;
        }
        let old_chunks: HashMap<_, _> = steps
            .iter()
            .filter_map(|step| {
                inner
                    .entity_chunks
                    .get(&step.id)
                    .copied()
                    .map(|chunk| (step.id, chunk))
            })
            .collect();
        let crossed_visibility_boundary = steps.iter().any(|step| {
            old_chunks.get(&step.id).is_some_and(|old_chunk| {
                *old_chunk != chunk_pos_from_coords(step.position.x, step.position.z)
            })
        });
        let old_observers_by_entity = if crossed_visibility_boundary {
            steps
                .iter()
                .map(|step| {
                    (
                        step.id,
                        visible_entity_observers_locked(&inner, step.id)
                            .into_iter()
                            .collect::<HashSet<_>>(),
                    )
                })
                .collect::<HashMap<_, _>>()
        } else {
            HashMap::new()
        };
        for step in steps {
            let _ = inner.entities.set_position(step.id, step.position);
            let _ = inner.entities.set_velocity(step.id, step.velocity);
            let _ = inner.entities.set_on_ground(step.id, step.on_ground);
            if let Some(old_chunk) = old_chunks.get(&step.id).copied() {
                let new_chunk = chunk_pos_from_coords(step.position.x, step.position.z);
                if old_chunk != new_chunk {
                    move_entity_chunk_locked(&mut inner, step.id, old_chunk, new_chunk);
                }
            }
        }
        let mut dispatches = if crossed_visibility_boundary {
            steps
                .iter()
                .filter_map(|step| {
                    let old_chunk = old_chunks.get(&step.id).copied()?;
                    let new_chunk = chunk_pos_from_coords(step.position.x, step.position.z);
                    (old_chunk != new_chunk).then_some((step.id, old_chunk, new_chunk))
                })
                .flat_map(|(entity_id, old_chunk, new_chunk)| {
                    refresh_entity_target_visibility_locked(
                        &mut inner, entity_id, old_chunk, new_chunk,
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        if !tick.is_multiple_of(ENTITY_MOVE_SEND_INTERVAL_TICKS) {
            drop(inner);
            dispatch_visibility_commands(dispatches);
            return;
        }

        let move_dispatch_start = dispatches.len();
        for step in steps {
            let Some(snapshot) = inner
                .entities
                .snapshot(step.id)
                .map(server_entity_snapshot_from)
            else {
                continue;
            };
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
            for observer_id in visible_entity_observers_locked(&inner, snapshot.id) {
                if crossed_visibility_boundary
                    && !old_observers_by_entity
                        .get(&snapshot.id)
                        .is_some_and(|observers| observers.contains(&observer_id))
                {
                    continue;
                }
                if let Some(observer) = inner.sessions.get(&observer_id) {
                    dispatches.push(VisibilityDispatch {
                        recipient: SessionRecipient {
                            id: observer_id,
                            tx: observer.tx.clone(),
                        },
                        command: OutboundCommand::MoveEntityRelative(ServerEntityMove {
                            id: snapshot.id,
                            delta,
                            velocity: snapshot.velocity,
                            rotation: snapshot.rotation,
                            on_ground: snapshot.on_ground,
                            send_velocity: entity_should_send_velocity(&snapshot),
                        }),
                    });
                }
            }
            inner
                .last_sent_entity_positions
                .insert(snapshot.id, snapshot.position);
        }
        record_entity_dispatches_locked(&mut inner, &dispatches[move_dispatch_start..]);
        drop(inner);
        dispatch_visibility_commands(dispatches);
    }

    pub(super) fn loaded_recipients_for_chunks(
        &self,
        chunks: &HashSet<(i32, i32)>,
        except: Option<SessionId>,
    ) -> Vec<SessionRecipient> {
        let inner = self.lock_inner("loaded recipients for chunks");
        let mut ids = HashSet::new();
        for chunk in chunks {
            if let Some(subscribers) = inner.tickets.get(chunk) {
                ids.extend(subscribers.iter().copied().filter(|id| Some(*id) != except));
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

    pub(super) fn prepared_chunk(&self, chunk: (i32, i32)) -> Option<Arc<PreparedChunkFrame>> {
        let inner = self.lock_inner("prepared chunk lookup");
        inner.prepared.get(&chunk).cloned()
    }

    pub(super) fn cache_prepared_chunk(
        &self,
        chunk: (i32, i32),
        prepared: Arc<PreparedChunkFrame>,
    ) {
        let mut inner = self.lock_inner("cache prepared chunk");
        if inner.tickets.contains_key(&chunk) {
            inner.prepared.entry(chunk).or_insert(prepared);
        }
    }

    pub(super) fn invalidate_prepared_chunks(&self, chunks: &HashSet<(i32, i32)>) {
        if chunks.is_empty() {
            return;
        }
        let mut inner = self.lock_inner("invalidate prepared chunks");
        for chunk in chunks {
            inner.prepared.remove(chunk);
        }
    }

    pub(crate) fn ticketed_chunks_sorted(&self) -> Vec<(i32, i32)> {
        let inner = self.lock_inner("ticketed chunks sorted");
        let mut chunks: Vec<_> = inner.tickets.keys().copied().collect();
        chunks.sort_unstable_by_key(|&(cx, cz)| (cz, cx));
        chunks
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

pub(super) fn server_entity_snapshot_from(
    entity: mc_entity::EntitySnapshot,
) -> ServerEntitySnapshot {
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
        experience_value: entity.experience_value,
        block_state: entity.block_state,
    }
}

fn server_entity_snapshot_from_view(entity: EntityView<'_>) -> ServerEntitySnapshot {
    ServerEntitySnapshot {
        id: entity.id,
        uuid: entity.uuid,
        type_id: entity.type_id,
        type_name: entity.type_name.to_owned(),
        position: entity.position,
        rotation: entity.rotation,
        velocity: entity.velocity,
        on_ground: entity.on_ground,
        item_stack: entity.item_stack,
        experience_value: entity.experience_value,
        block_state: entity.block_state,
    }
}

fn apply_entity_facts(entity: &mut SpawnEntity) {
    let Ok(id) = mc_data::Identifier::parse(entity.type_name.clone()) else {
        return;
    };
    let facts = mc_data::entity_types::fallback_entity_type_facts(id, entity.type_id as u32);
    if let Some(value) = facts.attributes.max_health {
        entity.attributes.set_base(AttributeKind::MaxHealth, value);
    }
    if let Some(value) = facts.attributes.movement_speed {
        entity
            .attributes
            .set_base(AttributeKind::MovementSpeed, value);
    }
    if let Some(value) = facts.attributes.follow_range {
        entity
            .attributes
            .set_base(AttributeKind::FollowRange, value);
    }
    if let Some(value) = facts.attributes.attack_damage {
        entity
            .attributes
            .set_base(AttributeKind::AttackDamage, value);
    }
}

fn server_entity_chunk_pos(entity: &ServerEntitySnapshot) -> (i32, i32) {
    chunk_pos_from_coords(entity.position.x, entity.position.z)
}

fn spawn_far_enough_from_players(inner: &SessionRegistryInner, position: Vec3) -> bool {
    let min_distance_sq =
        MIN_ENTITY_SPAWN_DISTANCE_FROM_PLAYER * MIN_ENTITY_SPAWN_DISTANCE_FROM_PLAYER;
    inner.sessions.values().all(|session| {
        distance_sq(
            position,
            Vec3::new(session.pose.x, session.pose.y, session.pose.z),
        ) > min_distance_sq
    })
}

fn item_pickup_ready_locked(inner: &SessionRegistryInner, entity_id: EntityId) -> bool {
    inner
        .item_pickup_ready_ticks
        .get(&entity_id)
        .is_none_or(|ready_tick| inner.world_time >= *ready_tick)
}

fn entity_is_near_player_chunk(chunk: (i32, i32), player_positions: &[Vec3]) -> bool {
    player_positions.iter().any(|position| {
        let player_chunk = chunk_pos_from_coords(position.x, position.z);
        (chunk.0 - player_chunk.0).abs() <= ENTITY_SIMULATION_DISTANCE_CHUNKS
            && (chunk.1 - player_chunk.1).abs() <= ENTITY_SIMULATION_DISTANCE_CHUNKS
    })
}

fn entity_distance_sq_to_players(position: Vec3, player_positions: &[Vec3]) -> f64 {
    player_positions
        .iter()
        .map(|player| distance_sq(position, *player))
        .min_by(f64::total_cmp)
        .unwrap_or(f64::INFINITY)
}

fn entity_type_is_aquatic(type_name: &str) -> bool {
    matches!(
        type_name,
        "minecraft:cod"
            | "minecraft:salmon"
            | "minecraft:tropical_fish"
            | "minecraft:pufferfish"
            | "minecraft:squid"
            | "minecraft:glow_squid"
            | "minecraft:dolphin"
            | "minecraft:axolotl"
            | "minecraft:turtle"
    )
}

fn entity_should_send_velocity(snapshot: &ServerEntitySnapshot) -> bool {
    if snapshot.velocity == Vec3::ZERO {
        return false;
    }
    !matches!(
        snapshot.type_name.as_str(),
        "minecraft:item" | "minecraft:experience_orb"
    )
}

fn update_hostile_targets_locked(inner: &mut SessionRegistryInner) {
    let players: Vec<_> = inner
        .sessions
        .values()
        .map(|session| Vec3::new(session.pose.x, session.pose.y, session.pose.z))
        .collect();
    if players.is_empty() {
        return;
    }
    let hostiles: Vec<_> = inner
        .entities
        .views()
        .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
        .filter(|entity| is_hostile_entity(entity.type_name))
        .map(|entity| {
            let follow_range = entity
                .attributes
                .base(&AttributeKind::FollowRange)
                .unwrap_or(16.0);
            (entity.id, entity.position, follow_range)
        })
        .collect();
    for (hostile_id, hostile_position, follow_range) in hostiles {
        let max_distance_sq = follow_range * follow_range;
        let target = players
            .iter()
            .copied()
            .filter(|position| distance_sq(*position, hostile_position) <= max_distance_sq)
            .min_by(|left, right| {
                distance_sq(*left, hostile_position)
                    .total_cmp(&distance_sq(*right, hostile_position))
            });
        let goal = target.map_or(
            GoalState::Wander {
                speed: HOSTILE_FOLLOW_SPEED,
                period_ticks: 20,
            },
            |target| GoalState::FollowPosition {
                target,
                speed: HOSTILE_FOLLOW_SPEED,
            },
        );
        let _ = inner.entities.set_goal(hostile_id, goal);
    }
}

pub(super) fn distance_sq(a: Vec3, b: Vec3) -> f64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    let dz = a.z - b.z;
    dx * dx + dy * dy + dz * dz
}

pub(super) fn entity_aabb(type_name: &str) -> mc_physics::Aabb {
    let facts = mc_data::Identifier::parse(type_name.to_string())
        .map(|id| mc_data::entity_types::fallback_entity_type_facts(id, 0))
        .ok();
    facts.map_or(mc_physics::Aabb::COW, |facts| mc_physics::Aabb {
        half_width: facts.dimensions.half_width(),
        height: facts.dimensions.height,
    })
}

fn push_entities_from_player_locked(inner: &mut SessionRegistryInner, pose: PlayerPose) {
    const PLAYER_HALF_WIDTH: f64 = 0.3;
    let player = Vec3::new(pose.x, pose.y, pose.z);
    let snapshots: Vec<_> = inner
        .entities
        .views()
        .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
        .filter(|entity| entity.item_stack.is_none())
        .map(|entity| {
            let aabb = inner
                .entity_type_aabbs
                .get(&entity.type_id)
                .copied()
                .unwrap_or_else(|| entity_aabb(entity.type_name));
            (entity.id, entity.position, aabb)
        })
        .collect();
    for (entity_id, entity_position, aabb) in snapshots {
        let min_distance = PLAYER_HALF_WIDTH + aabb.half_width;
        let dx = entity_position.x - player.x;
        let dz = entity_position.z - player.z;
        let distance = dx.hypot(dz);
        if distance >= min_distance || (entity_position.y - player.y).abs() > 1.5 {
            continue;
        }
        let (nx, nz) = if distance > 1.0e-6 {
            (dx / distance, dz / distance)
        } else {
            let yaw = f64::from(pose.yaw).to_radians();
            (yaw.sin(), -yaw.cos())
        };
        let push = min_distance - distance + 0.02;
        let _ = inner.entities.set_position(
            entity_id,
            Vec3::new(
                entity_position.x + nx * push,
                entity_position.y,
                entity_position.z + nz * push,
            ),
        );
    }
}

fn player_eye_position(pose: PlayerPose) -> Vec3 {
    Vec3::new(pose.x, pose.y + 1.62, pose.z)
}

fn block_center(position: i64) -> Vec3 {
    let (x, y, z) = unpack_block_pos(position);
    Vec3::new(x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5)
}

pub(super) fn within_block_reach(pose: PlayerPose, position: i64, game_mode: GameMode) -> bool {
    let max = if game_mode == GameMode::Creative {
        6.0
    } else {
        5.0
    };
    distance_sq(player_eye_position(pose), block_center(position)) <= max * max
}

pub(super) fn within_entity_reach(
    pose: PlayerPose,
    position: Vec3,
    aabb: mc_physics::Aabb,
    game_mode: GameMode,
) -> bool {
    let max = if game_mode == GameMode::Creative {
        6.0
    } else {
        5.0
    };
    distance_sq_to_entity_box(player_eye_position(pose), position, aabb) <= max * max
}

fn distance_sq_to_entity_box(point: Vec3, position: Vec3, aabb: mc_physics::Aabb) -> f64 {
    let dx = (point.x - position.x).abs() - aabb.half_width;
    let dz = (point.z - position.z).abs() - aabb.half_width;
    let dy = if point.y < position.y {
        position.y - point.y
    } else if point.y > position.y + aabb.height {
        point.y - (position.y + aabb.height)
    } else {
        0.0
    };
    dx.max(0.0).powi(2) + dy.powi(2) + dz.max(0.0).powi(2)
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

fn refresh_loaded_chunk_for_session_locked(
    inner: &mut SessionRegistryInner,
    observer_id: SessionId,
    chunk: (i32, i32),
) -> Vec<VisibilityDispatch> {
    let Some(recipient) = inner
        .sessions
        .get(&observer_id)
        .map(|session| SessionRecipient {
            id: observer_id,
            tx: session.tx.clone(),
        })
    else {
        return Vec::new();
    };
    let players = inner
        .sessions
        .iter()
        .filter(|&(&target_id, target)| {
            target_id != observer_id && target.pose.chunk_pos() == chunk
        })
        .map(|(&target_id, target)| (target_id, session_snapshot(target_id, target)))
        .collect::<Vec<_>>();
    let entities = inner
        .entities_by_chunk
        .get(&chunk)
        .into_iter()
        .flat_map(|entities| entities.iter().copied())
        .filter_map(|entity_id| {
            inner
                .entities
                .view(entity_id)
                .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
                .map(server_entity_snapshot_from_view)
        })
        .collect::<Vec<_>>();

    let Some(observer) = inner.sessions.get_mut(&observer_id) else {
        return Vec::new();
    };
    let mut dispatches = Vec::new();
    for (target_id, snapshot) in players {
        if observer.visible_players.insert(target_id) {
            dispatches.push(VisibilityDispatch {
                recipient: recipient.clone(),
                command: OutboundCommand::SpawnPlayer(snapshot),
            });
        }
    }
    for snapshot in entities {
        if observer.visible_entities.insert(snapshot.id) {
            dispatches.push(VisibilityDispatch {
                recipient: recipient.clone(),
                command: OutboundCommand::SpawnEntity(snapshot),
            });
        }
    }
    record_entity_dispatches_locked(inner, &dispatches);
    dispatches
}

fn refresh_unloaded_chunk_for_session_locked(
    inner: &mut SessionRegistryInner,
    observer_id: SessionId,
    chunk: (i32, i32),
) -> Vec<VisibilityDispatch> {
    let Some(observer) = inner.sessions.get(&observer_id) else {
        return Vec::new();
    };
    let recipient = SessionRecipient {
        id: observer_id,
        tx: observer.tx.clone(),
    };
    let visible_players = observer.visible_players.clone();
    let visible_entities = observer.visible_entities.clone();

    let players = visible_players
        .into_iter()
        .filter_map(|target_id| {
            let target = inner.sessions.get(&target_id)?;
            (target.pose.chunk_pos() == chunk)
                .then(|| (target_id, session_snapshot(target_id, target)))
        })
        .collect::<Vec<_>>();
    let entities = visible_entities
        .into_iter()
        .filter_map(|entity_id| {
            (inner.entity_chunks.get(&entity_id).copied() == Some(chunk)).then(|| {
                inner
                    .entities
                    .view(entity_id)
                    .map(server_entity_snapshot_from_view)
            })?
        })
        .collect::<Vec<_>>();

    let Some(observer) = inner.sessions.get_mut(&observer_id) else {
        return Vec::new();
    };
    let mut dispatches = Vec::new();
    for (target_id, snapshot) in players {
        if observer.visible_players.remove(&target_id) {
            dispatches.push(VisibilityDispatch {
                recipient: recipient.clone(),
                command: OutboundCommand::DespawnPlayer(snapshot),
            });
        }
    }
    for snapshot in entities {
        if observer.visible_entities.remove(&snapshot.id) {
            dispatches.push(VisibilityDispatch {
                recipient: recipient.clone(),
                command: OutboundCommand::DespawnEntity(snapshot),
            });
        }
    }
    record_entity_dispatches_locked(inner, &dispatches);
    dispatches
}

fn refresh_player_target_visibility_locked(
    inner: &mut SessionRegistryInner,
    target_id: SessionId,
    old_chunk: (i32, i32),
    new_chunk: (i32, i32),
) -> Vec<VisibilityDispatch> {
    let Some(snapshot) = inner
        .sessions
        .get(&target_id)
        .map(|session| session_snapshot(target_id, session))
    else {
        return Vec::new();
    };
    let mut observer_ids = visible_observers_locked(inner, target_id);
    for (&observer_id, observer) in &inner.sessions {
        if observer.loaded.contains(&old_chunk) || observer.loaded.contains(&new_chunk) {
            observer_ids.insert(observer_id);
        }
    }

    let mut dispatches = Vec::new();
    for observer_id in observer_ids {
        if observer_id == target_id {
            continue;
        }
        let Some(observer) = inner.sessions.get_mut(&observer_id) else {
            continue;
        };
        let desired = observer.loaded.contains(&new_chunk);
        let visible = observer.visible_players.contains(&target_id);
        match (desired, visible) {
            (true, false) => {
                observer.visible_players.insert(target_id);
                dispatches.push(VisibilityDispatch {
                    recipient: SessionRecipient {
                        id: observer_id,
                        tx: observer.tx.clone(),
                    },
                    command: OutboundCommand::SpawnPlayer(snapshot.clone()),
                });
            }
            (false, true) => {
                observer.visible_players.remove(&target_id);
                dispatches.push(VisibilityDispatch {
                    recipient: SessionRecipient {
                        id: observer_id,
                        tx: observer.tx.clone(),
                    },
                    command: OutboundCommand::DespawnPlayer(snapshot.clone()),
                });
            }
            _ => {}
        }
    }
    dispatches
}

fn spawn_entity_visibility_locked(
    inner: &mut SessionRegistryInner,
    entity_id: EntityId,
) -> Vec<VisibilityDispatch> {
    let Some(chunk) = inner.entity_chunks.get(&entity_id).copied() else {
        return Vec::new();
    };
    let Some(snapshot) = inner
        .entities
        .view(entity_id)
        .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
        .map(server_entity_snapshot_from_view)
    else {
        return Vec::new();
    };
    let mut dispatches = Vec::new();
    for (&observer_id, observer) in &mut inner.sessions {
        if observer.loaded.contains(&chunk) && observer.visible_entities.insert(entity_id) {
            dispatches.push(VisibilityDispatch {
                recipient: SessionRecipient {
                    id: observer_id,
                    tx: observer.tx.clone(),
                },
                command: OutboundCommand::SpawnEntity(snapshot.clone()),
            });
        }
    }
    record_entity_dispatches_locked(inner, &dispatches);
    dispatches
}

fn refresh_entity_target_visibility_locked(
    inner: &mut SessionRegistryInner,
    entity_id: EntityId,
    old_chunk: (i32, i32),
    new_chunk: (i32, i32),
) -> Vec<VisibilityDispatch> {
    let Some(snapshot) = inner
        .entities
        .view(entity_id)
        .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
        .map(server_entity_snapshot_from_view)
    else {
        return Vec::new();
    };
    let mut observer_ids = visible_entity_observers_locked(inner, entity_id);
    for (&observer_id, observer) in &inner.sessions {
        if observer.loaded.contains(&old_chunk) || observer.loaded.contains(&new_chunk) {
            observer_ids.insert(observer_id);
        }
    }

    let mut dispatches = Vec::new();
    for observer_id in observer_ids {
        let Some(observer) = inner.sessions.get_mut(&observer_id) else {
            continue;
        };
        let desired = observer.loaded.contains(&new_chunk);
        let visible = observer.visible_entities.contains(&entity_id);
        match (desired, visible) {
            (true, false) => {
                observer.visible_entities.insert(entity_id);
                dispatches.push(VisibilityDispatch {
                    recipient: SessionRecipient {
                        id: observer_id,
                        tx: observer.tx.clone(),
                    },
                    command: OutboundCommand::SpawnEntity(snapshot.clone()),
                });
            }
            (false, true) => {
                observer.visible_entities.remove(&entity_id);
                dispatches.push(VisibilityDispatch {
                    recipient: SessionRecipient {
                        id: observer_id,
                        tx: observer.tx.clone(),
                    },
                    command: OutboundCommand::DespawnEntity(snapshot.clone()),
                });
            }
            _ => {}
        }
    }
    record_entity_dispatches_locked(inner, &dispatches);
    dispatches
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
        .views()
        .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
        .map(|entity| {
            let snapshot = server_entity_snapshot_from_view(entity);
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
            let desired = observer
                .loaded
                .iter()
                .filter_map(|chunk| inner.entities_by_chunk.get(chunk))
                .flat_map(|entities| entities.iter().copied())
                .filter(|entity_id| entity_snapshots.contains_key(entity_id))
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
    record_entity_dispatches_locked(inner, &dispatches);
    dispatches
}

fn record_entity_dispatches_locked(
    inner: &mut SessionRegistryInner,
    dispatches: &[VisibilityDispatch],
) {
    for dispatch in dispatches {
        match dispatch.command {
            OutboundCommand::SpawnEntity(_) => inner.entity_dispatches.spawn += 1,
            OutboundCommand::UpdateEntityData(_) => inner.entity_dispatches.data += 1,
            OutboundCommand::MoveEntityRelative(_) => inner.entity_dispatches.move_relative += 1,
            OutboundCommand::TakeItemEntity { .. } => inner.entity_dispatches.take += 1,
            OutboundCommand::DespawnEntity(_) => inner.entity_dispatches.remove += 1,
            _ => {}
        }
    }
}

pub(super) fn dispatch_visibility_commands(dispatches: Vec<VisibilityDispatch>) {
    for dispatch in dispatches {
        let lane = dispatch.command.lane();
        let recipient_id = dispatch.recipient.id;
        match dispatch.recipient.tx.try_send(dispatch.command) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(command)) => match lane {
                OutboundLane::Reliable => {
                    retry_reliable_command(dispatch.recipient.tx, recipient_id, command)
                }
                OutboundLane::Coalescible => {
                    VISIBILITY_COMMAND_DROPS.fetch_add(1, Ordering::Relaxed);
                    debug!(
                        recipient = recipient_id,
                        "dropping coalescible outbound command"
                    );
                }
            },
            Err(mpsc::error::TrySendError::Closed(command)) => {
                VISIBILITY_COMMAND_DROPS.fetch_add(1, Ordering::Relaxed);
                match lane {
                    OutboundLane::Reliable => warn!(
                        recipient = recipient_id,
                        command = ?command,
                        "dropping reliable outbound command for closed session"
                    ),
                    OutboundLane::Coalescible => debug!(
                        recipient = recipient_id,
                        "dropping coalescible outbound command for closed session"
                    ),
                }
            }
        }
    }
}

fn retry_reliable_command(
    tx: mpsc::Sender<OutboundCommand>,
    recipient_id: SessionId,
    command: OutboundCommand,
) {
    RELIABLE_COMMAND_RETRIES.fetch_add(1, Ordering::Relaxed);
    RELIABLE_COMMAND_RETRIES_IN_FLIGHT.fetch_add(1, Ordering::Relaxed);
    tokio::spawn(async move {
        if tx.send(command).await.is_err() {
            VISIBILITY_COMMAND_DROPS.fetch_add(1, Ordering::Relaxed);
            debug!(
                recipient = recipient_id,
                "reliable outbound retry target closed"
            );
        }
        RELIABLE_COMMAND_RETRIES_IN_FLIGHT.fetch_sub(1, Ordering::Relaxed);
    });
}

fn picked_entity_dispatches_locked(
    inner: &mut SessionRegistryInner,
    entity_id: EntityId,
    collector_session: SessionId,
    amount: i32,
    snapshot: ServerEntitySnapshot,
) -> Vec<VisibilityDispatch> {
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
    record_entity_dispatches_locked(inner, &dispatches);
    dispatches
}

fn track_entity_chunk_locked(
    inner: &mut SessionRegistryInner,
    entity_id: EntityId,
    position: Vec3,
) {
    let chunk = chunk_pos_from_coords(position.x, position.z);
    inner.entity_chunks.insert(entity_id, chunk);
    inner
        .entities_by_chunk
        .entry(chunk)
        .or_default()
        .insert(entity_id);
}

fn move_entity_chunk_locked(
    inner: &mut SessionRegistryInner,
    entity_id: EntityId,
    old_chunk: (i32, i32),
    new_chunk: (i32, i32),
) {
    if let Some(entities) = inner.entities_by_chunk.get_mut(&old_chunk) {
        entities.remove(&entity_id);
        if entities.is_empty() {
            inner.entities_by_chunk.remove(&old_chunk);
        }
    }
    inner.entity_chunks.insert(entity_id, new_chunk);
    inner
        .entities_by_chunk
        .entry(new_chunk)
        .or_default()
        .insert(entity_id);
}

fn untrack_entity_chunk_locked(inner: &mut SessionRegistryInner, entity_id: EntityId) {
    if let Some(chunk) = inner.entity_chunks.remove(&entity_id)
        && let Some(entities) = inner.entities_by_chunk.get_mut(&chunk)
    {
        entities.remove(&entity_id);
        if entities.is_empty() {
            inner.entities_by_chunk.remove(&chunk);
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn profile(name: &str) -> LoggedInProfile {
        LoggedInProfile {
            uuid: crate::login::offline_uuid(name),
            name: name.to_string(),
        }
    }

    fn register_test_session(registry: &SessionRegistry, name: &str) -> SessionId {
        let (tx, _rx) = mpsc::channel(8);
        let (id, _) = registry.register(
            &profile(name),
            (0, 0),
            2,
            HashSet::new(),
            tx,
            PlayerPose::new(0.5, 64.0, 0.5),
        );
        id
    }

    fn registration<'a>(
        profile: &'a LoggedInProfile,
        tx: mpsc::Sender<OutboundCommand>,
        max_sessions: usize,
    ) -> SessionRegistration<'a> {
        SessionRegistration {
            profile,
            center: (0, 0),
            view_distance: 2,
            desired: HashSet::new(),
            tx,
            pose: PlayerPose::new(0.5, 64.0, 0.5),
            max_sessions,
        }
    }

    #[test]
    fn try_register_enforces_max_sessions() {
        let registry = SessionRegistry::new();
        let (tx, _rx) = mpsc::channel(8);
        let full_alice = profile("FullAlice");
        let first = registry.try_register(registration(&full_alice, tx, 1));
        assert!(first.is_ok());

        let (tx, _rx) = mpsc::channel(8);
        let full_bob = profile("FullBob");
        let second = registry.try_register(registration(&full_bob, tx, 1));
        assert!(matches!(
            second,
            Err(SessionAdmissionError::ServerFull { active: 1, max: 1 })
        ));
    }

    #[test]
    fn try_register_rejects_duplicate_profile_until_unregister() {
        let registry = SessionRegistry::new();
        let first_id = register_test_session(&registry, "DupAlice");
        let (tx, _rx) = mpsc::channel(8);

        let dup_alice = profile("DupAlice");
        let duplicate = registry.try_register(registration(&dup_alice, tx, 8));

        assert!(matches!(
            duplicate,
            Err(SessionAdmissionError::DuplicateProfile { existing_session })
                if existing_session == first_id
        ));
        let _ = registry.unregister(first_id);

        let (tx, _rx) = mpsc::channel(8);
        let dup_alice = profile("DupAlice");
        assert!(
            registry
                .try_register(registration(&dup_alice, tx, 8))
                .is_ok()
        );
    }

    #[test]
    fn same_chunk_pose_update_only_moves_existing_observers() {
        let registry = SessionRegistry::new();
        let (alice_tx, _alice_rx) = mpsc::channel(8);
        let (alice, _) = registry.register(
            &profile("MoveAlice"),
            (0, 0),
            2,
            HashSet::from([(0, 0)]),
            alice_tx,
            PlayerPose::new(0.5, 64.0, 0.5),
        );
        let (bob_tx, _bob_rx) = mpsc::channel(8);
        let (bob, _) = registry.register(
            &profile("MoveBob"),
            (0, 0),
            2,
            HashSet::from([(0, 0)]),
            bob_tx,
            PlayerPose::new(1.5, 64.0, 0.5),
        );
        let _ = registry.mark_loaded(alice, (0, 0));
        let _ = registry.mark_loaded(bob, (0, 0));

        let dispatches = registry.update_pose(bob, PlayerPose::new(2.5, 64.0, 0.5));

        assert_eq!(dispatches.len(), 1);
        assert!(matches!(
            &dispatches[0].command,
            OutboundCommand::MovePlayer(PlayerEntitySnapshot { session_id, .. }) if *session_id == bob
        ));
    }

    #[test]
    fn chunk_crossing_pose_update_diffs_target_observers() {
        let registry = SessionRegistry::new();
        let (alice_tx, _alice_rx) = mpsc::channel(8);
        let (alice, _) = registry.register(
            &profile("CrossAlice"),
            (0, 0),
            2,
            HashSet::from([(0, 0)]),
            alice_tx,
            PlayerPose::new(0.5, 64.0, 0.5),
        );
        let (bob_tx, _bob_rx) = mpsc::channel(8);
        let (bob, _) = registry.register(
            &profile("CrossBob"),
            (0, 0),
            2,
            HashSet::from([(0, 0), (1, 0)]),
            bob_tx,
            PlayerPose::new(1.5, 64.0, 0.5),
        );
        let (charlie_tx, _charlie_rx) = mpsc::channel(8);
        let (charlie, _) = registry.register(
            &profile("CrossCharlie"),
            (1, 0),
            2,
            HashSet::from([(1, 0)]),
            charlie_tx,
            PlayerPose::new(16.5, 64.0, 0.5),
        );
        let _ = registry.mark_loaded(alice, (0, 0));
        let _ = registry.mark_loaded(bob, (0, 0));
        let _ = registry.mark_loaded(charlie, (1, 0));

        let dispatches = registry.update_pose(bob, PlayerPose::new(16.5, 64.0, 0.5));

        assert_eq!(dispatches.len(), 2);
        assert!(dispatches.iter().any(|dispatch| matches!(
            &dispatch.command,
            OutboundCommand::DespawnPlayer(PlayerEntitySnapshot { session_id, .. })
                if *session_id == bob && dispatch.recipient.id == alice
        )));
        assert!(dispatches.iter().any(|dispatch| matches!(
            &dispatch.command,
            OutboundCommand::SpawnPlayer(PlayerEntitySnapshot { session_id, .. })
                if *session_id == bob && dispatch.recipient.id == charlie
        )));
    }

    #[test]
    fn item_pickup_claims_entity_once() {
        let registry = SessionRegistry::new();
        let alice = register_test_session(&registry, "PickupAlice");
        let bob = register_test_session(&registry, "PickupBob");
        registry.spawn_item_drop(1, Vec3::new(0.5, 64.0, 0.5), EntityItemStack::new(42, 3));
        assert!(
            registry
                .nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
                .is_empty()
        );

        registry.advance_world_time(ITEM_PICKUP_DELAY_TICKS);
        let entity_id = registry.nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)[0].id;

        let claimed = registry.claim_item_pickup(entity_id, alice, 3).unwrap();
        assert_eq!(claimed.stack, EntityItemStack::new(42, 3));
        assert!(registry.claim_item_pickup(entity_id, bob, 3).is_none());
        assert!(
            registry
                .nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
                .is_empty()
        );
    }

    #[test]
    fn pressure_snapshot_counts_entity_spawn_move_and_pickup_dispatches() {
        let registry = SessionRegistry::new();
        let alice = register_test_session(&registry, "DispatchAlice");
        assert!(registry.mark_loaded(alice, (0, 0)).is_empty());

        let start = registry.pressure_snapshot().entity_dispatches;
        let spawn_dispatches =
            registry.spawn_item_drop(1, Vec3::new(0.5, 64.0, 0.5), EntityItemStack::new(42, 3));
        assert_eq!(spawn_dispatches.len(), 1);

        let after_spawn = registry.pressure_snapshot().entity_dispatches;
        assert_eq!(after_spawn.spawn, start.spawn + 1);
        assert_eq!(after_spawn.move_relative, start.move_relative);
        assert_eq!(after_spawn.take, start.take);
        assert_eq!(after_spawn.remove, start.remove);

        let entity_id = {
            let inner = registry.inner.lock().expect("session registry poisoned");
            inner
                .entities
                .snapshots()
                .next()
                .expect("spawned entity")
                .id
        };
        registry.apply_entity_physics_and_dispatch(
            1,
            &[EntityPhysicsStep {
                id: entity_id,
                position: Vec3::new(0.75, 64.0, 0.5),
                velocity: Vec3::ZERO,
                on_ground: true,
            }],
        );

        let after_move = registry.pressure_snapshot().entity_dispatches;
        assert_eq!(after_move.move_relative, after_spawn.move_relative + 1);

        registry.advance_world_time(ITEM_PICKUP_DELAY_TICKS);
        let claimed = registry.claim_item_pickup(entity_id, alice, 3).unwrap();
        assert_eq!(claimed.dispatches.len(), 2);

        let after_pickup = registry.pressure_snapshot().entity_dispatches;
        assert_eq!(after_pickup.take, after_move.take + 1);
        assert_eq!(after_pickup.remove, after_move.remove + 1);
    }

    #[test]
    fn boundary_spawn_does_not_send_same_tick_relative_move_to_new_observer() {
        let registry = SessionRegistry::new();
        let (tx, mut rx) = mpsc::channel(8);
        let (alice, _) = registry.register(
            &profile("BoundaryAlice"),
            (0, 0),
            2,
            HashSet::new(),
            tx,
            PlayerPose::new(0.5, 64.0, 0.5),
        );
        assert!(registry.mark_loaded(alice, (1, 0)).is_empty());
        assert!(
            registry
                .spawn_command_entity(1, "minecraft:zombie".to_string(), Vec3::new(0.5, 64.0, 0.5))
                .is_empty()
        );
        let entity_id = {
            let inner = registry.inner.lock().expect("session registry poisoned");
            inner
                .entities
                .snapshots()
                .next()
                .expect("spawned entity")
                .id
        };

        registry.apply_entity_physics_and_dispatch(
            ENTITY_MOVE_SEND_INTERVAL_TICKS,
            &[EntityPhysicsStep {
                id: entity_id,
                position: Vec3::new(16.5, 64.0, 0.5),
                velocity: Vec3::ZERO,
                on_ground: true,
            }],
        );

        assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn moving_mobs_send_velocity_with_relative_move() {
        let registry = SessionRegistry::new();
        let (tx, mut rx) = mpsc::channel(8);
        let (alice, _) = registry.register(
            &profile("VelocityAlice"),
            (0, 0),
            2,
            HashSet::from([(0, 0)]),
            tx,
            PlayerPose::new(0.5, 64.0, 0.5),
        );
        assert!(registry.mark_loaded(alice, (0, 0)).is_empty());
        let spawn_dispatches = registry.spawn_command_entity(
            1,
            "minecraft:zombie".to_string(),
            Vec3::new(0.5, 64.0, 0.5),
        );
        dispatch_visibility_commands(spawn_dispatches);
        let entity_id = {
            let inner = registry.inner.lock().expect("session registry poisoned");
            inner
                .entities
                .snapshots()
                .next()
                .expect("spawned entity")
                .id
        };

        registry.apply_entity_physics_and_dispatch(
            ENTITY_MOVE_SEND_INTERVAL_TICKS,
            &[EntityPhysicsStep {
                id: entity_id,
                position: Vec3::new(0.5, 64.1, 0.5),
                velocity: Vec3::new(0.0, 0.05, 0.0),
                on_ground: false,
            }],
        );

        assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));
        let Ok(OutboundCommand::MoveEntityRelative(movement)) = rx.try_recv() else {
            panic!("expected relative mob movement");
        };
        assert!(movement.send_velocity);
    }

    #[test]
    fn item_drop_relative_move_does_not_emit_extra_velocity_packet() {
        let registry = SessionRegistry::new();
        let (tx, mut rx) = mpsc::channel(8);
        let (alice, _) = registry.register(
            &profile("ItemVelocityAlice"),
            (0, 0),
            2,
            HashSet::from([(0, 0)]),
            tx,
            PlayerPose::new(0.5, 64.0, 0.5),
        );
        assert!(registry.mark_loaded(alice, (0, 0)).is_empty());
        let spawn_dispatches =
            registry.spawn_item_drop(1, Vec3::new(0.5, 64.0, 0.5), EntityItemStack::new(42, 1));
        dispatch_visibility_commands(spawn_dispatches);
        let entity_id = {
            let inner = registry.inner.lock().expect("session registry poisoned");
            inner.entities.snapshots().next().expect("spawned item").id
        };

        registry.apply_entity_physics_and_dispatch(
            ENTITY_MOVE_SEND_INTERVAL_TICKS,
            &[EntityPhysicsStep {
                id: entity_id,
                position: Vec3::new(0.5, 64.1, 0.5),
                velocity: Vec3::new(0.0, 0.05, 0.0),
                on_ground: false,
            }],
        );

        assert!(matches!(rx.try_recv(), Ok(OutboundCommand::SpawnEntity(_))));
        let Ok(OutboundCommand::MoveEntityRelative(movement)) = rx.try_recv() else {
            panic!("expected relative item movement");
        };
        assert!(!movement.send_velocity);
    }

    #[test]
    fn pressure_snapshot_counts_dropped_visibility_commands() {
        let registry = SessionRegistry::new();
        let start = registry.pressure_snapshot().visibility_command_drops;
        let (tx, _rx) = mpsc::channel(1);
        tx.try_send(OutboundCommand::AnimatePlayer { entity_id: 1 })
            .expect("fill recipient queue");

        dispatch_visibility_commands(vec![VisibilityDispatch {
            recipient: SessionRecipient { id: 1, tx },
            command: OutboundCommand::AnimatePlayer { entity_id: 2 },
        }]);

        assert!(registry.pressure_snapshot().visibility_command_drops > start);
    }

    #[tokio::test]
    async fn reliable_visibility_commands_retry_when_channel_is_full() {
        let registry = SessionRegistry::new();
        let start = registry.pressure_snapshot().reliable_command_retries;
        let (tx, mut rx) = mpsc::channel(1);
        tx.try_send(OutboundCommand::AnimatePlayer { entity_id: 1 })
            .expect("fill recipient queue");

        dispatch_visibility_commands(vec![VisibilityDispatch {
            recipient: SessionRecipient { id: 7, tx },
            command: OutboundCommand::SpawnPlayer(PlayerEntitySnapshot {
                session_id: 7,
                entity_id: 7,
                uuid: uuid::Uuid::nil(),
                name: "RetryPlayer".to_string(),
                pose: PlayerPose::new(0.5, 64.0, 0.5),
            }),
        }]);

        let pressure = registry.pressure_snapshot();
        assert_eq!(pressure.reliable_command_retries, start + 1);
        assert!(pressure.reliable_command_retries_in_flight > 0);
        assert!(matches!(
            rx.recv().await,
            Some(OutboundCommand::AnimatePlayer { entity_id: 1 })
        ));
        assert!(matches!(
            rx.recv().await,
            Some(OutboundCommand::SpawnPlayer(PlayerEntitySnapshot {
                session_id: 7,
                ..
            }))
        ));
        for _ in 0..8 {
            if registry
                .pressure_snapshot()
                .reliable_command_retries_in_flight
                == 0
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            registry
                .pressure_snapshot()
                .reliable_command_retries_in_flight,
            0
        );
    }

    #[test]
    fn xp_pickup_removal_succeeds_once() {
        let registry = SessionRegistry::new();
        let alice = register_test_session(&registry, "XpAlice");
        let bob = register_test_session(&registry, "XpBob");
        registry.spawn_xp_orb(99, Vec3::new(1.0, 64.0, 1.0), 5);
        let entity_id = registry.nearby_experience_entities(Vec3::new(1.0, 64.0, 1.0), 2.25)[0].id;

        assert!(registry.remove_picked_item(entity_id, alice, 5).is_some());
        assert!(registry.remove_picked_item(entity_id, bob, 5).is_none());
    }

    #[test]
    fn damage_server_entity_respects_hurt_invulnerability_ticks() {
        let registry = SessionRegistry::new();
        let entity_id = {
            let mut inner = registry.inner.lock().expect("session registry poisoned");
            inner
                .entities
                .spawn(SpawnEntity::new(1, "minecraft:zombie", Vec3::ZERO))
        };

        let first = registry.damage_server_entity(entity_id, 5.0).unwrap();
        assert_eq!(first.snapshot.health, 15.0);
        assert!(registry.damage_server_entity(entity_id, 5.0).is_none());

        registry.advance_world_time(ENTITY_HURT_INVULNERABLE_TICKS - 1);
        assert!(registry.damage_server_entity(entity_id, 5.0).is_none());

        registry.advance_world_time(1);
        let second = registry.damage_server_entity(entity_id, 5.0).unwrap();
        assert_eq!(second.snapshot.health, 10.0);
    }

    #[test]
    fn xp_orbs_are_spawned_and_found_by_pickup_radius() {
        let registry = SessionRegistry::new();

        registry.spawn_xp_orb(99, Vec3::new(1.0, 64.0, 1.0), 5);

        let nearby = registry.nearby_experience_entities(Vec3::new(1.5, 64.0, 1.0), 2.25);
        assert_eq!(nearby.len(), 1);
        assert_eq!(nearby[0].type_name, "minecraft:experience_orb");
        assert_eq!(nearby[0].experience_value, Some(5));
        assert!(
            registry
                .nearby_experience_entities(Vec3::new(10.0, 64.0, 10.0), 2.25)
                .is_empty()
        );
    }
}
