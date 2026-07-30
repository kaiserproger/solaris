use std::collections::{BTreeMap, HashMap, VecDeque, hash_map::Entry};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use mc_entity::{EntityId, EntityItemStack, Rotation, Vec3};
use mc_nbt::Tag;
use mc_protocol::codec::Identifier;
use mc_protocol::packets::play::{
    ClientboundExplode, EntityDataValue, ItemStack, LevelEvent, LightData,
};
use mc_world::ChunkPos;
use mc_world::light::ChunkLight;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::play::PlayerPose;
use crate::play::block_wire::BlockDelta;
use crate::play::combat::{MeleeKnockback, PlayerDamageRequest};
use crate::play::inventory::PlayerInventory;
use crate::play::persistence::XpState;
use crate::play::session::{
    LoaderItemGrantCommand, ScriptMenuCloseRequest, ScriptMenuOpenRequest,
    ScriptPlayerInventoryCommand, ScriptPlayerTeleportCommand,
};
use crate::play::wire_entities::ServerEntityWireMove;

use super::{SessionId, SessionRegistryInner};

#[derive(Debug, Clone)]
pub(in crate::play) struct PlayerEntitySnapshot {
    pub(in crate::play) session_id: SessionId,
    pub(in crate::play) entity_id: i32,
    pub(in crate::play) uuid: uuid::Uuid,
    pub(in crate::play) name: String,
    pub(in crate::play) properties: Vec<mc_protocol::packets::login::GameProfileProperty>,
    pub(in crate::play) pose: PlayerPose,
}

#[derive(Debug, Clone)]
pub(in crate::play) struct ServerEntitySnapshot {
    pub(in crate::play) id: EntityId,
    pub(in crate::play) uuid: uuid::Uuid,
    pub(in crate::play) type_id: i32,
    pub(in crate::play) type_name: String,
    pub(in crate::play) position: Vec3,
    pub(in crate::play) rotation: Rotation,
    pub(in crate::play) velocity: Vec3,
    pub(in crate::play) on_ground: bool,
    pub(in crate::play) health: Option<f32>,
    pub(in crate::play) item_stack: Option<EntityItemStack>,
    pub(in crate::play) experience_value: Option<i32>,
    pub(in crate::play) block_state: Option<u32>,
    pub(in crate::play) animal: Option<mc_entity::AnimalBreedingState>,
    pub(in crate::play) villager: Option<mc_entity::VillagerData>,
    pub(in crate::play) villager_baby: bool,
    pub(in crate::play) main_hand_item: Option<EntityItemStack>,
    pub(in crate::play) crossbow_charging: bool,
    pub(in crate::play) guardian_attack_target_entity_id: i32,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::play) struct ServerEntityMove {
    pub(in crate::play) id: EntityId,
    pub(in crate::play) position: Vec3,
    pub(in crate::play) wire_move: Option<ServerEntityWireMove>,
    pub(in crate::play) velocity: Vec3,
    pub(in crate::play) rotation: Rotation,
    pub(in crate::play) on_ground: bool,
    pub(in crate::play) send_velocity: bool,
    pub(in crate::play) send_head_rotation: bool,
}

#[derive(Debug)]
pub(in crate::play) enum OutboundCommand {
    BlockDeltas(Vec<BlockDelta>),
    LightUpdates(Vec<OutboundLightUpdate>),
    SpawnPlayer(PlayerEntitySnapshot),
    MovePlayer(PlayerEntitySnapshot),
    DespawnPlayer(PlayerEntitySnapshot),
    SpawnEntity(ServerEntitySnapshot),
    SpawnEntities(Vec<ServerEntitySnapshot>),
    UpdateEntityData(ServerEntitySnapshot),
    UpdateEntityHealth(ServerEntitySnapshot),
    MoveEntityRelative(ServerEntityMove),
    MoveEntitiesRelative(Vec<ServerEntityMove>),
    EntityEvent {
        entity_id: i32,
        event_id: i8,
    },
    LevelEvent(LevelEvent),
    DamagePlayer {
        damage: PlayerDamageRequest,
    },
    PlayerDamageCommitted {
        publication: Box<PlayerDamagePublication>,
        hurt_event: PlayerHurtEvent,
    },
    TakeItemEntity {
        item_entity_id: i32,
        player_entity_id: i32,
        amount: i32,
    },
    PickupCandidates(Vec<ServerEntitySnapshot>),
    DespawnEntity(ServerEntitySnapshot),
    AnimatePlayer {
        entity_id: i32,
    },
    PlayerEntityData {
        entity_id: i32,
        values: Vec<EntityDataValue>,
    },
    BlockEntityData {
        position: mc_world::BlockPos,
        block_entity_type: i32,
        nbt: Tag,
    },
    FurnaceSlots {
        position: mc_world::BlockPos,
        state_id: i32,
        slots: [ItemStack; 3],
    },
    ChestSlots {
        position: mc_world::BlockPos,
        state_id: i32,
        slots: Vec<ItemStack>,
    },
    FurnaceData {
        position: mc_world::BlockPos,
        changed: Vec<(i16, i16)>,
    },
    CustomPayload {
        channel: Identifier,
        payload: Vec<u8>,
    },
    SystemChat {
        message: String,
    },
    WorldTime {
        world_time: u64,
        rate: f32,
    },
    WakeFromBed {
        bed: mc_world::BlockPos,
    },
    DisconnectPlayer {
        reason: String,
    },
    OpenScriptMenu(ScriptMenuOpenRequest),
    CloseScriptMenu(ScriptMenuCloseRequest),
    ScriptPlayerTeleport(ScriptPlayerTeleportCommand),
    ScriptPlayerInventoryTransaction(ScriptPlayerInventoryCommand),
    LoaderItemGrant(LoaderItemGrantCommand),
    AuthoritativeInventory {
        inventory: Box<PlayerInventory>,
        carried_item: ItemStack,
    },
    Explosion(ClientboundExplode),
}

