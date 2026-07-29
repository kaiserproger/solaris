use super::combat::{ActiveShield, PlayerHurtResistance};
use super::persistence::PlayerPersistedState;
#[cfg(test)]
use super::simulation::PlayerStateEvent;
use super::simulation::{EntitySimulationWorldContext, SimulationAuthority};
use super::*;
#[cfg(test)]
use mc_entity::RegionKey;
use mc_entity::{
    EntityDamageRequest, EntityKinematics, EntityMotionState, EntitySnapshot, RegionalOwnerHandle,
    RegionalOwnerRuntime, RegionalOwnerStatus, VersionedEntitySnapshots,
};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

mod campfire_authority;
mod chunk_view_authority;
#[cfg(test)]
#[path = "session/combat_load_tests.rs"]
mod combat_load_tests;
mod container_state;
mod container_views;
#[cfg(test)]
#[path = "session/death_policy_tests.rs"]
mod death_policy_tests;
mod entity_combat;
mod entity_goal_defaults;
mod entity_lifecycle;
mod entity_owner;
mod entity_physics_class;
mod entity_simulation;
mod entity_tracking;
#[cfg(test)]
mod entity_tracking_tests;
mod explosion_authority;
mod herd_spawn_authority;
mod hostile_authority;
#[cfg(test)]
#[path = "session/hostile_authority_tests.rs"]
mod hostile_authority_tests;
mod inhabited_time;
#[cfg(test)]
mod inhabited_time_tests;
mod interaction_geometry;
#[cfg(test)]
#[path = "session/interaction_geometry_tests.rs"]
mod interaction_geometry_tests;
mod movement_publication;
mod outbound;
#[cfg(test)]
mod outbound_backpressure_tests;
mod outbound_publication;
mod passive_mobs;
mod pathing;
mod pickups;
mod player_combat;
mod player_item_action_authority;
mod player_pose_adapter;
mod player_pose_authority;
mod player_state;
mod player_state_adapter;
#[cfg(test)]
mod position_sync_tests;
mod prepared_chunks;
mod projectiles;
#[cfg(test)]
#[path = "session/projectiles_tests.rs"]
mod projectiles_tests;
mod script_client_screen_endpoint;
#[cfg(test)]
mod script_client_screen_endpoint_tests;
mod script_loader_interaction_endpoint;
#[cfg(test)]
mod script_loader_interaction_endpoint_tests;
pub(super) use script_loader_interaction_endpoint::route_client_loader_interaction;
mod script_colony_endpoint;
mod script_commit_events;
mod script_entity_interaction;
#[cfg(test)]
mod script_entity_interaction_tests;
mod script_inventory_transaction_endpoint;
#[cfg(test)]
mod script_inventory_transaction_endpoint_tests;
mod script_loader_item_endpoint;
#[cfg(test)]
mod script_loader_item_endpoint_tests;
mod script_menu_endpoint;
#[cfg(test)]
mod script_menu_endpoint_tests;
mod script_player_inventory_endpoint;
#[cfg(test)]
mod script_player_inventory_endpoint_tests;
mod script_player_query_endpoint;
#[cfg(test)]
mod script_player_query_endpoint_tests;
mod script_teleport_endpoint;
#[cfg(test)]
mod script_teleport_endpoint_tests;
mod session_lifecycle;
mod settlement_authority;
pub(super) use settlement_authority::toolsmith_merchant_state;
mod simulation_input_publication;
#[cfg(test)]
mod simulation_input_publication_tests;
mod sleep;
mod survival_action_authority;
mod transactions;
#[cfg(test)]
mod villager_brain_tests;
mod villager_merchant_authority;
#[cfg(test)]
mod villager_merchant_tests;
mod villager_population;
mod visibility;
mod zombie_villager;

pub(super) fn prewarm_canonical_pathing_state_facts() -> usize {
    pathing::prewarm_canonical_pathing_state_facts()
}

#[cfg(test)]
use super::simulation::{PlayerSurvivalCommitOutcome, PlayerSurvivalPlan};
#[cfg(test)]
use campfire_authority::CampfireRecoveryProbe;
use container_state::ContainerRegistryShards;
pub(super) use container_state::{ContainerCommitContext, ContainerStateCommitError};
#[cfg(test)]
use container_state::{
    ContainerCommitProbe, ServerContainerDispatchProbe, ServerFurnaceCommitProbe,
};
pub(super) use entity_combat::ServerEntityPlayerAttack;
use entity_goal_defaults::passive_ground_wander_speed;
use entity_lifecycle::remove_server_entity_locked;
#[cfg(test)]
use entity_lifecycle::{
    ENTITY_EVENT_DEATH_COMPLETE, nearby_entity_candidate_ids_locked, nearby_entity_snapshots_locked,
};
use entity_owner::*;
use entity_tracking::EntityMovementTrackers;
#[allow(unused_imports)]
pub(super) use explosion_authority::{
    EXPLOSIONS_PER_TICK, ExpiredPrimedTnt, ExplosionEntityTarget, ExplosionPlayerTarget,
    ServerEntityExplosionImpact,
};
pub(super) use herd_spawn_authority::HerdSpawnOutcome;
#[cfg(test)]
use herd_spawn_authority::{
    ChunkHerdClaimProbe, VANILLA_CREATURE_MOB_CAP, VANILLA_HOSTILE_MOB_CAP,
    install_committed_herd_spawns_locked,
};
use herd_spawn_authority::{ClaimedPendingHostiles, claim_loaded_pending_hostiles_locked};
use hostile_authority::update_hostile_targets_from_projections;
#[cfg(test)]
use hostile_authority::{
    HostileCommitProbe, HostileScanProbe, changed_hostile_goal, hostile_wander_goal,
};
pub(super) use interaction_geometry::within_block_reach;
#[cfg(test)]
pub(super) use interaction_geometry::{
    entity_aabb, entity_is_near_player_chunk, within_entity_reach,
};
use movement_publication::{
    MovementRecipientIndex, PublishedCombatTarget, PublishedEntityVisibility,
    SessionPublicationEpoch, build_movement_recipient_index,
};
pub(crate) use outbound::{EntityDispatchCounters, SessionPressureSnapshot};
#[cfg(test)]
use outbound::{
    MIN_RELIABLE_RETRY_QUEUE_CAPACITY, RELIABLE_RETRY_OVERFLOW_REASON, ReliableRetryQueue,
    ReliableRetryWorkerGuard,
};
use outbound::{
    OrderedDispatchState, OutboundPressureMetrics, SessionPressureObservation,
    dispatch_visibility_command,
};
pub(super) use outbound::{
    OutboundCommand, OutboundLightUpdate, PlayerDamagePublication, PlayerEntitySnapshot,
    ServerEntityMove, ServerEntitySnapshot, ShieldCooldownPublication, VisibilityDispatch,
    dispatch_visibility_commands,
};
#[cfg(test)]
pub(super) use outbound::{PlayerInventorySlotDelta, SessionRecipient};
pub(in crate::play) use passive_mobs::SHEEP_GRAZING_ANIMATION_TICKS;
pub(super) use passive_mobs::SheepGrazingCandidate;
#[cfg(test)]
pub(super) use passive_mobs::sheep_grazing_starts_on_tick;
#[cfg(test)]
use passive_mobs::{
    BreedingAnimal, GrazingSheep, SHEEP_GRAZING_ACTION_TICK, advance_sheep_grazing, plan_breeding,
};
#[cfg(test)]
use passive_mobs::{sheep_breeding_color, sheep_recipe_mix};
use pathing::*;
#[cfg(test)]
pub(in crate::play) use pickups::ITEM_PICKUP_DELAY_TICKS;
#[cfg(test)]
use pickups::item_pickup_ready_locked;
pub(in crate::play) use pickups::{
    CreditedArrowPickup, CreditedExperiencePickup, CreditedItemPickup, ENTITY_PICKUP_RADIUS,
};
use pickups::{spawn_item_drop_locked, spawn_xp_orb_locked};
pub(super) use player_combat::PlayerEntityAttack;
use player_pose_authority::filter_current_expected_entity_snapshots;
use projectiles::resolve_arrow_entity_hits_locked;
#[cfg(test)]
use projectiles::{
    arrow_entity_candidate_snapshots_locked, segment_aabb_intersection_t, spawn_arrow_locked,
};
pub(in crate::play) use script_loader_item_endpoint::{
    LoaderItemGrantCommand, apply_loader_item_grant,
};
pub(in crate::play) use script_menu_endpoint::{
    ScriptMenuCloseRequest, ScriptMenuOpenRequest, publish_script_menu_click,
};
pub(in crate::play) use script_player_inventory_endpoint::{
    ScriptPlayerInventoryCommand, apply_script_player_inventory_transaction,
};
pub(in crate::play) use script_teleport_endpoint::{
    ScriptPlayerTeleportCommand, ScriptPlayerTeleportCompletion,
};
use simulation_input_publication::SimulationInputPublication;
pub(super) use sleep::SleepOutcome;
#[cfg(test)]
use sleep::{DEEP_SLEEP_TICKS, sleepers_needed};
use sleep::{DEFAULT_PLAYERS_SLEEPING_PERCENTAGE, SleepingState};
pub(super) use transactions::*;
use visibility::EntityPositionUpdate;
pub(super) use visibility::server_entity_snapshot_from;
use visibility::{
    entity_event_dispatches_locked, entity_velocity_changed,
    initialize_entity_wire_state_from_snapshot_locked, initialize_entity_wire_state_locked,
    install_committed_entity_publications_locked, ordered_session_recipient,
    packed_rotation_changed, plan_entity_position_update, publish_server_entity_snapshot_locked,
    refresh_entity_target_visibility_locked, session_recipients,
    spawn_entity_visibility_from_snapshot_locked, spawn_entity_visibility_locked,
    spawned_xp_observer_ids, visibility_dispatches, visible_entity_observers_locked,
    visible_observers_locked,
};

