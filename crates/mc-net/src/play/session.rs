use super::*;

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
}

#[derive(Debug, Clone)]
pub(super) struct ServerEntityMove {
    pub(super) id: EntityId,
    pub(super) delta: Vec3,
    pub(super) rotation: mc_entity::Rotation,
    pub(super) on_ground: bool,
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
    chest_viewers: HashMap<mc_world::BlockPos, HashMap<SessionId, FurnaceViewer>>,
    player_persistence: HashMap<SessionId, Arc<Mutex<PlayerPersistedState>>>,
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
            last_sent_entity_positions: HashMap::new(),
            spawned_entity_chunks: HashSet::new(),
            furnace_viewers: HashMap::new(),
            chest_viewers: HashMap::new(),
            player_persistence: HashMap::new(),
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

    pub(crate) fn set_world_time(&self, world_time: u64) {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        inner.world_time = world_time;
    }

    pub(crate) fn advance_world_time(&self, ticks: u64) -> u64 {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        inner.world_time = inner.world_time.wrapping_add(ticks);
        inner.world_time
    }

    pub(crate) fn world_time(&self) -> u64 {
        let inner = self.inner.lock().expect("session registry poisoned");
        inner.world_time
    }

    pub(super) fn register_player_persistence(
        &self,
        id: SessionId,
        state: Arc<Mutex<PlayerPersistedState>>,
    ) {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        inner.player_persistence.insert(id, state);
    }