#[derive(Debug)]
pub(in crate::play) struct PlayerDamagePublication {
    pub(in crate::play) expected_health: f32,
    pub(in crate::play) health: f32,
    pub(in crate::play) inventory: Vec<PlayerInventorySlotDelta>,
    pub(in crate::play) carried_item: Option<PlayerCarriedItemDelta>,
    pub(in crate::play) xp: Option<PlayerXpDelta>,
    pub(in crate::play) died: bool,
    pub(in crate::play) fresh_hurt: bool,
    pub(in crate::play) shield_blocked: bool,
    pub(in crate::play) shield_cooldown: Option<ShieldCooldownPublication>,
    pub(in crate::play) knockback: Option<MeleeKnockback>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::play) struct ShieldCooldownPublication {
    pub(in crate::play) cooldown_group: mc_data::Identifier,
    pub(in crate::play) duration: i32,
}

#[derive(Debug)]
pub(in crate::play) struct PlayerInventorySlotDelta {
    pub(in crate::play) slot: usize,
    pub(in crate::play) expected: ItemStack,
    pub(in crate::play) updated: ItemStack,
}

#[derive(Debug)]
pub(in crate::play) struct PlayerCarriedItemDelta {
    pub(in crate::play) expected: ItemStack,
    pub(in crate::play) updated: ItemStack,
}

#[derive(Debug)]
pub(in crate::play) struct PlayerXpDelta {
    pub(in crate::play) expected: XpState,
    pub(in crate::play) updated: XpState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutboundLane {
    Reliable,
    BestEffort,
}

pub(super) const MIN_RELIABLE_RETRY_QUEUE_CAPACITY: usize = 16;
pub(super) const RELIABLE_RETRY_OVERFLOW_REASON: &str =
    "Client cannot keep up with reliable server updates";

impl OutboundCommand {
    fn lane(&self) -> OutboundLane {
        match self {
            Self::AnimatePlayer { .. } => OutboundLane::BestEffort,
            Self::SpawnPlayer(_)
            | Self::MovePlayer(_)
            | Self::DespawnPlayer(_)
            | Self::SpawnEntity(_)
            | Self::SpawnEntities(_)
            | Self::UpdateEntityData(_)
            | Self::UpdateEntityHealth(_)
            | Self::MoveEntityRelative(_)
            | Self::MoveEntitiesRelative(_)
            | Self::BlockDeltas(_)
            | Self::LightUpdates(_)
            | Self::EntityEvent { .. }
            | Self::LevelEvent(_)
            | Self::DamagePlayer { .. }
            | Self::PlayerDamageCommitted { .. }
            | Self::TakeItemEntity { .. }
            | Self::PickupCandidates(_)
            | Self::DespawnEntity(_)
            | Self::PlayerEntityData { .. }
            | Self::BlockEntityData { .. }
            | Self::FurnaceSlots { .. }
            | Self::ChestSlots { .. }
            | Self::FurnaceData { .. }
            | Self::CustomPayload { .. }
            | Self::SystemChat { .. }
            | Self::WorldTime { .. }
            | Self::WakeFromBed { .. }
            | Self::DisconnectPlayer { .. }
            | Self::OpenScriptMenu(_)
            | Self::CloseScriptMenu(_)
            | Self::ScriptPlayerTeleport(_)
            | Self::ScriptPlayerInventoryTransaction(_)
            | Self::LoaderItemGrant(_)
            | Self::AuthoritativeInventory { .. }
            | Self::Explosion(_) => OutboundLane::Reliable,
        }
    }
}

#[derive(Debug, Clone)]
pub(in crate::play) struct OutboundLightUpdate {
    pub(in crate::play) pos: ChunkPos,
    pub(in crate::play) light: ChunkLight,
    pub(in crate::play) wire: LightData,
}

#[derive(Debug, Clone)]
pub(in crate::play) struct SessionRecipient {
    pub(in crate::play) id: SessionId,
    pub(in crate::play) tx: mpsc::Sender<OutboundCommand>,
    pub(super) pressure: Arc<OutboundPressureMetrics>,
    ordered: Option<OrderedDispatchReservation>,
}

impl SessionRecipient {
    pub(super) fn unordered(
        id: SessionId,
        tx: mpsc::Sender<OutboundCommand>,
        pressure: Arc<OutboundPressureMetrics>,
    ) -> Self {
        Self {
            id,
            tx,
            pressure,
            ordered: None,
        }
    }