pub(super) type SessionId = u64;

pub(in crate::play) const ENTITY_DEATH_TICKS: u64 = 20;
const ENTITY_EVENT_DEATH: i8 = 3;
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SessionAdmissionError {
    ServerFull { active: usize, max: usize },
    DuplicateProfile { existing_session: SessionId },
}

#[derive(Debug)]
pub(super) enum PlayerInventoryCommitError {
    MissingPlayer,
}

#[derive(Debug)]
pub(super) enum EntityAttackOutcome {
    Damaged {
        damage: mc_entity::EntityDamage,
        dispatches: Vec<VisibilityDispatch>,
        attacker_costs: Option<CommittedPlayerAttackCosts>,
    },
    Killed {
        damage: mc_entity::EntityDamage,
        entity: ServerEntitySnapshot,
        dispatches: Vec<VisibilityDispatch>,
        attacker_costs: Option<CommittedPlayerAttackCosts>,
    },
    PlayerDamaged {
        target_session: SessionId,
        dispatches: Vec<VisibilityDispatch>,
        damage_applied: bool,
        attacker_costs: Option<CommittedPlayerAttackCosts>,
    },
}

#[derive(Debug)]
pub(super) struct CommittedPlayerAttackCosts {
    pub(super) survival: SurvivalState,
    pub(super) inventory: PlayerInventory,
}

#[derive(Debug)]
pub(super) enum PlayerAttackResult {
    ValidationRejected,
    AcceptedNoDamage,
    Damaged(Box<EntityAttackOutcome>),
}

#[derive(Debug, Clone, Default)]
pub(super) struct EntityKillRewards {
    pub(super) items: Vec<(i32, EntityItemStack)>,
    pub(super) experience: Option<(i32, i32)>,
}

#[cfg(test)]
impl EntityAttackOutcome {
    pub(super) fn damage(&self) -> &mc_entity::EntityDamage {
        match self {
            Self::Damaged { damage, .. } | Self::Killed { damage, .. } => damage,
            Self::PlayerDamaged { .. } => panic!("player damage is committed by player authority"),
        }
    }

    pub(super) fn dispatches(&self) -> &[VisibilityDispatch] {
        match self {
            Self::Damaged { dispatches, .. }
            | Self::Killed { dispatches, .. }
            | Self::PlayerDamaged { dispatches, .. } => dispatches,
        }
    }
}

impl EntityAttackOutcome {
    #[cfg(test)]
    pub(super) fn into_dispatches(self) -> Vec<VisibilityDispatch> {
        match self {
            Self::Damaged { dispatches, .. }
            | Self::Killed { dispatches, .. }
            | Self::PlayerDamaged { dispatches, .. } => dispatches,
        }
    }

    fn dispatches_mut(&mut self) -> &mut Vec<VisibilityDispatch> {
        match self {
            Self::Damaged { dispatches, .. }
            | Self::Killed { dispatches, .. }
            | Self::PlayerDamaged { dispatches, .. } => dispatches,
        }
    }
}

pub(super) struct SessionRegistration<'a> {
    pub(super) profile: &'a LoggedInProfile,
    pub(super) properties: &'a [mc_protocol::packets::login::GameProfileProperty],
    pub(super) center: (i32, i32),
    pub(super) view_distance: i32,
    pub(super) desired: HashSet<(i32, i32)>,
    pub(super) tx: mpsc::Sender<OutboundCommand>,
    pub(super) pose: PlayerPose,
    pub(super) max_sessions: usize,
    pub(super) script_operator: bool,
    pub(super) dimension: &'a str,
    pub(super) loader_session: Option<crate::LoaderSession>,
}

#[derive(Debug)]
struct PlaySession {
    name: String,
    uuid: uuid::Uuid,
    properties: Vec<mc_protocol::packets::login::GameProfileProperty>,
    entity_id: i32,
    pose: PlayerPose,
    center: (i32, i32),
    view_distance: i32,
    desired: HashSet<(i32, i32)>,
    loaded: HashSet<(i32, i32)>,
    visible_players: HashSet<SessionId>,
    visible_entities: PublishedEntityVisibility,
    combat_target: PublishedCombatTarget,
    tx: mpsc::Sender<OutboundCommand>,
    pressure: Arc<OutboundPressureMetrics>,
    ordered_dispatch: Arc<OrderedDispatchState>,
    script_inventory_transaction_gate:
        Arc<script_inventory_transaction_endpoint::ScriptInventoryTransactionGate>,
    script_operator: bool,
    dimension: String,
    loader_session: Option<crate::LoaderSession>,
    last_broadcast_world_time: Option<u64>,
}

#[derive(Debug, Clone)]
struct DisconnectedPlayerPersistence {
    generation: u64,
    state: Arc<Mutex<PlayerPersistedState>>,
}

#[derive(Debug, Default)]
struct SessionRegistryInner {
    next_id: SessionId,
    sessions: HashMap<SessionId, PlaySession>,
    loaded_chunk_refcounts: HashMap<(i32, i32), usize>,
    tickets: HashMap<(i32, i32), HashSet<SessionId>>,
    hostile_entities: HashSet<EntityId>,
    natural_hostile_mobs: HashSet<EntityId>,
    natural_ground_mobs: HashSet<EntityId>,
    natural_aquatic_mobs: HashSet<EntityId>,
    sheep_entities: HashSet<EntityId>,
    published_entity_snapshots: HashMap<EntityId, ServerEntitySnapshot>,
    entity_type_aabbs: HashMap<i32, mc_physics::Aabb>,
    simulation_inputs: Arc<SimulationInputPublication>,
    entity_movement_trackers: Arc<EntityMovementTrackers>,
    arrow_tick_scratch: projectiles::ArrowTickScratch,
    spawned_entity_chunks: HashSet<(i32, i32)>,
    settlement_spawn_claims: BTreeSet<String>,
    settlement_vacant_homes: BTreeMap<String, Vec3>,
    settlement_claimed_homes: BTreeSet<String>,
    villager_birth_deadlines: BTreeMap<u64, VecDeque<[EntityId; 2]>>,
    villager_birth_deadline_by_parent: HashMap<EntityId, u64>,
    pending_hostile_spawns: BTreeMap<(i32, i32), Vec<HerdSpawn>>,
    item_pickup_ready: BTreeMap<u64, Vec<EntityId>>,
    item_despawn_deadlines: BTreeMap<u64, VecDeque<EntityId>>,
    item_despawn_deadline_by_id: HashMap<EntityId, u64>,
    dying_entity_deadlines: BTreeMap<u64, VecDeque<EntityId>>,
    dying_entity_deadline_by_id: HashMap<EntityId, u64>,
    zombie_villager_conversion_deadlines: BTreeMap<u64, VecDeque<EntityId>>,
    zombie_villager_conversion_deadline_by_id: HashMap<EntityId, u64>,
    primed_tnt_deadlines: BTreeMap<u64, BTreeSet<EntityId>>,
    primed_tnt_deadline_by_id: HashMap<EntityId, u64>,
    last_primed_tnt_claim_tick: Option<u64>,
    player_persistence: HashMap<SessionId, Arc<Mutex<PlayerPersistedState>>>,
    player_hurt_resistance: HashMap<SessionId, PlayerHurtResistance>,
    active_shields: HashMap<SessionId, ActiveShield>,
    shield_disabled_until: HashMap<SessionId, u64>,
    disconnected_player_persistence: HashMap<uuid::Uuid, DisconnectedPlayerPersistence>,
    next_disconnected_player_generation: u64,
    sleeping_sessions: HashMap<SessionId, SleepingState>,
    spectator_sessions: HashSet<SessionId>,
    dead_sessions: HashSet<SessionId>,
    client_unloaded_sessions: HashSet<SessionId>,
    keep_inventory: bool,
    entity_dispatches: EntityDispatchCounters,
    arrow_kill_rewards: ArrowKillRewards,
    player_combat: PlayerCombatResources,
    script_commit_events: Option<tokio::sync::mpsc::UnboundedSender<ScriptEvent>>,
}

impl SessionRegistryInner {
    fn publish_combat_target(&mut self, id: SessionId) {
        let alive = !self.dead_sessions.contains(&id);
        let targetable = alive
            && !self.spectator_sessions.contains(&id)
            && !self.client_unloaded_sessions.contains(&id);
        if let Some(session) = self.sessions.get(&id) {
            session
                .combat_target
                .publish(session.pose, alive, targetable);
        }
    }
}