    pub(crate) fn persisted_player_states(&self) -> Vec<(uuid::Uuid, PlayerPersistedState)> {
        let entries = {
            let inner = self.inner.lock().expect("session registry poisoned");
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
                (
                    uuid,
                    state
                        .lock()
                        .expect("player persistence state poisoned")
                        .clone(),
                )
            })
            .collect()
    }

    pub(super) fn register(
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

    pub(super) fn unregister(&self, id: SessionId) -> Vec<VisibilityDispatch> {
        let mut inner = self.inner.lock().expect("session registry poisoned");
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

    pub(super) fn unregister_furnace_viewer(&self, id: SessionId, position: mc_world::BlockPos) {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        if let Some(viewers) = inner.furnace_viewers.get_mut(&position) {
            viewers.remove(&id);
            if viewers.is_empty() {
                inner.furnace_viewers.remove(&position);
            }
        }
    }

    pub(super) fn register_chest_viewer(&self, id: SessionId, position: mc_world::BlockPos) {
        let mut inner = self.inner.lock().expect("session registry poisoned");
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
        let mut inner = self.inner.lock().expect("session registry poisoned");
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
        let inner = self.inner.lock().expect("session registry poisoned");
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

    pub(super) fn is_furnace_tick_owner(
        &self,
        position: mc_world::BlockPos,
        id: SessionId,
    ) -> bool {
        let inner = self.inner.lock().expect("session registry poisoned");
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

    pub(super) fn mark_loaded(&self, id: SessionId, chunk: (i32, i32)) -> Vec<VisibilityDispatch> {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        if let Some(session) = inner.sessions.get_mut(&id) {
            session.loaded.insert(chunk);
            refresh_visibility_locked(&mut inner)
        } else {
            Vec::new()
        }
    }

    pub(super) fn mark_unloaded(
        &self,
        id: SessionId,
        chunks: &[(i32, i32)],
    ) -> Vec<VisibilityDispatch> {
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

    pub(super) fn update_pose(&self, id: SessionId, pose: PlayerPose) -> Vec<VisibilityDispatch> {
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

    pub(super) fn broadcast_player_animation(&self, id: SessionId) -> Vec<VisibilityDispatch> {
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

    pub(super) fn ensure_chunk_herd(
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
            if spawn.hostile {
                entity.attributes.set_base(AttributeKind::AttackDamage, 3.0);
                entity
                    .attributes
                    .set_base(AttributeKind::MovementSpeed, 0.23);
            }
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

    pub(super) fn spawn_item_drop(
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

    pub(super) fn nearby_item_entities(
        &self,
        position: Vec3,
        radius: f64,
    ) -> Vec<ServerEntitySnapshot> {
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

    pub(super) fn nearby_hostile_entities(
        &self,
        position: Vec3,
        radius: f64,
    ) -> Vec<ServerEntitySnapshot> {
        let radius_sq = radius * radius;
        let inner = self.inner.lock().expect("session registry poisoned");
        inner
            .entities
            .snapshots()
            .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
            .filter(|entity| entity.item_stack.is_none())
            .filter(|entity| is_hostile_entity(&entity.type_name))
            .filter(|entity| distance_sq(entity.position, position) <= radius_sq)
            .map(server_entity_snapshot_from)
            .collect()
    }

    pub(super) fn server_entity_snapshot(
        &self,
        entity_id: EntityId,
    ) -> Option<ServerEntitySnapshot> {
        let inner = self.inner.lock().expect("session registry poisoned");
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
        let mut inner = self.inner.lock().expect("session registry poisoned");
        inner.entities.damage(entity_id, amount)
    }

    pub(super) fn update_item_stack(
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

    pub(super) fn remove_server_entity(
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

    pub(super) fn remove_picked_item(
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

    pub(crate) fn restore_persisted_entities(
        &self,
        entities: impl IntoIterator<Item = mc_entity::EntitySnapshot>,
    ) -> usize {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        let mut restored = 0;
        for entity in entities {
            let chunk = server_entity_chunk_pos(&server_entity_snapshot_from(entity.clone()));
            if inner.entities.insert_snapshot(entity) {
                inner.spawned_entity_chunks.insert(chunk);
                restored += 1;
            }
        }
        restored
    }

    pub(crate) fn persisted_entity_snapshots(&self) -> Vec<mc_entity::EntitySnapshot> {
        let inner = self.inner.lock().expect("session registry poisoned");
        inner
            .entities
            .snapshots()
            .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
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

    pub(super) fn loaded_recipients_for_chunks(
        &self,
        chunks: &HashSet<(i32, i32)>,
        except: Option<SessionId>,
    ) -> Vec<SessionRecipient> {
        let inner = self.inner.lock().expect("session registry poisoned");
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
        let inner = self.inner.lock().expect("session registry poisoned");
        inner.prepared.get(&chunk).cloned()
    }

    pub(super) fn cache_prepared_chunk(
        &self,
        chunk: (i32, i32),
        prepared: Arc<PreparedChunkFrame>,
    ) {
        let mut inner = self.inner.lock().expect("session registry poisoned");
        if inner.tickets.contains_key(&chunk) {
            inner.prepared.entry(chunk).or_insert(prepared);
        }
    }

    pub(super) fn invalidate_prepared_chunks(&self, chunks: &HashSet<(i32, i32)>) {
        if chunks.is_empty() {
            return;
        }
        let mut inner = self.inner.lock().expect("session registry poisoned");
        for chunk in chunks {
            inner.prepared.remove(chunk);
        }
    }

    pub(crate) fn ticketed_chunks_sorted(&self) -> Vec<(i32, i32)> {
        let inner = self.inner.lock().expect("session registry poisoned");
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
    }
}

fn server_entity_chunk_pos(entity: &ServerEntitySnapshot) -> (i32, i32) {
    chunk_pos_from_coords(entity.position.x, entity.position.z)
}

pub(super) fn distance_sq(a: Vec3, b: Vec3) -> f64 {
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

pub(super) fn within_block_reach(pose: PlayerPose, position: i64, game_mode: GameMode) -> bool {
    let max = if game_mode == GameMode::Creative {
        6.0
    } else {
        5.0
    };
    distance_sq(player_eye_position(pose), block_center(position)) <= max * max
}

pub(super) fn within_entity_reach(pose: PlayerPose, position: Vec3, game_mode: GameMode) -> bool {
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

pub(super) fn dispatch_visibility_commands(dispatches: Vec<VisibilityDispatch>) {
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