    pub(super) fn ordered(
        id: SessionId,
        tx: mpsc::Sender<OutboundCommand>,
        pressure: Arc<OutboundPressureMetrics>,
        state: &Arc<OrderedDispatchState>,
    ) -> Self {
        Self::ordered_with_cancel_action(id, tx, pressure, state, OrderedCancelAction::Skip)
    }

    pub(super) fn ordered_spawn(
        id: SessionId,
        tx: mpsc::Sender<OutboundCommand>,
        pressure: Arc<OutboundPressureMetrics>,
        state: &Arc<OrderedDispatchState>,
    ) -> Self {
        Self::ordered_with_cancel_action(id, tx, pressure, state, OrderedCancelAction::Disconnect)
    }

    fn ordered_with_cancel_action(
        id: SessionId,
        tx: mpsc::Sender<OutboundCommand>,
        pressure: Arc<OutboundPressureMetrics>,
        state: &Arc<OrderedDispatchState>,
        cancel_action: OrderedCancelAction,
    ) -> Self {
        let sequence = state.reserve();
        let target = OrderedDispatchTarget {
            id,
            tx: tx.clone(),
            pressure: Arc::clone(&pressure),
        };
        Self {
            id,
            tx,
            pressure,
            ordered: Some(OrderedDispatchReservation {
                sequence,
                token: Arc::new(OrderedDispatchToken {
                    state: Arc::clone(state),
                    target,
                    sequence,
                    committed: AtomicBool::new(false),
                    cancel_action,
                }),
            }),
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct OrderedDispatchState {
    next_planned: AtomicU64,
    queue: Mutex<OrderedDispatchQueue>,
}

impl OrderedDispatchState {
    fn reserve(&self) -> u64 {
        self.next_planned
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |sequence| {
                sequence.checked_add(1)
            })
            .expect("session outbound sequence exhausted")
    }

    pub(super) fn close(&self) -> usize {
        let mut queue = lock_ordered_dispatch_state(self);
        if queue.closing {
            return 0;
        }
        queue.closing = true;
        let dropped = queue.pending.values().map(ordered_queue_entry_weight).sum();
        queue.pending.clear();
        dropped
    }
}

#[derive(Debug, Default)]
struct OrderedDispatchQueue {
    next_dispatched: u64,
    pending: BTreeMap<u64, OrderedQueueEntry>,
    closing: bool,
}

#[derive(Debug, Clone)]
struct OrderedDispatchReservation {
    sequence: u64,
    token: Arc<OrderedDispatchToken>,
}

#[derive(Debug)]
struct OrderedDispatchToken {
    state: Arc<OrderedDispatchState>,
    target: OrderedDispatchTarget,
    sequence: u64,
    committed: AtomicBool,
    cancel_action: OrderedCancelAction,
}

impl Drop for OrderedDispatchToken {
    fn drop(&mut self) {
        if self.committed.load(Ordering::Acquire) {
            return;
        }
        cancel_ordered_reservation(self);
    }
}

#[derive(Debug, Clone)]
struct OrderedDispatchTarget {
    id: SessionId,
    tx: mpsc::Sender<OutboundCommand>,
    pressure: Arc<OutboundPressureMetrics>,
}

impl OrderedDispatchTarget {
    fn recipient(&self) -> SessionRecipient {
        SessionRecipient::unordered(self.id, self.tx.clone(), Arc::clone(&self.pressure))
    }
}

#[derive(Debug)]
enum OrderedQueueEntry {
    Canceled {
        action: OrderedCancelAction,
        target: OrderedDispatchTarget,
    },
    Commands {
        target: OrderedDispatchTarget,
        commands: Vec<OutboundCommand>,
    },
}

#[derive(Debug, Clone, Copy)]
enum OrderedCancelAction {
    Skip,
    Disconnect,
}

fn lock_ordered_dispatch_state(
    state: &OrderedDispatchState,
) -> MutexGuard<'_, OrderedDispatchQueue> {
    state.queue.lock().unwrap_or_else(|poisoned| {
        warn!("ordered outbound queue was poisoned; recovering state");
        poisoned.into_inner()
    })
}

#[derive(Debug, Clone)]
pub(in crate::play) struct PlayerHurtEvent {
    pub(in crate::play) entity_id: i32,
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
    pub(crate) best_effort_animation_drops: u64,
    pub(crate) reliable_command_drops: u64,
    pub(crate) reliable_command_retries: u64,
    pub(crate) reliable_command_retries_in_flight: u64,
    pub(crate) max_reliable_command_retries_in_flight: u64,
    pub(crate) slow_client_write_timeouts: u64,
    pub(crate) slow_client_pressure_sheds: u64,
}

#[derive(Debug)]
pub(super) struct OutboundPressureMetrics {
    best_effort_animation_drops: AtomicU64,
    reliable_command_drops: AtomicU64,
    reliable_command_retries: AtomicU64,
    reliable_command_retries_in_flight: AtomicU64,
    reliable_command_retries_max_in_flight: AtomicU64,
    slow_client_write_timeouts: AtomicU64,
    slow_client_pressure_sheds: AtomicU64,
    change_generation: AtomicU64,
    changed: tokio::sync::Notify,
    reliable_retry_queues: Mutex<HashMap<SessionId, ReliableRetryQueue>>,
    #[cfg(test)]
    pub(super) reliable_retry_completed: tokio::sync::Notify,
    #[cfg(test)]
    pub(super) reliable_retry_dequeued: tokio::sync::Notify,
}

impl Default for OutboundPressureMetrics {
    fn default() -> Self {
        Self {
            best_effort_animation_drops: AtomicU64::new(0),
            reliable_command_drops: AtomicU64::new(0),
            reliable_command_retries: AtomicU64::new(0),
            reliable_command_retries_in_flight: AtomicU64::new(0),
            reliable_command_retries_max_in_flight: AtomicU64::new(0),
            slow_client_write_timeouts: AtomicU64::new(0),
            slow_client_pressure_sheds: AtomicU64::new(0),
            change_generation: AtomicU64::new(0),
            changed: tokio::sync::Notify::new(),
            reliable_retry_queues: Mutex::new(HashMap::new()),
            #[cfg(test)]
            reliable_retry_completed: tokio::sync::Notify::new(),
            #[cfg(test)]
            reliable_retry_dequeued: tokio::sync::Notify::new(),
        }
    }
}

impl OutboundPressureMetrics {
    pub(super) fn lock_reliable_retry_queues(
        &self,
    ) -> MutexGuard<'_, HashMap<SessionId, ReliableRetryQueue>> {
        self.reliable_retry_queues
            .lock()
            .unwrap_or_else(|poisoned| {
                warn!("reliable retry queue map was poisoned; recovering state");
                poisoned.into_inner()
            })
    }