#[derive(Debug, Default)]
struct PreparedChunkCache {
    prepared: HashMap<(i32, i32), Arc<PreparedChunkFrame>>,
    prewarmed_prepared: VecDeque<(i32, i32)>,
    prepared_in_flight: HashMap<(i32, i32), PreparedChunkClaim>,
    prepared_revisions: HashMap<(i32, i32), u64>,
    next_prepared_claim: u64,
    ticket_counts: HashMap<(i32, i32), usize>,
    pending_subscriber_counts: HashMap<(i32, i32), usize>,
    prewarm_frontier_counts: HashMap<(i32, i32), usize>,
}

#[derive(Debug, Clone, Default)]
struct ArrowKillRewards {
    item_entity_type_id: Option<i32>,
    xp_orb_entity_type_id: Option<i32>,
    items: Option<Arc<ItemRegistry>>,
    item_facts: Option<Arc<ItemFactsTable>>,
    loot: Option<Arc<mc_data::loot::LootTables>>,
}

#[derive(Debug, Clone)]
struct PlayerCombatResources {
    item_entity_type_id: Option<i32>,
    xp_orb_entity_type_id: Option<i32>,
    items: Arc<ItemRegistry>,
    item_facts: Arc<ItemFactsTable>,
}

impl Default for PlayerCombatResources {
    fn default() -> Self {
        Self {
            item_entity_type_id: None,
            xp_orb_entity_type_id: None,
            items: Arc::new(ItemRegistry::default()),
            item_facts: Arc::new(ItemFactsTable::default()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct PreparedChunkClaim {
    id: u64,
    pub(super) revision: u64,
    owner_session: Option<SessionId>,
}

#[derive(Debug, Clone)]
pub(super) enum PreparedChunkClaimResult {
    Cached,
    Claimed(PreparedChunkClaim),
    InFlight,
}

#[derive(Debug, Clone)]
pub(super) enum SessionPreparedChunkClaimResult {
    Cached(Arc<PreparedChunkFrame>, u64),
    Claimed(PreparedChunkClaim),
    InFlight,
    WaitingForEarlierSession,
}

#[derive(Debug)]
pub(crate) struct SessionRegistry {
    inner: Mutex<SessionRegistryInner>,
    simulation_inputs: Arc<SimulationInputPublication>,
    movement_recipients: arc_swap::ArcSwap<MovementRecipientIndex>,
    active_simulation_entities: arc_swap::ArcSwap<HashSet<EntityId>>,
    active_hostile_entities: arc_swap::ArcSwap<HashSet<EntityId>>,
    entity_update_budget_per_lane: AtomicUsize,
    entity_update_budget_total: AtomicUsize,
    entity_update_selected: AtomicUsize,
    entity_update_active_population: AtomicUsize,
    entity_movement_publication_budget: AtomicUsize,
    overridden_villager_entities: arc_swap::ArcSwap<HashSet<EntityId>>,
    villager_brain_profile: arc_swap::ArcSwap<mc_entity::villager_26_1_2::VillagerBrainProfile>,
    mob_behavior_table: arc_swap::ArcSwap<mc_data::mob_behavior_26_1_2::MobBehaviorTable>,
    hostile_arrow_entity_type_id: AtomicI32,
    prepared_cache: Mutex<PreparedChunkCache>,
    entities: SessionEntityOwners,
    world_chunk_journal: Mutex<Option<super::world_journal::WorldChunkJournal>>,
    world_chunk_journal_failure: tokio::sync::watch::Sender<bool>,
    containers: ContainerRegistryShards,
    campfire_cooking: Arc<Mutex<HashMap<mc_world::BlockPos, CampfireCookingState>>>,
    pressure_observation: Arc<SessionPressureObservation>,
    outbound_pressure: Arc<OutboundPressureMetrics>,
    world_time: AtomicU64,
    scheduled_block_tick_in_flight: AtomicBool,
    players_sleeping_percentage: AtomicU32,
    entity_lifecycle_tick: AtomicU64,
    simulation_tick_sender: tokio::sync::watch::Sender<u64>,
    player_attack_sender: tokio::sync::broadcast::Sender<PlayerAttackObservation>,
    player_attack_sequence: AtomicU64,
    active_session_sender: tokio::sync::watch::Sender<usize>,
    live_session_count: AtomicUsize,
    live_session_generation: AtomicU64,
    session_empty_generation: AtomicU64,
    session_became_empty: tokio::sync::Notify,
    player_save_generation: AtomicU64,
    player_save_requested: tokio::sync::Notify,
    prepared_change_generation: AtomicU64,
    prepared_changed: tokio::sync::Notify,
    #[cfg(test)]
    entity_owner_reconfiguration_calls: AtomicU64,
    #[cfg(test)]
    prepared_chunk_shed_calls: AtomicU64,
    #[cfg(test)]
    prepared_claim_calls: AtomicU64,
    #[cfg(test)]
    move_fanout_probe: Mutex<Option<MoveFanoutProbe>>,
    #[cfg(test)]
    movement_dispatch_probe: Mutex<Option<MoveFanoutProbe>>,
    #[cfg(test)]
    movement_visibility_load_probe: Mutex<Option<MoveFanoutProbe>>,
    #[cfg(test)]
    entity_apply_release_probe: Mutex<Option<EntityApplyReleaseProbe>>,
    #[cfg(test)]
    physics_owner_apply_probe: Mutex<Option<EntityApplyReleaseProbe>>,
    #[cfg(test)]
    physics_routing_probe: Mutex<Option<EntityApplyReleaseProbe>>,
    #[cfg(test)]
    arrow_transaction_probe: Mutex<Option<ArrowTransactionProbe>>,
    #[cfg(test)]
    breeding_plan_probe: Mutex<Option<BreedingPlanProbe>>,
    #[cfg(test)]
    breeding_commit_probe: Mutex<Option<BreedingCommitProbe>>,
    #[cfg(test)]
    sheep_grazing_plan_probe: Mutex<Option<SheepGrazingPlanProbe>>,
    #[cfg(test)]
    sheep_grazing_commit_probe: Mutex<Option<SheepGrazingCommitProbe>>,
    #[cfg(test)]
    sheep_grazing_owner_read_probe: Mutex<Option<EntityApplyReleaseProbe>>,
    #[cfg(test)]
    entity_save_owner_probe: Mutex<Option<EntityApplyReleaseProbe>>,
    #[cfg(test)]
    script_transaction_capture_probe: Mutex<Option<EntityApplyReleaseProbe>>,
    #[cfg(test)]
    hostile_scan_probe: Mutex<Option<HostileScanProbe>>,
    #[cfg(test)]
    hostile_reconcile_probe: Mutex<Option<HostileScanProbe>>,
    #[cfg(test)]
    hostile_commit_probe: Mutex<Option<HostileCommitProbe>>,
    #[cfg(test)]
    hostile_publication_probe: Mutex<Option<HostileCommitProbe>>,
    #[cfg(test)]
    hostile_target_snapshot_probe: Mutex<Option<HostileCommitProbe>>,
    #[cfg(test)]
    player_push_commit_probe: Mutex<Option<PlayerPushCommitProbe>>,
    #[cfg(test)]
    pickup_snapshot_probe: Mutex<Option<PickupSnapshotProbe>>,
    #[cfg(test)]
    item_pickup_plan_probe: Mutex<Option<ItemPickupPlanProbe>>,
    #[cfg(test)]
    item_pickup_owner_probe: Mutex<Option<EntityApplyReleaseProbe>>,
    #[cfg(test)]
    item_pickup_claimed_probe: Mutex<Option<EntityApplyReleaseProbe>>,
    #[cfg(test)]
    server_container_dispatch_probe: Mutex<Option<ServerContainerDispatchProbe>>,
    #[cfg(test)]
    server_relight_compute_probe: Mutex<Option<ServerRelightComputeProbe>>,
    #[cfg(test)]
    entity_goal_compute_probe: Mutex<Option<EntityGoalComputeProbe>>,
    #[cfg(test)]
    server_furnace_commit_probe: Mutex<Option<ServerFurnaceCommitProbe>>,
    #[cfg(test)]
    container_commit_probe: Mutex<Option<ContainerCommitProbe>>,
    #[cfg(test)]
    chunk_herd_claim_probe: Mutex<Option<ChunkHerdClaimProbe>>,
    #[cfg(test)]
    campfire_d1_probe: Mutex<Option<CampfireRecoveryProbe>>,
    #[cfg(test)]
    campfire_entity_probe: Mutex<Option<CampfireRecoveryProbe>>,
    #[cfg(test)]
    physics_boundary_observer_scans: AtomicU64,
    #[cfg(test)]
    active_entity_selection_visits: AtomicU64,
    #[cfg(test)]
    player_push_entity_visits: AtomicU64,
    #[cfg(test)]
    breeding_state_updates: AtomicU64,
    #[cfg(test)]
    breeding_commits: AtomicU64,
    #[cfg(test)]
    breeding_entity_scan_visits: AtomicU64,
    #[cfg(test)]
    sheep_grazing_entity_visits: AtomicU64,
    #[cfg(test)]
    hostile_attack_candidates: AtomicU64,
    #[cfg(test)]
    hostile_entity_scan_visits: AtomicU64,
}

#[cfg(test)]
#[derive(Debug)]
struct MoveFanoutProbe {
    reached: std::sync::mpsc::Sender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
#[derive(Debug)]
struct EntityApplyReleaseProbe {
    reached: std::sync::mpsc::Sender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
#[derive(Debug)]
struct ArrowTransactionProbe {
    reached: std::sync::mpsc::Sender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
#[derive(Debug)]
struct BreedingPlanProbe {
    reached: std::sync::mpsc::Sender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
#[derive(Debug)]
struct BreedingCommitProbe {
    reached: std::sync::mpsc::Sender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
#[derive(Debug)]
struct SheepGrazingPlanProbe {
    reached: std::sync::mpsc::Sender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
#[derive(Debug)]
struct SheepGrazingCommitProbe {
    reached: std::sync::mpsc::Sender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
#[derive(Debug)]
struct PlayerPushCommitProbe {
    reached: std::sync::mpsc::Sender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
#[derive(Debug)]
struct PickupSnapshotProbe {
    reached: std::sync::mpsc::Sender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
#[derive(Debug)]
struct ItemPickupPlanProbe {
    reached: std::sync::mpsc::Sender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
#[derive(Debug)]
struct ServerRelightComputeProbe {
    reached: std::sync::mpsc::Sender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
#[derive(Debug)]
struct EntityGoalComputeProbe {
    reached: std::sync::mpsc::Sender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}

struct SessionInnerGuard<'a> {
    guard: crate::lock_metrics::TimedGuard<MutexGuard<'a, SessionRegistryInner>>,
    observation: &'a SessionPressureObservation,
    dirty: bool,
}

impl Deref for SessionInnerGuard<'_> {
    type Target = SessionRegistryInner;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl DerefMut for SessionInnerGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.dirty = true;
        &mut self.guard
    }
}

impl Drop for SessionInnerGuard<'_> {
    fn drop(&mut self) {
        if self.dirty {
            self.observation.publish_sessions(&self.guard);
        }
    }
}

struct EntityStoreGuard<'a> {
    access: EntityOwnerAccess,
    marker: std::marker::PhantomData<&'a SessionRegistry>,
}

impl Deref for EntityStoreGuard<'_> {
    type Target = EntityOwnerAccess;

    fn deref(&self) -> &Self::Target {
        &self.access
    }
}

impl DerefMut for EntityStoreGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.access
    }
}

struct SessionEntityGuards<'a> {
    inner: SessionInnerGuard<'a>,
    entities: EntityStoreGuard<'a>,
    entity_lifecycle_tick: u64,
}

impl Deref for SessionEntityGuards<'_> {
    type Target = SessionRegistryInner;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for SessionEntityGuards<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl Default for SessionRegistry {
    fn default() -> Self {
        let lane_count = std::thread::available_parallelism().map_or(1, usize::from);
        Self::with_entity_owner_lanes(lane_count)
    }
}

impl SessionRegistry {
    fn with_entity_owner_lanes(lane_count: usize) -> Self {
        Self::with_entity_owner_lanes_and_journal(lane_count, None)
    }

    fn with_entity_owner_lanes_and_journal(
        lane_count: usize,
        journal: Option<Box<dyn mc_entity::RegionalDecisionJournal>>,
    ) -> Self {
        let (simulation_tick_sender, _) = tokio::sync::watch::channel(0);
        let (player_attack_sender, _) = tokio::sync::broadcast::channel(64);
        let (active_session_sender, _) = tokio::sync::watch::channel(0);
        let pressure_observation = Arc::new(SessionPressureObservation::default());
        let simulation_inputs = Arc::new(SimulationInputPublication::default());
        Self {
            inner: Mutex::new(SessionRegistryInner {
                simulation_inputs: Arc::clone(&simulation_inputs),
                ..SessionRegistryInner::default()
            }),
            simulation_inputs,
            movement_recipients: arc_swap::ArcSwap::from_pointee(MovementRecipientIndex::new()),
            active_simulation_entities: arc_swap::ArcSwap::from_pointee(HashSet::new()),
            active_hostile_entities: arc_swap::ArcSwap::from_pointee(HashSet::new()),
            entity_update_budget_per_lane: AtomicUsize::new(0),
            entity_update_budget_total: AtomicUsize::new(0),
            entity_update_selected: AtomicUsize::new(0),
            entity_update_active_population: AtomicUsize::new(0),
            entity_movement_publication_budget: AtomicUsize::new(
                ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN,
            ),
            overridden_villager_entities: arc_swap::ArcSwap::from_pointee(HashSet::new()),
            villager_brain_profile: arc_swap::ArcSwap::from_pointee(
                mc_entity::villager_26_1_2::VillagerBrainProfile::vanilla_26_1_2(),
            ),
            mob_behavior_table: arc_swap::ArcSwap::from_pointee(
                mc_data::mob_behavior_26_1_2::MobBehaviorTable::vanilla_26_1_2(),
            ),
            hostile_arrow_entity_type_id: AtomicI32::new(-1),
            prepared_cache: Mutex::new(PreparedChunkCache::default()),
            entities: SessionEntityOwners::new(
                Arc::clone(&pressure_observation),
                lane_count,
                journal,
            ),
            world_chunk_journal: Mutex::new(None),
            world_chunk_journal_failure: tokio::sync::watch::channel(false).0,
            containers: ContainerRegistryShards::default(),
            campfire_cooking: Arc::new(Mutex::new(HashMap::new())),
            pressure_observation,
            outbound_pressure: Arc::new(OutboundPressureMetrics::default()),
            world_time: AtomicU64::new(0),
            scheduled_block_tick_in_flight: AtomicBool::new(false),
            players_sleeping_percentage: AtomicU32::new(DEFAULT_PLAYERS_SLEEPING_PERCENTAGE),
            entity_lifecycle_tick: AtomicU64::new(0),
            simulation_tick_sender,
            player_attack_sender,
            player_attack_sequence: AtomicU64::new(0),
            active_session_sender,
            live_session_count: AtomicUsize::new(0),
            live_session_generation: AtomicU64::new(0),
            session_empty_generation: AtomicU64::new(0),
            session_became_empty: tokio::sync::Notify::new(),
            player_save_generation: AtomicU64::new(0),
            player_save_requested: tokio::sync::Notify::new(),
            prepared_change_generation: AtomicU64::new(0),
            prepared_changed: tokio::sync::Notify::new(),
            #[cfg(test)]
            entity_owner_reconfiguration_calls: AtomicU64::new(0),
            #[cfg(test)]
            prepared_chunk_shed_calls: AtomicU64::new(0),
            #[cfg(test)]
            prepared_claim_calls: AtomicU64::new(0),
            #[cfg(test)]
            move_fanout_probe: Mutex::new(None),
            #[cfg(test)]
            movement_dispatch_probe: Mutex::new(None),
            #[cfg(test)]
            movement_visibility_load_probe: Mutex::new(None),
            #[cfg(test)]
            entity_apply_release_probe: Mutex::new(None),
            #[cfg(test)]
            physics_owner_apply_probe: Mutex::new(None),
            #[cfg(test)]
            physics_routing_probe: Mutex::new(None),
            #[cfg(test)]
            arrow_transaction_probe: Mutex::new(None),
            #[cfg(test)]
            breeding_plan_probe: Mutex::new(None),
            #[cfg(test)]
            breeding_commit_probe: Mutex::new(None),
            #[cfg(test)]
            sheep_grazing_plan_probe: Mutex::new(None),
            #[cfg(test)]
            sheep_grazing_commit_probe: Mutex::new(None),
            #[cfg(test)]
            sheep_grazing_owner_read_probe: Mutex::new(None),
            #[cfg(test)]
            entity_save_owner_probe: Mutex::new(None),
            #[cfg(test)]
            script_transaction_capture_probe: Mutex::new(None),
            #[cfg(test)]
            hostile_scan_probe: Mutex::new(None),
            #[cfg(test)]
            hostile_reconcile_probe: Mutex::new(None),
            #[cfg(test)]
            hostile_commit_probe: Mutex::new(None),
            #[cfg(test)]
            hostile_publication_probe: Mutex::new(None),
            #[cfg(test)]
            hostile_target_snapshot_probe: Mutex::new(None),
            #[cfg(test)]
            player_push_commit_probe: Mutex::new(None),
            #[cfg(test)]
            pickup_snapshot_probe: Mutex::new(None),
            #[cfg(test)]
            item_pickup_plan_probe: Mutex::new(None),
            #[cfg(test)]
            item_pickup_owner_probe: Mutex::new(None),
            #[cfg(test)]
            item_pickup_claimed_probe: Mutex::new(None),
            #[cfg(test)]
            server_container_dispatch_probe: Mutex::new(None),
            #[cfg(test)]
            server_relight_compute_probe: Mutex::new(None),
            #[cfg(test)]
            entity_goal_compute_probe: Mutex::new(None),
            #[cfg(test)]
            server_furnace_commit_probe: Mutex::new(None),
            #[cfg(test)]
            container_commit_probe: Mutex::new(None),
            #[cfg(test)]
            chunk_herd_claim_probe: Mutex::new(None),
            #[cfg(test)]
            campfire_d1_probe: Mutex::new(None),
            #[cfg(test)]
            campfire_entity_probe: Mutex::new(None),
            #[cfg(test)]
            physics_boundary_observer_scans: AtomicU64::new(0),
            #[cfg(test)]
            active_entity_selection_visits: AtomicU64::new(0),
            #[cfg(test)]
            player_push_entity_visits: AtomicU64::new(0),
            #[cfg(test)]
            breeding_state_updates: AtomicU64::new(0),
            #[cfg(test)]
            breeding_commits: AtomicU64::new(0),
            #[cfg(test)]
            breeding_entity_scan_visits: AtomicU64::new(0),
            #[cfg(test)]
            sheep_grazing_entity_visits: AtomicU64::new(0),
            #[cfg(test)]
            hostile_attack_candidates: AtomicU64::new(0),
            #[cfg(test)]
            hostile_entity_scan_visits: AtomicU64::new(0),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn configure_mob_behavior_table(
        &self,
        table: mc_data::mob_behavior_26_1_2::MobBehaviorTable,
    ) -> Result<(), mc_data::mob_behavior_26_1_2::MobBehaviorError> {
        table.validate()?;
        self.mob_behavior_table.store(Arc::new(table));
        Ok(())
    }

    fn mob_behavior_table(&self) -> Arc<mc_data::mob_behavior_26_1_2::MobBehaviorTable> {
        self.mob_behavior_table.load_full()
    }

    fn track_villager_override(&self, entity: EntityId) {
        self.overridden_villager_entities.rcu(|current| {
            if current.contains(&entity) {
                return Arc::clone(current);
            }
            let mut next = (**current).clone();
            next.insert(entity);
            Arc::new(next)
        });
    }

    fn clear_villager_overrides(&self, entities: &[EntityId]) {
        if entities.is_empty() {
            return;
        }
        self.overridden_villager_entities.rcu(|current| {
            let mut next = (**current).clone();
            let previous_len = next.len();
            for entity in entities {
                next.remove(entity);
            }
            if next.len() == previous_len {
                Arc::clone(current)
            } else {
                Arc::new(next)
            }
        });
    }

    fn overridden_villager_entities(&self) -> Arc<HashSet<EntityId>> {
        self.overridden_villager_entities.load_full()
    }

    pub(crate) fn configure_villager_brain_profile(
        &self,
        profile: mc_entity::villager_26_1_2::VillagerBrainProfile,
    ) -> Result<(), mc_entity::villager_26_1_2::VillagerBrainError> {
        profile.validate()?;
        self.villager_brain_profile.store(Arc::new(profile));
        Ok(())
    }

    fn villager_brain_profile(&self) -> Arc<mc_entity::villager_26_1_2::VillagerBrainProfile> {
        self.villager_brain_profile.load_full()
    }

    pub(in crate::play) fn loader_block_projection(
        &self,
        session_id: SessionId,
        blocks: &mc_world::BlockRegistry,
    ) -> Option<crate::loader::LoaderBlockProjection> {
        let inner = self.lock_inner("Loader block projection");
        inner
            .sessions
            .get(&session_id)?
            .loader_session
            .as_ref()?
            .block_projection(blocks)
    }

    pub(in crate::play) fn loader_item_placement_state(
        &self,
        session_id: SessionId,
        held: &ItemStack,
        items: &ItemRegistry,
        blocks: &mc_world::BlockRegistry,
    ) -> Option<mc_world::BlockStateId> {
        let paper = Identifier::parse("minecraft:paper").expect("static paper item id");
        if held.is_empty() || items.id_of(&paper) != Some(held.item_id) {
            return None;
        }
        let item_model = held.item_model.as_deref()?;
        let inner = self.lock_inner("Loader item placement state");
        let session = inner.sessions.get(&session_id)?;
        if session.tx.is_closed() {
            return None;
        }
        session
            .loader_session
            .as_ref()?
            .block_projection(blocks)
            .and_then(|projection| projection.canonical_state_for_item_model(item_model))
    }

    pub(in crate::play) fn loader_block_drop_stack(
        &self,
        session_id: SessionId,
        block_state: mc_world::BlockStateId,
        items: &ItemRegistry,
        blocks: &mc_world::BlockRegistry,
    ) -> Option<ItemStack> {
        let inner = self.lock_inner("Loader block drop stack");
        let session = inner.sessions.get(&session_id)?;
        if session.tx.is_closed() {
            return None;
        }
        let projection = session.loader_session.as_ref()?.block_projection(blocks)?;
        projection.item_stack_for_state(items, block_state, 1)
    }

    pub(crate) fn install_world_chunk_journal(
        &self,
        journal: super::world_journal::WorldChunkJournal,
    ) {
        *self
            .world_chunk_journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(journal);
    }

    pub(crate) fn world_chunk_journal(&self) -> Option<super::world_journal::WorldChunkJournal> {
        self.world_chunk_journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(crate) fn world_chunk_journal_watermark(&self) -> Option<u64> {
        self.world_chunk_journal()
            .and_then(|journal| journal.watermark())
    }

    pub(crate) fn subscribe_world_chunk_journal_failure(
        &self,
    ) -> tokio::sync::watch::Receiver<bool> {
        self.world_chunk_journal_failure.subscribe()
    }

    pub(crate) fn report_world_chunk_journal_failure(&self) {
        self.world_chunk_journal_failure.send_replace(true);
    }

    pub(crate) fn world_chunk_journal_failure_reporter(&self) -> tokio::sync::watch::Sender<bool> {
        self.world_chunk_journal_failure.clone()
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub(crate) fn new_with_entity_owner_lanes(lane_count: usize) -> Self {
        Self::with_entity_owner_lanes(lane_count)
    }

    #[must_use]
    pub(crate) fn new_with_entity_owner_journal(
        lane_count: usize,
        journal: Box<dyn mc_entity::RegionalDecisionJournal>,
    ) -> Self {
        Self::with_entity_owner_lanes_and_journal(lane_count, Some(journal))
    }

    pub(crate) fn clear_recovered_entity_commits(
        &self,
        phases: &[mc_entity::RegionPhase],
    ) -> Result<(), mc_entity::RegionOwnerLaneError> {
        self.entities
            .handle
            .clear_recovered_commits(phases.iter().copied())
    }

    pub(crate) fn reconfigure_entity_owner_lanes(&self, lane_count: usize) -> usize {
        #[cfg(test)]
        self.entity_owner_reconfiguration_calls
            .fetch_add(1, Ordering::Relaxed);
        self.entities.reconfigure_lanes(lane_count)
    }

    #[cfg(test)]
    pub(crate) fn entity_owner_reconfiguration_calls(&self) -> u64 {
        self.entity_owner_reconfiguration_calls
            .load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn prepared_chunk_shed_calls(&self) -> u64 {
        self.prepared_chunk_shed_calls.load(Ordering::Relaxed)
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn entity_owner_lane_count(&self) -> usize {
        self.entities.status().lane_count
    }

    #[cfg(test)]
    pub(super) fn reset_entity_owner_requests_for_test(&self) {
        self.entities.reset_owner_requests_for_test();
    }

    #[cfg(test)]
    pub(super) fn entity_owner_requests_for_test(&self) -> u64 {
        self.entities.owner_requests_for_test()
    }

    fn lock_inner(&self, operation: &'static str) -> SessionInnerGuard<'_> {
        SessionInnerGuard {
            guard: crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::SessionRegistry,
                operation,
                Instant::now(),
                self.inner.lock().unwrap_or_else(|poisoned| {
                    warn!(
                        operation,
                        "session registry mutex was poisoned; recovering state"
                    );
                    poisoned.into_inner()
                }),
            ),
            observation: &self.pressure_observation,
            dirty: false,
        }
    }

    fn publish_movement_recipient_index(&self, inner: &SessionRegistryInner) {
        self.movement_recipients
            .store(Arc::new(build_movement_recipient_index(&inner.sessions)));
    }

    fn publish_live_session_count(&self, inner: &SessionRegistryInner) -> bool {
        let live = inner
            .sessions
            .keys()
            .filter(|session_id| !inner.dead_sessions.contains(session_id))
            .count();
        let previous = self.live_session_count.swap(live, Ordering::AcqRel);
        if previous == live {
            return false;
        }
        if live == 0 {
            self.clear_active_simulation_entities();
        }
        self.live_session_generation.fetch_add(1, Ordering::Release);
        previous != 0 && live == 0
    }

    fn has_live_sessions(&self) -> bool {
        self.live_session_count.load(Ordering::Acquire) != 0
    }

    fn publish_active_entity_selection(
        &self,
        expected_live_session_generation: u64,
        entities: HashSet<EntityId>,
        hostiles: HashSet<EntityId>,
    ) {
        self.active_simulation_entities.store(Arc::new(entities));
        self.active_hostile_entities.store(Arc::new(hostiles));
        if self.live_session_generation.load(Ordering::Acquire) != expected_live_session_generation
            || !self.has_live_sessions()
        {
            self.clear_active_simulation_entities();
        }
    }

    fn publish_active_hostile_entity(&self, entity: EntityId) {
        if !self.has_live_sessions() {
            return;
        }
        let live_session_generation = self.live_session_generation.load(Ordering::Acquire);
        let current = self.active_hostile_entities.load_full();
        if current.contains(&entity) {
            return;
        }
        let mut next = (*current).clone();
        next.insert(entity);
        self.active_hostile_entities.store(Arc::new(next));
        if self.live_session_generation.load(Ordering::Acquire) != live_session_generation
            || !self.has_live_sessions()
        {
            self.active_hostile_entities.store(Arc::new(HashSet::new()));
        }
    }

    fn clear_active_simulation_entities(&self) {
        if !self.active_simulation_entities.load().is_empty() {
            self.active_simulation_entities
                .store(Arc::new(HashSet::new()));
        }
        if !self.active_hostile_entities.load().is_empty() {
            self.active_hostile_entities.store(Arc::new(HashSet::new()));
        }
    }

    #[cfg(test)]
    pub(in crate::play) fn publish_active_simulation_entities_for_test(
        &self,
        entities: impl IntoIterator<Item = EntityId>,
    ) {
        let entities = entities.into_iter().collect::<HashSet<_>>();
        self.refresh_breeding_tick_entities_for_test(entities.iter().copied());
        self.publish_active_entity_selection(
            self.live_session_generation.load(Ordering::Acquire),
            entities,
            HashSet::new(),
        );
    }

    #[cfg(test)]
    fn refresh_breeding_tick_entities_for_test(
        &self,
        entities: impl IntoIterator<Item = EntityId>,
    ) {
        let owner = self.lock_entities("publish test active breeding entities");
        for entity_id in entities {
            self.simulation_inputs.update_breeding_tick_entity(
                entity_id,
                owner.snapshot(entity_id).and_then(|entity| entity.animal),
            );
        }
    }

    fn lock_entities(&self, operation: &'static str) -> EntityStoreGuard<'_> {
        let _ = operation;
        EntityStoreGuard {
            access: self.entities.access(),
            marker: std::marker::PhantomData,
        }
    }

    fn current_expected_entity_snapshots(
        &self,
        expected: impl IntoIterator<Item = EntitySnapshot>,
    ) -> Vec<EntitySnapshot> {
        let expected = expected.into_iter().collect::<Vec<_>>();
        let ids = expected
            .iter()
            .map(|snapshot| snapshot.id)
            .collect::<HashSet<_>>();
        #[cfg(test)]
        self.entities.record_owner_request_for_test();
        let current = owner_result(self.entities.handle.snapshots_for_ids(&ids));
        filter_current_expected_entity_snapshots(expected, current)
    }

    fn lock_session_entities(&self, operation: &'static str) -> SessionEntityGuards<'_> {
        let entities = self.lock_entities(operation);
        let inner = self.lock_inner(operation);
        SessionEntityGuards {
            inner,
            entities,
            entity_lifecycle_tick: self.simulation_tick(),
        }
    }

    #[cfg(test)]
    fn pause_before_move_fanout_for_test(&self) {
        let probe = self
            .move_fanout_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe.reached.send(()).expect("move fanout probe receiver");
            probe.resume.recv().expect("move fanout probe release");
        }
    }

    #[cfg(test)]
    fn pause_after_movement_recipient_validation_for_test(&self) {
        let probe = self
            .movement_dispatch_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            let _ = probe.reached.send(());
            let _ = probe.resume.recv();
        }
    }

    #[cfg(test)]
    fn pause_after_movement_visibility_load_for_test(&self) {
        let probe = self
            .movement_visibility_load_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe
                .reached
                .send(())
                .expect("movement visibility-load probe receiver");
            probe
                .resume
                .recv()
                .expect("movement visibility-load probe release");
        }
    }

    #[cfg(test)]
    fn pause_before_session_movement_plan_for_test(&self) {
        let probe = self
            .entity_apply_release_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe.reached.send(()).expect("entity apply probe receiver");
            probe.resume.recv().expect("entity apply probe release");
        }
    }

    #[cfg(test)]
    fn pause_before_physics_owner_apply_for_test(&self) {
        let probe = self
            .physics_owner_apply_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe
                .reached
                .send(())
                .expect("physics owner apply probe receiver");
            probe
                .resume
                .recv()
                .expect("physics owner apply probe release");
        }
    }

    #[cfg(test)]
    fn pause_before_physics_routing_for_test(&self) {
        let probe = self
            .physics_routing_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe
                .reached
                .send(())
                .expect("physics routing probe receiver");
            probe.resume.recv().expect("physics routing probe release");
        }
    }

    #[cfg(test)]
    fn install_arrow_transaction_probe(
        &self,
        reached: std::sync::mpsc::Sender<()>,
        resume: std::sync::mpsc::Receiver<()>,
    ) {
        *self
            .arrow_transaction_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(ArrowTransactionProbe { reached, resume });
    }

    #[cfg(test)]
    fn pause_before_arrow_transaction_for_test(&self) {
        let probe = self
            .arrow_transaction_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe
                .reached
                .send(())
                .expect("arrow transaction probe receiver");
            probe
                .resume
                .recv()
                .expect("arrow transaction probe release");
        }
    }

    #[cfg(test)]
    fn pause_during_breeding_plan_for_test(&self) {
        let probe = self
            .breeding_plan_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe.reached.send(()).expect("breeding probe receiver");
            probe.resume.recv().expect("breeding probe release");
        }
    }

    #[cfg(test)]
    fn pause_between_breeding_entity_and_session_commit_for_test(&self) {
        let probe = self
            .breeding_commit_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe
                .reached
                .send(())
                .expect("breeding commit probe receiver");
            probe.resume.recv().expect("breeding commit probe release");
        }
    }

    #[cfg(test)]
    fn pause_during_sheep_grazing_plan_for_test(&self) {
        let probe = self
            .sheep_grazing_plan_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe
                .reached
                .send(())
                .expect("sheep grazing probe receiver");
            probe.resume.recv().expect("sheep grazing probe release");
        }
    }

    #[cfg(test)]
    fn pause_between_sheep_grazing_entity_and_session_commit_for_test(&self) {
        let probe = self
            .sheep_grazing_commit_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe
                .reached
                .send(())
                .expect("sheep grazing commit probe receiver");
            probe
                .resume
                .recv()
                .expect("sheep grazing commit probe release");
        }
    }

    #[cfg(test)]
    fn pause_before_sheep_grazing_owner_read_for_test(&self) {
        let probe = self
            .sheep_grazing_owner_read_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe
                .reached
                .send(())
                .expect("sheep grazing owner read probe receiver");
            probe
                .resume
                .recv()
                .expect("sheep grazing owner read probe release");
        }
    }

    #[cfg(test)]
    fn pause_before_entity_save_owner_barrier_for_test(&self) {
        let probe = self
            .entity_save_owner_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe
                .reached
                .send(())
                .expect("entity save owner probe receiver");
            probe
                .resume
                .recv()
                .expect("entity save owner probe release");
        }
    }

    #[cfg(test)]
    fn pause_between_player_push_entity_and_session_commit_for_test(&self) {
        let probe = self
            .player_push_commit_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe
                .reached
                .send(())
                .expect("player push commit probe receiver");
            probe
                .resume
                .recv()
                .expect("player push commit probe release");
        }
    }