    fn record_reliable_retry_in_flight(&self, current: u64) {
        let mut observed = self
            .reliable_command_retries_max_in_flight
            .load(Ordering::Relaxed);
        while current > observed {
            match self
                .reliable_command_retries_max_in_flight
                .compare_exchange_weak(observed, current, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return,
                Err(actual) => observed = actual,
            }
        }
    }

    fn record_best_effort_animation_drop(&self) {
        self.best_effort_animation_drops
            .fetch_add(1, Ordering::Relaxed);
        self.mark_changed();
    }

    pub(super) fn record_reliable_command_drops(&self, count: usize) {
        if count == 0 {
            return;
        }
        self.reliable_command_drops
            .fetch_add(count as u64, Ordering::Relaxed);
        self.mark_changed();
    }

    pub(super) fn record_reliable_retry_started(&self) -> u64 {
        let worker_id = self
            .reliable_command_retries
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        let in_flight = self
            .reliable_command_retries_in_flight
            .fetch_add(1, Ordering::Relaxed)
            + 1;
        self.record_reliable_retry_in_flight(in_flight);
        self.mark_changed();
        worker_id
    }

    fn record_reliable_retry_finished(&self) {
        self.reliable_command_retries_in_flight
            .fetch_sub(1, Ordering::Relaxed);
        self.mark_changed();
    }

    pub(super) fn record_slow_client_write_timeout(&self) {
        self.slow_client_write_timeouts
            .fetch_add(1, Ordering::Relaxed);
        self.mark_changed();
    }

    pub(super) fn record_slow_client_pressure_shed(&self) {
        self.slow_client_pressure_sheds
            .fetch_add(1, Ordering::Relaxed);
        self.mark_changed();
    }

    fn mark_changed(&self) {
        self.change_generation.fetch_add(1, Ordering::Release);
        self.changed.notify_waiters();
    }

    pub(super) fn change_generation(&self) -> u64 {
        self.change_generation.load(Ordering::Acquire)
    }

    pub(super) async fn wait_for_change(&self, observed: u64) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.change_generation() != observed {
                return;
            }
            changed.await;
        }
    }
}

#[derive(Debug)]
pub(super) struct ReliableRetryQueue {
    pending: VecDeque<OutboundCommand>,
    capacity: usize,
    closing: bool,
    worker_id: u64,
}

impl ReliableRetryQueue {
    pub(super) fn new(command: OutboundCommand, capacity: usize, worker_id: u64) -> Self {
        let closing = matches!(command, OutboundCommand::DisconnectPlayer { .. });
        Self {
            pending: VecDeque::from([command]),
            capacity,
            closing,
            worker_id,
        }
    }

    fn enqueue(&mut self, command: OutboundCommand) -> ReliableEnqueueResult {
        if self.closing {
            return ReliableEnqueueResult::Dropped;
        }
        let command = match self.pending.back_mut() {
            Some(pending) => {
                let command = match try_coalesce_entity_spawns(pending, command) {
                    None => return ReliableEnqueueResult::Queued,
                    Some(command) => command,
                };
                match try_coalesce_entity_movements(pending, command) {
                    None => return ReliableEnqueueResult::Queued,
                    Some(command) => command,
                }
            }
            None => command,
        };
        if self.pending.len() < self.capacity {
            self.closing = matches!(command, OutboundCommand::DisconnectPlayer { .. });
            self.pending.push_back(command);
            return ReliableEnqueueResult::Queued;
        }

        let dropped = self.pending.len() + 1;
        self.pending.clear();
        self.pending.push_back(OutboundCommand::DisconnectPlayer {
            reason: RELIABLE_RETRY_OVERFLOW_REASON.to_string(),
        });
        self.closing = true;
        ReliableEnqueueResult::Shed { dropped }
    }
}

fn try_coalesce_entity_spawns(
    pending: &mut OutboundCommand,
    command: OutboundCommand,
) -> Option<OutboundCommand> {
    let mut incoming = match command {
        OutboundCommand::SpawnEntity(entity) => vec![entity],
        OutboundCommand::SpawnEntities(entities) => entities,
        command => return Some(command),
    };
    let existing = match pending {
        OutboundCommand::SpawnEntity(entity) => {
            let entity = entity.clone();
            *pending = OutboundCommand::SpawnEntities(vec![entity]);
            let OutboundCommand::SpawnEntities(existing) = pending else {
                unreachable!();
            };
            existing
        }
        OutboundCommand::SpawnEntities(existing) => existing,
        _ if incoming.len() == 1 => return Some(OutboundCommand::SpawnEntity(incoming.remove(0))),
        _ => return Some(OutboundCommand::SpawnEntities(incoming)),
    };
    existing.append(&mut incoming);
    None
}

fn try_coalesce_entity_movements(
    pending: &mut OutboundCommand,
    command: OutboundCommand,
) -> Option<OutboundCommand> {
    let (incoming, was_single) = match command {
        OutboundCommand::MoveEntityRelative(movement) => (vec![movement], true),
        OutboundCommand::MoveEntitiesRelative(movements) => (movements, false),
        command => return Some(command),
    };
    let existing = match pending {
        OutboundCommand::MoveEntityRelative(movement) => {
            let movement = *movement;
            *pending = OutboundCommand::MoveEntitiesRelative(vec![movement]);
            let OutboundCommand::MoveEntitiesRelative(existing) = pending else {
                unreachable!();
            };
            existing
        }
        OutboundCommand::MoveEntitiesRelative(existing) => existing,
        _ if was_single => return Some(OutboundCommand::MoveEntityRelative(incoming[0])),
        _ => return Some(OutboundCommand::MoveEntitiesRelative(incoming)),
    };

    if existing.len() == incoming.len()
        && existing
            .iter()
            .zip(&incoming)
            .all(|(left, right)| left.id == right.id)
    {
        for (existing, incoming) in existing.iter_mut().zip(incoming) {
            merge_entity_movement(existing, incoming);
        }
        return None;
    }

    let mut by_id = existing
        .iter()
        .enumerate()
        .map(|(index, movement)| (movement.id, index))
        .collect::<HashMap<_, _>>();
    for incoming in incoming {
        if let Some(&index) = by_id.get(&incoming.id) {
            merge_entity_movement(&mut existing[index], incoming);
        } else {
            let index = existing.len();
            by_id.insert(incoming.id, index);
            existing.push(incoming);
        }
    }
    None
}