    #[cfg(test)]
    fn pause_during_pickup_snapshot_for_test(&self) {
        let probe = self
            .pickup_snapshot_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe
                .reached
                .send(())
                .expect("pickup snapshot probe receiver");
            probe.resume.recv().expect("pickup snapshot probe release");
        }
    }

    #[cfg(test)]
    pub(super) fn install_server_relight_compute_probe(
        &self,
        reached: std::sync::mpsc::Sender<()>,
        resume: std::sync::mpsc::Receiver<()>,
    ) {
        *self
            .server_relight_compute_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(ServerRelightComputeProbe { reached, resume });
    }

    #[cfg(test)]
    pub(super) fn pause_before_server_relight_compute_for_test(&self) {
        let probe = self
            .server_relight_compute_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe
                .reached
                .send(())
                .expect("server relight compute probe receiver");
            probe
                .resume
                .recv()
                .expect("server relight compute probe release");
        }
    }

    #[cfg(test)]
    fn install_entity_goal_compute_probe(
        &self,
        reached: std::sync::mpsc::Sender<()>,
        resume: std::sync::mpsc::Receiver<()>,
    ) {
        *self
            .entity_goal_compute_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(EntityGoalComputeProbe { reached, resume });
    }

    #[cfg(test)]
    fn pause_before_entity_goal_compute_for_test(&self) {
        let probe = self
            .entity_goal_compute_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe
                .reached
                .send(())
                .expect("entity goal compute probe receiver");
            probe
                .resume
                .recv()
                .expect("entity goal compute probe release");
        }
    }

    pub(crate) fn simulation_tick(&self) -> u64 {
        self.entity_lifecycle_tick.load(Ordering::Acquire)
    }

    pub(crate) fn subscribe_simulation_ticks(&self) -> tokio::sync::watch::Receiver<u64> {
        self.simulation_tick_sender.subscribe()
    }

    pub(crate) fn install_script_commit_event_outbox(
        &self,
    ) -> tokio::sync::mpsc::UnboundedReceiver<ScriptEvent> {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut inner = self.lock_inner("install script commit event outbox");
        assert!(
            inner.script_commit_events.replace(sender).is_none(),
            "script commit event outbox may only be installed once"
        );
        receiver
    }

    pub(crate) fn close_script_commit_event_outbox(&self) {
        self.lock_inner("close script commit event outbox")
            .script_commit_events = None;
    }

    pub(crate) fn subscribe_player_attacks(
        &self,
    ) -> tokio::sync::broadcast::Receiver<PlayerAttackObservation> {
        self.player_attack_sender.subscribe()
    }

    pub(crate) fn publish_player_attack(
        &self,
        attacker_session_id: SessionId,
        target_entity_id: i32,
        cooldown_tick: u64,
        authority_tick: u64,
    ) {
        let authority_sequence = self
            .player_attack_sequence
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        let observation = PlayerAttackObservation {
            attacker_session_id,
            target_entity_id,
            cooldown_tick,
            authority_tick,
            authority_sequence,
        };
        let _ = self.player_attack_sender.send(observation);
    }

    pub(super) fn player_pose(&self, id: SessionId) -> Option<PlayerPose> {
        let inner = self.lock_inner("read player pose");
        inner.sessions.get(&id).map(|session| session.pose)
    }

    pub(crate) fn pressure_snapshot(&self) -> SessionPressureSnapshot {
        self.pressure_observation.snapshot(&self.outbound_pressure)
    }

    pub(crate) fn pressure_change_generation(&self) -> u64 {
        self.outbound_pressure.change_generation()
    }

    pub(crate) async fn wait_for_pressure_change(&self, observed: u64) {
        self.outbound_pressure.wait_for_change(observed).await;
    }

    pub(super) fn record_slow_client_write_timeout(&self) {
        self.outbound_pressure.record_slow_client_write_timeout();
    }

    #[cfg(test)]
    pub(super) fn record_slow_client_pressure_shed(&self) {
        self.outbound_pressure.record_slow_client_pressure_shed();
    }

    #[cfg(test)]
    pub(crate) fn set_sheep_sheared_for_test(&self, entity_id: EntityId, sheared: bool) -> bool {
        let mut inner = self.lock_session_entities("set test sheep sheared state");
        let Some(entity) = inner.entities.snapshot(entity_id) else {
            return false;
        };
        let Some(mut animal) = entity.animal else {
            return false;
        };
        let Some(mut wool) = animal.sheep_wool else {
            return false;
        };
        wool.sheared = sheared;
        animal.sheep_wool = Some(wool);
        if !inner.entities.set_animal_state(entity_id, animal) {
            return false;
        }
        if let Some(snapshot) = inner.published_entity_snapshots.get_mut(&entity_id) {
            snapshot.animal = Some(animal);
        }
        true
    }

    #[cfg(test)]
    fn breeding_state_update_count(&self) -> u64 {
        self.breeding_state_updates.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn breeding_commit_count(&self) -> u64 {
        self.breeding_commits.load(Ordering::Relaxed)
    }

    pub(crate) fn configure_arrow_kill_rewards(
        &self,
        item_entity_type_id: Option<i32>,
        xp_orb_entity_type_id: Option<i32>,
        arrow_entity_type_id: Option<i32>,
        items: Arc<ItemRegistry>,
        item_facts: Arc<ItemFactsTable>,
        loot: Arc<mc_data::loot::LootTables>,
    ) {
        let mut inner = self.lock_inner("configure arrow kill rewards");
        inner.arrow_kill_rewards = ArrowKillRewards {
            item_entity_type_id,
            xp_orb_entity_type_id,
            items: Some(items),
            item_facts: Some(item_facts),
            loot: Some(loot),
        };
        self.hostile_arrow_entity_type_id
            .store(arrow_entity_type_id.unwrap_or(-1), Ordering::Release);
    }

    pub(crate) fn configure_player_combat(
        &self,
        item_entity_type_id: Option<i32>,
        xp_orb_entity_type_id: Option<i32>,
        items: Arc<ItemRegistry>,
        item_facts: Arc<ItemFactsTable>,
    ) {
        let mut inner = self.lock_inner("configure player combat resources");
        inner.player_combat = PlayerCombatResources {
            item_entity_type_id,
            xp_orb_entity_type_id,
            items,
            item_facts,
        };
    }

    #[cfg(test)]
    pub(super) fn spawn_arrow_for_test(
        &self,
        owner_session: Option<SessionId>,
        entity_type_id: i32,
        position: Vec3,
        velocity: Vec3,
        rotation: Rotation,
    ) -> Vec<VisibilityDispatch> {
        self.spawn_arrow_owned(owner_session, entity_type_id, position, velocity, rotation)
    }

    #[cfg(test)]
    fn spawn_arrow_owned(
        &self,
        owner_session: Option<SessionId>,
        entity_type_id: i32,
        position: Vec3,
        velocity: Vec3,
        rotation: Rotation,
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_session_entities("spawn arrow");
        spawn_arrow_locked(
            &mut inner,
            owner_session,
            entity_type_id,
            position,
            velocity,
            rotation,
        )
        .1
    }

    #[cfg(test)]
    pub(super) fn nearby_item_entities(
        &self,
        position: Vec3,
        radius: f64,
    ) -> Vec<ServerEntitySnapshot> {
        let inner = self.lock_session_entities("nearby item entities");
        nearby_entity_snapshots_locked(&inner, position, radius, |entity| {
            entity.item_stack.is_some()
                && item_pickup_ready_locked(&inner, entity.id, inner.entity_lifecycle_tick)
        })
    }

    #[cfg(test)]
    pub(super) fn nearby_grounded_arrows(
        &self,
        position: Vec3,
        radius: f64,
    ) -> Vec<ServerEntitySnapshot> {
        let inner = self.lock_session_entities("nearby grounded arrows");
        nearby_entity_snapshots_locked(&inner, position, radius, |entity| {
            entity.type_name == "minecraft:arrow"
                && entity.on_ground
                && entity.velocity == Vec3::ZERO
        })
    }

    #[cfg(test)]
    pub(super) fn nearby_experience_entities(
        &self,
        position: Vec3,
        radius: f64,
    ) -> Vec<ServerEntitySnapshot> {
        let inner = self.lock_session_entities("nearby experience entities");
        nearby_entity_snapshots_locked(&inner, position, radius, |entity| {
            entity.experience_value.is_some()
        })
    }

    #[cfg(test)]
    pub(super) fn server_entity_snapshot(
        &self,
        entity_id: EntityId,
    ) -> Option<ServerEntitySnapshot> {
        let entities = self.lock_entities("server entity snapshot");
        entities
            .snapshot(entity_id)
            .map(server_entity_snapshot_from)
    }

    #[cfg(test)]
    pub(crate) fn authoritative_entity_snapshot(
        &self,
        entity_id: EntityId,
    ) -> Option<EntitySnapshot> {
        self.lock_entities("authoritative entity snapshot test API")
            .snapshot(entity_id)
    }

    #[cfg(test)]
    pub(super) fn apply_player_melee_knockback_legacy_for_test(
        &self,
        entity_id: EntityId,
        player_position: Vec3,
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_session_entities("legacy player melee knockback test");
        apply_player_melee_knockback_locked(&mut inner, entity_id, player_position)
    }

    #[cfg(test)]
    fn terrain_pathing_entity_count(&self) -> usize {
        self.simulation_inputs.terrain_pathing_entities().len()
    }
}