fn merge_entity_movement(existing: &mut ServerEntityMove, mut incoming: ServerEntityMove) {
    let send_position_or_rotation = existing.wire_move.is_some() || incoming.wire_move.is_some();
    incoming.send_velocity |= existing.send_velocity;
    incoming.send_head_rotation |= existing.send_head_rotation;
    if send_position_or_rotation {
        incoming.wire_move = Some(ServerEntityWireMove::Absolute {
            position: incoming.position,
        });
    }
    *existing = incoming;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReliableEnqueueResult {
    Queued,
    Dropped,
    Shed { dropped: usize },
}

pub(super) struct ReliableRetryWorkerGuard {
    pub(super) recipient_id: SessionId,
    pub(super) worker_id: u64,
    pub(super) pressure: Arc<OutboundPressureMetrics>,
}

impl Drop for ReliableRetryWorkerGuard {
    fn drop(&mut self) {
        let mut queues = self.pressure.lock_reliable_retry_queues();
        if queues
            .get(&self.recipient_id)
            .is_some_and(|queue| queue.worker_id == self.worker_id)
        {
            queues.remove(&self.recipient_id);
        }
        drop(queues);
        self.pressure.record_reliable_retry_finished();
        #[cfg(test)]
        self.pressure.reliable_retry_completed.notify_waiters();
    }
}

#[derive(Debug)]
pub(in crate::play) struct VisibilityDispatch {
    pub(crate) recipient: SessionRecipient,
    pub(crate) command: OutboundCommand,
}

// Runtime observers read this producer-published projection without entering gameplay locks.
#[derive(Debug, Default)]
pub(super) struct SessionPressureObservation {
    sessions: AtomicUsize,
    ticketed_chunks: AtomicUsize,
    pub(super) prepared_chunks: AtomicUsize,
    server_entities: AtomicUsize,
    furnace_viewer_sets: AtomicUsize,
    chest_viewer_sets: AtomicUsize,
    entity_spawn_dispatches: AtomicU64,
    entity_move_dispatches: AtomicU64,
    unlocked_entity_move_dispatches: AtomicU64,
    entity_data_dispatches: AtomicU64,
    entity_take_dispatches: AtomicU64,
    entity_remove_dispatches: AtomicU64,
}

fn adjust_atomic_count(counter: &AtomicUsize, before: usize, after: usize) {
    if after >= before {
        counter.fetch_add(after - before, Ordering::Relaxed);
    } else {
        let removed = before - after;
        let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            Some(current.saturating_sub(removed))
        });
    }
}

impl SessionPressureObservation {
    pub(super) fn publish_sessions(&self, inner: &SessionRegistryInner) {
        self.sessions.store(inner.sessions.len(), Ordering::Relaxed);
        self.ticketed_chunks
            .store(inner.tickets.len(), Ordering::Relaxed);
        self.entity_spawn_dispatches
            .store(inner.entity_dispatches.spawn, Ordering::Relaxed);
        self.entity_move_dispatches
            .store(inner.entity_dispatches.move_relative, Ordering::Relaxed);
        self.entity_data_dispatches
            .store(inner.entity_dispatches.data, Ordering::Relaxed);
        self.entity_take_dispatches
            .store(inner.entity_dispatches.take, Ordering::Relaxed);
        self.entity_remove_dispatches
            .store(inner.entity_dispatches.remove, Ordering::Relaxed);
    }

    pub(super) fn record_entity_inserts(&self, count: usize) {
        self.server_entities.fetch_add(count, Ordering::Relaxed);
    }

    pub(super) fn record_unlocked_entity_move_dispatches(&self, count: usize) {
        self.unlocked_entity_move_dispatches
            .fetch_add(count as u64, Ordering::Relaxed);
    }

    pub(super) fn record_entity_remove(&self) {
        let _ = self
            .server_entities
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                Some(count.saturating_sub(1))
            });
    }

    pub(super) fn record_container_viewer_set_change(
        &self,
        before_furnaces: usize,
        after_furnaces: usize,
        before_chests: usize,
        after_chests: usize,
    ) {
        adjust_atomic_count(&self.furnace_viewer_sets, before_furnaces, after_furnaces);
        adjust_atomic_count(&self.chest_viewer_sets, before_chests, after_chests);
    }

    pub(super) fn snapshot(&self, outbound: &OutboundPressureMetrics) -> SessionPressureSnapshot {
        SessionPressureSnapshot {
            sessions: self.sessions.load(Ordering::Relaxed),
            ticketed_chunks: self.ticketed_chunks.load(Ordering::Relaxed),
            prepared_chunks: self.prepared_chunks.load(Ordering::Relaxed),
            server_entities: self.server_entities.load(Ordering::Relaxed),
            furnace_viewer_sets: self.furnace_viewer_sets.load(Ordering::Relaxed),
            chest_viewer_sets: self.chest_viewer_sets.load(Ordering::Relaxed),
            entity_dispatches: EntityDispatchCounters {
                spawn: self.entity_spawn_dispatches.load(Ordering::Relaxed),
                move_relative: self.entity_move_dispatches.load(Ordering::Relaxed)
                    + self.unlocked_entity_move_dispatches.load(Ordering::Relaxed),
                data: self.entity_data_dispatches.load(Ordering::Relaxed),
                take: self.entity_take_dispatches.load(Ordering::Relaxed),
                remove: self.entity_remove_dispatches.load(Ordering::Relaxed),
            },
            best_effort_animation_drops: outbound
                .best_effort_animation_drops
                .load(Ordering::Relaxed),
            reliable_command_drops: outbound.reliable_command_drops.load(Ordering::Relaxed),
            reliable_command_retries: outbound.reliable_command_retries.load(Ordering::Relaxed),
            reliable_command_retries_in_flight: outbound
                .reliable_command_retries_in_flight
                .load(Ordering::Relaxed),
            max_reliable_command_retries_in_flight: outbound
                .reliable_command_retries_max_in_flight
                .load(Ordering::Relaxed),
            slow_client_write_timeouts: outbound.slow_client_write_timeouts.load(Ordering::Relaxed),
            slow_client_pressure_sheds: outbound.slow_client_pressure_sheds.load(Ordering::Relaxed),
        }
    }
}