pub(crate) struct ScheduledBlockTickAdmission<'a> {
    in_flight: &'a AtomicBool,
}

impl Drop for ScheduledBlockTickAdmission<'_> {
    fn drop(&mut self) {
        self.in_flight.store(false, Ordering::Release);
    }
}

impl SessionRegistry {
    pub(crate) fn try_begin_scheduled_block_ticks(
        &self,
    ) -> Option<ScheduledBlockTickAdmission<'_>> {
        self.scheduled_block_tick_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| ScheduledBlockTickAdmission {
                in_flight: &self.scheduled_block_tick_in_flight,
            })
    }
}

fn apply_entity_facts(entity: &mut SpawnEntity) {
    let Some(facts) = interaction_geometry::canonical_entity_facts(&entity.type_name) else {
        return;
    };
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
    match entity.type_name.as_str() {
        "minecraft:sheep" => {
            entity.animal = Some(mc_entity::AnimalBreedingState::adult_sheep(
                mc_entity::SheepColor::White,
            ));
        }
        "minecraft:cow" | "minecraft:chicken" => {
            entity.animal = Some(mc_entity::AnimalBreedingState::adult());
        }
        _ => {}
    }
}

fn schedule_item_despawn_locked(
    inner: &mut SessionRegistryInner,
    entity_id: EntityId,
    spawn_tick: u64,
) {
    let deadline = spawn_tick.saturating_add(ITEM_DESPAWN_AGE_TICKS);
    let previous = inner
        .item_despawn_deadline_by_id
        .insert(entity_id, deadline);
    if previous == Some(deadline) {
        return;
    }
    if let Some(previous) = previous {
        let remove_bucket = inner
            .item_despawn_deadlines
            .get_mut(&previous)
            .is_some_and(|bucket| {
                bucket.retain(|queued| *queued != entity_id);
                bucket.is_empty()
            });
        if remove_bucket {
            inner.item_despawn_deadlines.remove(&previous);
        }
    }
    inner
        .item_despawn_deadlines
        .entry(deadline)
        .or_default()
        .push_back(entity_id);
}

fn despawn_expired_items_locked(inner: &mut SessionEntityGuards<'_>) -> Vec<VisibilityDispatch> {
    let current_tick = inner.entity_lifecycle_tick;
    let mut expired = Vec::with_capacity(ITEM_DESPAWN_SWEEP_BUDGET);
    while expired.len() < ITEM_DESPAWN_SWEEP_BUDGET {
        let Some(mut entry) = inner.item_despawn_deadlines.first_entry() else {
            break;
        };
        if *entry.key() > current_tick {
            break;
        }
        let deadline = *entry.key();
        let entity_id = entry
            .get_mut()
            .pop_front()
            .expect("item despawn deadline bucket is non-empty");
        if entry.get().is_empty() {
            entry.remove_entry();
        }
        if inner.item_despawn_deadline_by_id.get(&entity_id) != Some(&deadline) {
            continue;
        }
        inner.item_despawn_deadline_by_id.remove(&entity_id);
        expired.push(entity_id);
    }
    expired
        .into_iter()
        .filter_map(|entity_id| {
            let entity = inner.entities.snapshot(entity_id)?;
            entity.item_stack.as_ref()?;
            if entity.retained.item_pickup_claim.is_some() {
                let retry_tick = current_tick.saturating_add(1);
                inner
                    .item_despawn_deadline_by_id
                    .insert(entity_id, retry_tick);
                inner
                    .item_despawn_deadlines
                    .entry(retry_tick)
                    .or_default()
                    .push_back(entity_id);
                return None;
            }
            let deadline = entity
                .retained
                .spawn_tick
                .saturating_add(ITEM_DESPAWN_AGE_TICKS);
            if deadline > current_tick {
                schedule_item_despawn_locked(inner, entity_id, entity.retained.spawn_tick);
                return None;
            }
            match remove_server_entity_locked(inner, entity_id) {
                Some((_, dispatches)) => Some(dispatches),
                None => {
                    if let Some(current) = inner.entities.snapshot(entity_id)
                        && current.item_stack.is_some()
                    {
                        schedule_item_despawn_locked(inner, entity_id, current.retained.spawn_tick);
                    }
                    None
                }
            }
        })
        .flatten()
        .collect()
}