pub(in crate::play) fn dispatch_visibility_commands(dispatches: Vec<VisibilityDispatch>) {
    let mut batches = Vec::<(SessionRecipient, Vec<OutboundCommand>)>::new();
    let mut ordered_batches = HashMap::<(SessionId, usize, u64), usize>::new();
    for dispatch in dispatches {
        let Some(reservation) = dispatch.recipient.ordered.as_ref() else {
            batches.push((dispatch.recipient, vec![dispatch.command]));
            continue;
        };
        let key = (
            dispatch.recipient.id,
            Arc::as_ptr(&reservation.token.state) as usize,
            reservation.sequence,
        );
        if let Some(&batch_index) = ordered_batches.get(&key) {
            batches[batch_index].1.push(dispatch.command);
        } else {
            ordered_batches.insert(key, batches.len());
            batches.push((dispatch.recipient, vec![dispatch.command]));
        }
    }
    for (recipient, commands) in batches {
        dispatch_visibility_batch(&recipient, commands);
    }
}

pub(super) fn dispatch_visibility_command(recipient: &SessionRecipient, command: OutboundCommand) {
    dispatch_visibility_batch(recipient, vec![command]);
}

fn dispatch_visibility_batch(recipient: &SessionRecipient, commands: Vec<OutboundCommand>) {
    if let Some(reservation) = &recipient.ordered {
        dispatch_ordered_commands(recipient, reservation, commands);
        return;
    }
    for command in commands {
        dispatch_unordered_command(recipient, command);
    }
}

fn dispatch_ordered_commands(
    recipient: &SessionRecipient,
    reservation: &OrderedDispatchReservation,
    commands: Vec<OutboundCommand>,
) {
    reservation.token.committed.store(true, Ordering::Release);
    let mut state = lock_ordered_dispatch_state(&reservation.token.state);
    if state.closing || reservation.sequence < state.next_dispatched {
        return;
    }
    if state
        .pending
        .insert(
            reservation.sequence,
            OrderedQueueEntry::Commands {
                target: OrderedDispatchTarget {
                    id: recipient.id,
                    tx: recipient.tx.clone(),
                    pressure: Arc::clone(&recipient.pressure),
                },
                commands,
            },
        )
        .is_some()
    {
        warn!(
            recipient = recipient.id,
            sequence = reservation.sequence,
            "dropping duplicate ordered outbound reservation"
        );
        return;
    }

    if shed_ordered_overflow(&mut state, &reservation.token.target) {
        return;
    }
    drain_ordered_queue(&mut state);
}

fn cancel_ordered_reservation(token: &OrderedDispatchToken) {
    let mut state = lock_ordered_dispatch_state(&token.state);
    if state.closing || token.sequence < state.next_dispatched {
        return;
    }
    state
        .pending
        .entry(token.sequence)
        .or_insert_with(|| OrderedQueueEntry::Canceled {
            action: token.cancel_action,
            target: token.target.clone(),
        });
    if shed_ordered_overflow(&mut state, &token.target) {
        return;
    }
    drain_ordered_queue(&mut state);
}

fn drain_ordered_queue(state: &mut OrderedDispatchQueue) {
    loop {
        let next = state.next_dispatched;
        let Some(entry) = state.pending.remove(&next) else {
            break;
        };
        state.next_dispatched = state
            .next_dispatched
            .checked_add(1)
            .expect("session outbound sequence exhausted");
        match entry {
            OrderedQueueEntry::Canceled {
                action: OrderedCancelAction::Skip,
                ..
            } => {}
            OrderedQueueEntry::Canceled {
                action: OrderedCancelAction::Disconnect,
                target,
            } => {
                let dropped = state
                    .pending
                    .values()
                    .map(ordered_queue_entry_weight)
                    .sum::<usize>();
                state.pending.clear();
                state.closing = true;
                target.pressure.record_reliable_command_drops(dropped);
                dispatch_unordered_command(
                    &target.recipient(),
                    OutboundCommand::DisconnectPlayer {
                        reason: "Required entity spawn publication was canceled".to_string(),
                    },
                );
                return;
            }
            OrderedQueueEntry::Commands { target, commands } => {
                let recipient = target.recipient();
                for command in commands {
                    dispatch_unordered_command(&recipient, command);
                }
            }
        }
    }
}