fn entity_kill_drop_stacks(
    config: &ArrowKillRewards,
    entity_type: &str,
    animal: Option<mc_entity::AnimalBreedingState>,
    seed: u64,
) -> Vec<ItemStack> {
    let (Some(loot), Some(items), Some(item_facts)) = (
        config.loot.as_deref(),
        config.items.as_deref(),
        config.item_facts.as_deref(),
    ) else {
        return Vec::new();
    };
    let mut drops = mob_drop_stacks_from_seed(loot, items, item_facts, entity_type, seed);
    if entity_type != "minecraft:sheep" {
        return drops;
    }
    let Some(wool) = animal.and_then(|animal| animal.sheep_wool) else {
        return drops;
    };
    let wool_item_id = (!wool.sheared)
        .then(|| Identifier::parse(wool.color.wool_item_name()).ok())
        .flatten()
        .and_then(|item| items.id_of(&item));
    if !wool.sheared && wool_item_id.is_none() {
        return drops;
    }

    drops.retain(|stack| {
        !items
            .name_of(stack.item_id)
            .is_some_and(|item| item.namespace() == "minecraft" && item.path().ends_with("_wool"))
    });
    if let Some(item_id) = wool_item_id {
        drops.push(ItemStack::new(item_id, 1));
    }
    drops
}