fn shed_ordered_overflow(state: &mut OrderedDispatchQueue, target: &OrderedDispatchTarget) -> bool {
    let capacity = target
        .tx
        .max_capacity()
        .max(MIN_RELIABLE_RETRY_QUEUE_CAPACITY);
    let pending_commands = state
        .pending
        .values()
        .map(ordered_queue_entry_weight)
        .sum::<usize>();
    if pending_commands <= capacity {
        return false;
    }

    state.pending.clear();
    state.closing = true;
    target
        .pressure
        .record_reliable_command_drops(pending_commands);
    target.pressure.record_slow_client_pressure_shed();
    dispatch_unordered_command(
        &target.recipient(),
        OutboundCommand::DisconnectPlayer {
            reason: RELIABLE_RETRY_OVERFLOW_REASON.to_string(),
        },
    );
    true
}

fn ordered_queue_entry_weight(entry: &OrderedQueueEntry) -> usize {
    match entry {
        OrderedQueueEntry::Canceled { .. } => 1,
        OrderedQueueEntry::Commands { commands, .. } => commands.len(),
    }
}

fn dispatch_unordered_command(recipient: &SessionRecipient, command: OutboundCommand) {
    match command.lane() {
        OutboundLane::Reliable => dispatch_reliable_command(recipient, command),
        OutboundLane::BestEffort => dispatch_best_effort_command(recipient, command),
    }
}

fn dispatch_best_effort_command(recipient: &SessionRecipient, command: OutboundCommand) {
    match recipient.tx.try_send(command) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            recipient.pressure.record_best_effort_animation_drop();
            debug!(
                recipient = recipient.id,
                "dropping best-effort outbound command"
            );
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            recipient.pressure.record_best_effort_animation_drop();
            debug!(
                recipient = recipient.id,
                "dropping best-effort outbound command for closed session"
            );
        }
    }
}

fn dispatch_reliable_command(recipient: &SessionRecipient, command: OutboundCommand) {
    let mut worker_id = None;
    let mut enqueue_result = ReliableEnqueueResult::Queued;
    {
        let mut queues = recipient.pressure.lock_reliable_retry_queues();
        match queues.entry(recipient.id) {
            Entry::Occupied(mut entry) => {
                enqueue_result = entry.get_mut().enqueue(command);
            }
            Entry::Vacant(entry) => match recipient.tx.try_send(command) {
                Ok(()) => return,
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    drop(queues);
                    warn!(
                        recipient = recipient.id,
                        "dropping reliable outbound command for closed session"
                    );
                    return;
                }
                Err(mpsc::error::TrySendError::Full(command)) => {
                    let capacity = recipient
                        .tx
                        .max_capacity()
                        .max(MIN_RELIABLE_RETRY_QUEUE_CAPACITY);
                    let id = recipient.pressure.record_reliable_retry_started();
                    entry.insert(ReliableRetryQueue::new(command, capacity, id));
                    worker_id = Some(id);
                }
            },
        }
    }

    match enqueue_result {
        ReliableEnqueueResult::Queued => {}
        ReliableEnqueueResult::Dropped => {}
        ReliableEnqueueResult::Shed { dropped } => {
            recipient.pressure.record_reliable_command_drops(dropped);
            recipient.pressure.record_slow_client_pressure_shed();
            warn!(
                recipient = recipient.id,
                dropped, "reliable outbound backlog exceeded its bound; closing session"
            );
        }
    }

    let Some(worker_id) = worker_id else {
        return;
    };

    let tx = recipient.tx.clone();
    let recipient_id = recipient.id;
    let pressure = Arc::clone(&recipient.pressure);
    let guard = ReliableRetryWorkerGuard {
        recipient_id,
        worker_id,
        pressure: Arc::clone(&pressure),
    };
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(async move {
            let _guard = guard;
            loop {
                let Some(command) = pop_reliable_retry_command(&pressure, recipient_id) else {
                    return;
                };
                if tx.send(command).await.is_err() {
                    debug!(
                        recipient = recipient_id,
                        "reliable outbound retry target closed"
                    );
                    return;
                }
            }
        });
        return;
    }

    if let Err(error) = std::thread::Builder::new()
        .name(format!("solaris-outbound-{recipient_id}"))
        .spawn(move || {
            let _guard = guard;
            loop {
                let Some(command) = pop_reliable_retry_command(&pressure, recipient_id) else {
                    return;
                };
                if tx.blocking_send(command).is_err() {
                    debug!(
                        recipient = recipient_id,
                        "blocking reliable outbound retry target closed"
                    );
                    return;
                }
            }
        })
    {
        warn!(recipient = recipient_id, %error, "failed to start reliable outbound retry worker");
    }
}

fn pop_reliable_retry_command(
    pressure: &OutboundPressureMetrics,
    recipient_id: SessionId,
) -> Option<OutboundCommand> {
    let command = {
        let mut queues = pressure.lock_reliable_retry_queues();
        let command = queues
            .get_mut(&recipient_id)
            .and_then(|queue| queue.pending.pop_front());
        if command.is_none() {
            queues.remove(&recipient_id);
        }
        command
    };
    #[cfg(test)]
    if command.is_some() {
        pressure.reliable_retry_dequeued.notify_waiters();
    }
    command
}