const PLAYER_MELEE_KNOCKBACK_HORIZONTAL: f64 = 0.45;
const PLAYER_MELEE_KNOCKBACK_VERTICAL: f64 = 0.20;

fn apply_player_melee_knockback_locked(
    inner: &mut SessionEntityGuards<'_>,
    target_id: EntityId,
    player_position: Vec3,
) -> Vec<VisibilityDispatch> {
    let Some(target) = inner.entities.snapshot(target_id) else {
        return Vec::new();
    };
    let dx = target.position.x - player_position.x;
    let dz = target.position.z - player_position.z;
    let horizontal = dx.hypot(dz);
    if horizontal <= f64::EPSILON {
        return Vec::new();
    }
    let velocity = Vec3::new(
        target.velocity.x + dx / horizontal * PLAYER_MELEE_KNOCKBACK_HORIZONTAL,
        (target.velocity.y + PLAYER_MELEE_KNOCKBACK_VERTICAL).max(PLAYER_MELEE_KNOCKBACK_VERTICAL),
        target.velocity.z + dz / horizontal * PLAYER_MELEE_KNOCKBACK_HORIZONTAL,
    );
    if !inner.entities.set_velocity(target_id, velocity) {
        return Vec::new();
    }
    let snapshot = if let Some(snapshot) = inner.published_entity_snapshots.get_mut(&target_id) {
        snapshot.velocity = velocity;
        snapshot.clone()
    } else {
        let Some(snapshot) = publish_server_entity_snapshot_locked(inner, target_id) else {
            return Vec::new();
        };
        snapshot
    };
    visible_entity_observers_locked(inner, target_id)
        .into_iter()
        .filter_map(|observer_id| {
            let observer = inner.sessions.get(&observer_id)?;
            Some(VisibilityDispatch {
                recipient: ordered_session_recipient(observer_id, observer),
                command: OutboundCommand::MoveEntityRelative(ServerEntityMove {
                    id: target_id,
                    position: snapshot.position,
                    wire_move: None,
                    velocity: snapshot.velocity,
                    rotation: snapshot.rotation,
                    on_ground: snapshot.on_ground,
                    send_velocity: true,
                    send_head_rotation: false,
                }),
            })
        })
        .collect()
}

fn player_collision_position(pose: PlayerPose) -> Vec3 {
    Vec3::new(pose.x, pose.y, pose.z)
}

fn player_aabb() -> mc_physics::Aabb {
    mc_physics::Aabb {
        half_width: 0.3,
        height: 1.8,
    }
}

fn record_entity_dispatches_locked(
    inner: &mut SessionRegistryInner,
    dispatches: &[VisibilityDispatch],
) {
    for dispatch in dispatches {
        match &dispatch.command {
            OutboundCommand::SpawnEntity(_) => inner.entity_dispatches.spawn += 1,
            OutboundCommand::SpawnEntities(entities) => {
                inner.entity_dispatches.spawn += entities.len() as u64;
            }
            OutboundCommand::UpdateEntityData(_) | OutboundCommand::UpdateEntityHealth(_) => {
                inner.entity_dispatches.data += 1;
            }
            OutboundCommand::MoveEntityRelative(_) => inner.entity_dispatches.move_relative += 1,
            OutboundCommand::MoveEntitiesRelative(movements) => {
                inner.entity_dispatches.move_relative += movements.len() as u64;
            }
            OutboundCommand::TakeItemEntity { .. } => inner.entity_dispatches.take += 1,
            OutboundCommand::PickupCandidates(_) => {}
            OutboundCommand::DespawnEntity(_) => inner.entity_dispatches.remove += 1,
            _ => {}
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

fn add_loaded_chunk_reference_locked(inner: &mut SessionRegistryInner, chunk: (i32, i32)) {
    let refcount = inner.loaded_chunk_refcounts.entry(chunk).or_default();
    let became_active = *refcount == 0;
    *refcount += 1;
    if became_active {
        inner.simulation_inputs.insert_active_chunk(chunk);
    }
}

fn remove_loaded_chunk_reference_locked(inner: &mut SessionRegistryInner, chunk: (i32, i32)) {
    let Some(refcount) = inner.loaded_chunk_refcounts.get_mut(&chunk) else {
        debug_assert!(false, "loaded chunk index is missing {chunk:?}");
        return;
    };
    debug_assert!(*refcount > 0, "loaded chunk refcount is zero for {chunk:?}");
    *refcount = refcount.saturating_sub(1);
    if *refcount == 0 {
        inner.loaded_chunk_refcounts.remove(&chunk);
        inner.simulation_inputs.remove_active_chunk(chunk);
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
#[path = "session/visibility_tests.rs"]
mod visibility_tests;
