//! # mc-entity
//!
//! Entity system, AI, pathfinding.
//!
//! Part of the Solaris engine.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::f64::consts::TAU;
use std::ops::Range;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

// W05 stays caller-neutral until regional authority owns transition execution.
#[allow(dead_code)]
pub(crate) mod ai_core_26_1_2;
pub mod attributes_26_1_2;
pub mod effects_26_1_2;
pub mod equipment_26_1_2;
pub mod living_26_1_2;
pub mod mob_control_26_1_2;
pub mod navigation_26_1_2;
pub mod projectile_26_1_2;
mod regional;
mod runtime;
pub mod runtime_26_1_2;
pub mod synced_data_26_1_2;

pub use regional::VillagerBindingClaim;
pub use regional::{
    REGION_SIZE_CHUNKS, RegionEntityStoreError, RegionEpoch, RegionKey, RegionLease,
    RegionOwnerBatch, RegionOwnerCompletion, RegionOwnerLaneError, RegionOwnerLaneStartError,
    RegionOwnerMutation, RegionOwnership, RegionOwnershipError, RegionPhase,
    RegionalCommitDecision, RegionalDecisionJournal, RegionalDecisionJournalError,
    RegionalEntityAuthority, RegionalEntityStore, RegionalKinematicsApply,
    RegionalOwnerCoordinator, RegionalOwnerCutoverError, RegionalOwnerHandle, RegionalOwnerLane,
    RegionalOwnerRuntime, RegionalOwnerRuntimeShutdownError, RegionalOwnerSaveSnapshot,
    RegionalOwnerShutdownError, RegionalOwnerStatus, RegionalPreparedGoalTick,
    RegionalResolvedGoalTick, SequencedRegionMutation, TransferApply, TransferDecision, TransferId,
    VersionedEntitySnapshots,
};

pub use runtime::{
    EntityCombatCommand, EntityEffectApplied, EntityEffectOperation, EntityEffectRejection,
    EntityEffectRequest, EntityEffectResult, EntityInputCommand, EntityPhysicsResult,
    EntityRuntime, EntityStage,
};

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Stable runtime entity id used by the server and vanilla protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EntityId(pub i32);

/// 3D vector for entity positions and velocities.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    #[must_use]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    #[must_use]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    #[must_use]
    pub fn horizontal_len(self) -> f64 {
        self.x.hypot(self.z)
    }

    #[must_use]
    pub fn horizontal_normalized(self) -> Self {
        let len = self.horizontal_len();
        if len <= f64::EPSILON {
            Self::ZERO
        } else {
            Self {
                x: self.x / len,
                y: 0.0,
                z: self.z / len,
            }
        }
    }
}

/// Entity yaw/pitch/head-yaw in degrees.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rotation {
    pub yaw: f32,
    pub pitch: f32,
    pub head_yaw: f32,
}

impl Rotation {
    pub const ZERO: Self = Self {
        yaw: 0.0,
        pitch: 0.0,
        head_yaw: 0.0,
    };

    #[must_use]
    pub fn is_finite(self) -> bool {
        self.yaw.is_finite() && self.pitch.is_finite() && self.head_yaw.is_finite()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntityLifecycle {
    Alive,
    Despawning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityItemStack {
    pub item_id: u32,
    pub count: i32,
    pub damage: Option<i32>,
    pub enchantments: Vec<mc_data::ItemEnchantment>,
}

impl EntityItemStack {
    #[must_use]
    pub const fn new(item_id: u32, count: i32) -> Self {
        Self {
            item_id,
            count,
            damage: None,
            enchantments: Vec::new(),
        }
    }

    #[must_use]
    pub const fn with_damage(mut self, damage: i32) -> Self {
        self.damage = Some(damage);
        self
    }

    #[must_use]
    pub fn with_enchantment(mut self, id: mc_data::Identifier, level: i32) -> Self {
        self.enchantments.retain(|enchantment| enchantment.id != id);
        self.enchantments
            .push(mc_data::ItemEnchantment { id, level });
        self.enchantments
            .sort_unstable_by(|left, right| left.id.cmp(&right.id));
        self
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.count <= 0
    }
}

pub const BABY_START_AGE_TICKS: i32 = -24_000;
pub const PARENT_BREEDING_COOLDOWN_TICKS: i32 = 6_000;
pub const ANIMAL_LOVE_DURATION_TICKS: u16 = 600;
pub const ANIMAL_BREEDING_COURTSHIP_TICKS: u16 = 60;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum SheepColor {
    #[default]
    White = 0,
    Orange = 1,
    Magenta = 2,
    LightBlue = 3,
    Yellow = 4,
    Lime = 5,
    Pink = 6,
    Gray = 7,
    LightGray = 8,
    Cyan = 9,
    Purple = 10,
    Blue = 11,
    Brown = 12,
    Green = 13,
    Red = 14,
    Black = 15,
}

impl SheepColor {
    pub const ALL: [Self; 16] = [
        Self::White,
        Self::Orange,
        Self::Magenta,
        Self::LightBlue,
        Self::Yellow,
        Self::Lime,
        Self::Pink,
        Self::Gray,
        Self::LightGray,
        Self::Cyan,
        Self::Purple,
        Self::Blue,
        Self::Brown,
        Self::Green,
        Self::Red,
        Self::Black,
    ];

    #[must_use]
    pub const fn from_id(id: u8) -> Option<Self> {
        Some(match id {
            0 => Self::White,
            1 => Self::Orange,
            2 => Self::Magenta,
            3 => Self::LightBlue,
            4 => Self::Yellow,
            5 => Self::Lime,
            6 => Self::Pink,
            7 => Self::Gray,
            8 => Self::LightGray,
            9 => Self::Cyan,
            10 => Self::Purple,
            11 => Self::Blue,
            12 => Self::Brown,
            13 => Self::Green,
            14 => Self::Red,
            15 => Self::Black,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn id(self) -> u8 {
        self as u8
    }

    #[must_use]
    pub const fn wool_item_name(self) -> &'static str {
        match self {
            Self::White => "minecraft:white_wool",
            Self::Orange => "minecraft:orange_wool",
            Self::Magenta => "minecraft:magenta_wool",
            Self::LightBlue => "minecraft:light_blue_wool",
            Self::Yellow => "minecraft:yellow_wool",
            Self::Lime => "minecraft:lime_wool",
            Self::Pink => "minecraft:pink_wool",
            Self::Gray => "minecraft:gray_wool",
            Self::LightGray => "minecraft:light_gray_wool",
            Self::Cyan => "minecraft:cyan_wool",
            Self::Purple => "minecraft:purple_wool",
            Self::Blue => "minecraft:blue_wool",
            Self::Brown => "minecraft:brown_wool",
            Self::Green => "minecraft:green_wool",
            Self::Red => "minecraft:red_wool",
            Self::Black => "minecraft:black_wool",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SheepWoolState {
    pub color: SheepColor,
    pub sheared: bool,
}

impl SheepWoolState {
    #[must_use]
    pub const fn packed_metadata(self) -> i8 {
        (self.color.id() | if self.sheared { 0x10 } else { 0 }) as i8
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimalBreedingState {
    pub age_ticks: i32,
    pub love_ticks: u16,
    pub sheep_wool: Option<SheepWoolState>,
}

impl AnimalBreedingState {
    #[must_use]
    pub const fn needs_breeding_tick(self) -> bool {
        self.age_ticks != 0 || self.love_ticks != 0
    }

    #[must_use]
    pub const fn adult() -> Self {
        Self {
            age_ticks: 0,
            love_ticks: 0,
            sheep_wool: None,
        }
    }

    #[must_use]
    pub const fn baby() -> Self {
        Self {
            age_ticks: BABY_START_AGE_TICKS,
            love_ticks: 0,
            sheep_wool: None,
        }
    }

    #[must_use]
    pub const fn adult_sheep(color: SheepColor) -> Self {
        Self {
            age_ticks: 0,
            love_ticks: 0,
            sheep_wool: Some(SheepWoolState {
                color,
                sheared: false,
            }),
        }
    }

    #[must_use]
    pub const fn is_baby(self) -> bool {
        self.age_ticks < 0
    }

    #[must_use]
    pub const fn can_fall_in_love(self) -> bool {
        self.age_ticks == 0 && self.love_ticks == 0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpawnEntity {
    pub uuid: Option<Uuid>,
    pub type_id: i32,
    pub type_name: String,
    pub position: Vec3,
    pub rotation: Rotation,
    pub velocity: Vec3,
    pub on_ground: bool,
    pub item_stack: Option<EntityItemStack>,
    pub experience_value: Option<i32>,
    pub block_state: Option<u32>,
    pub attributes: AttributeSet,
    pub goal: GoalState,
    pub vehicle: Option<VehicleState>,
    pub animal: Option<AnimalBreedingState>,
    pub retained: EntityRetainedState,
}

impl SpawnEntity {
    #[must_use]
    pub fn new(type_id: i32, type_name: impl Into<String>, position: Vec3) -> Self {
        Self {
            uuid: None,
            type_id,
            type_name: type_name.into(),
            position,
            rotation: Rotation::ZERO,
            velocity: Vec3::ZERO,
            on_ground: true,
            item_stack: None,
            experience_value: None,
            block_state: None,
            attributes: AttributeSet::vanilla_mob_defaults(),
            goal: GoalState::Idle,
            vehicle: None,
            animal: None,
            retained: EntityRetainedState::default(),
        }
    }

    #[must_use]
    pub fn vehicle(
        kind: VehicleKind,
        type_id: i32,
        type_name: impl Into<String>,
        position: Vec3,
    ) -> Self {
        let mut entity = Self::new(type_id, type_name, position);
        entity.vehicle = Some(VehicleState::new(kind));
        entity.attributes = AttributeSet::new();
        entity
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntitySnapshot {
    pub id: EntityId,
    pub uuid: Uuid,
    pub type_id: i32,
    pub type_name: String,
    pub position: Vec3,
    pub rotation: Rotation,
    pub velocity: Vec3,
    pub on_ground: bool,
    pub item_stack: Option<EntityItemStack>,
    pub experience_value: Option<i32>,
    pub block_state: Option<u32>,
    pub lifecycle: EntityLifecycle,
    pub health: f32,
    pub attributes: AttributeSet,
    pub goal: GoalState,
    pub vehicle: Option<VehicleState>,
    pub animal: Option<AnimalBreedingState>,
    pub retained: EntityRetainedState,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EntityRetainedState {
    path: RetainedPathState,
    pub living: EntityLivingRetainedState,
    pub active_effects: Option<EntityActiveEffectsState>,
    pub arrow_state: Option<projectile_26_1_2::ArrowState>,
    pub last_damage_tick: Option<u64>,
    pub death_remove_tick: Option<u64>,
    pub sheep_grazing_ticks: Option<u8>,
    pub spawn_tick: u64,
    pub item_pickup_ready_tick: Option<u64>,
    pub item_pickup_owner_block: Option<EntityItemPickupOwnerBlock>,
    pub primed_tnt: Option<EntityPrimedTntState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct EntityLivingRetainedState {
    pub absorption: f32,
    pub invulnerable_time: u32,
    pub hurt_time: u32,
    pub last_hurt: f32,
    pub death_time: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityActiveEffectsState {
    pub effects: effects_26_1_2::ActiveEffectsSnapshot,
    pub action_order: Vec<effects_26_1_2::EffectId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityItemPickupOwnerBlock {
    pub owner_session: u64,
    pub expires_tick: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityPrimedTntState {
    pub expires_tick: u64,
    pub air_block_state: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityKinematics {
    pub id: EntityId,
    pub position: Vec3,
    pub rotation: Rotation,
    pub velocity: Vec3,
    pub on_ground: bool,
}

impl EntityKinematics {
    fn is_finite(self) -> bool {
        self.position.is_finite() && self.rotation.is_finite() && self.velocity.is_finite()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityMotionState {
    pub id: EntityId,
    pub position: Vec3,
    pub rotation: Rotation,
    pub velocity: Vec3,
    pub on_ground: bool,
    pub is_item: bool,
    pub is_experience: bool,
    pub is_arrow: bool,
    pub arrow_revision: Option<u64>,
    pub arrow_embedded_block: Option<projectile_26_1_2::BlockPosition>,
    pub sends_velocity: bool,
}

#[derive(Debug, Clone)]
pub struct EntityView<'a> {
    pub id: EntityId,
    pub uuid: Uuid,
    pub type_id: i32,
    pub type_name: &'a str,
    pub position: Vec3,
    pub rotation: Rotation,
    pub velocity: Vec3,
    pub on_ground: bool,
    pub item_stack: Option<EntityItemStack>,
    pub experience_value: Option<i32>,
    pub block_state: Option<u32>,
    pub lifecycle: EntityLifecycle,
    pub health: f32,
    pub attributes: &'a AttributeSet,
    pub goal: &'a GoalState,
    pub vehicle: Option<VehicleState>,
    pub animal: Option<AnimalBreedingState>,
    pub retained: EntityRetainedState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntityDamage {
    pub snapshot: EntitySnapshot,
    pub killed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityDamageRequest {
    pub amount: f32,
    pub tick: u64,
    pub death_remove_tick: u64,
}

impl EntityDamageRequest {
    #[must_use]
    pub fn is_valid(self) -> bool {
        self.amount.is_finite() && self.amount > 0.0 && self.death_remove_tick >= self.tick
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VehicleKind {
    Boat,
    Minecart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VehicleState {
    pub kind: VehicleKind,
    pub passenger: Option<EntityId>,
}

impl VehicleState {
    #[must_use]
    pub const fn new(kind: VehicleKind) -> Self {
        Self {
            kind,
            passenger: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VehicleInput {
    pub left: bool,
    pub right: bool,
    pub forward: bool,
    pub backward: bool,
}

impl VehicleInput {
    #[must_use]
    pub const fn forward() -> Self {
        Self {
            left: false,
            right: false,
            forward: true,
            backward: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VehicleError {
    MissingVehicle,
    MissingPassenger,
    NotVehicle,
    SelfMount,
    Cycle,
    InvalidLifecycle,
    AlreadyMounted,
    PassengerAlreadyMounted,
    PassengerMismatch,
    UnsupportedSteering,
}

/// Small vanilla attribute subset needed before real mob AI/combat.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AttributeKind {
    MaxHealth,
    MovementSpeed,
    FollowRange,
    AttackDamage,
    Custom(String),
}

impl AttributeKind {
    #[must_use]
    pub fn vanilla_name(&self) -> &str {
        match self {
            Self::MaxHealth => "minecraft:max_health",
            Self::MovementSpeed => "minecraft:movement_speed",
            Self::FollowRange => "minecraft:follow_range",
            Self::AttackDamage => "minecraft:attack_damage",
            Self::Custom(name) => name.as_str(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AttributeValue {
    pub base: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AttributeSet {
    values: BTreeMap<AttributeKind, AttributeValue>,
}

impl Serialize for AttributeSet {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.values.iter().collect::<Vec<_>>().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AttributeSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let values = Vec::<(AttributeKind, AttributeValue)>::deserialize(deserializer)?;
        Ok(Self {
            values: values.into_iter().collect(),
        })
    }
}

impl AttributeSet {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn vanilla_mob_defaults() -> Self {
        let mut attrs = Self::new();
        attrs.set_base(AttributeKind::MaxHealth, 20.0);
        attrs.set_base(AttributeKind::MovementSpeed, 0.25);
        attrs.set_base(AttributeKind::FollowRange, 16.0);
        attrs.set_base(AttributeKind::AttackDamage, 0.0);
        attrs
    }

    pub fn set_base(&mut self, kind: AttributeKind, base: f64) {
        self.values.insert(kind, AttributeValue { base });
    }

    #[must_use]
    pub fn base(&self, kind: &AttributeKind) -> Option<f64> {
        self.values.get(kind).map(|value| value.base)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&AttributeKind, &AttributeValue)> {
        self.values.iter()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GoalState {
    Idle,
    Wander {
        speed: f64,
        period_ticks: u32,
    },
    AquaticWander {
        speed: f64,
        vertical_speed: f64,
        period_ticks: u32,
    },
    FollowTarget {
        target: EntityId,
        speed: f64,
    },
    FollowPosition {
        target: Vec3,
        speed: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathingProbeResult {
    Walkable,
    Blocked,
    Unloaded,
}

pub trait PathingProbe {
    fn can_stand_at(&self, position: Vec3) -> PathingProbeResult;

    fn can_entity_stand_at(&self, _entity_id: EntityId, position: Vec3) -> PathingProbeResult {
        self.can_stand_at(position)
    }

    fn direct_path_resolved(&self, _entity_id: EntityId) {}
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathingBudget {
    /// Historical name: this is the hard upper bound for actual probe calls per entity.
    pub max_candidates_per_entity: usize,
    pub step_height: f64,
}

impl PathingBudget {
    pub const TICK_SECONDS: f64 = 0.05;

    pub const DEFAULT: Self = Self {
        max_candidates_per_entity: 8,
        step_height: 1.0,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathingDecisionKind {
    Move,
    Blocked,
    Unloaded,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct PathingDecision {
    velocity: Vec3,
    kind: PathingDecisionKind,
    direct: bool,
}

const RETAINED_PATH_NODE_CAPACITY: usize = 2;
const RETAINED_PATH_NODE_DISTANCE: f64 = 1.5;
const RETAINED_PATH_PROGRESS_EPSILON: f64 = 1.0e-4;
const RETAINED_PATH_NO_PROGRESS_LIMIT: u8 = 6;
const RETAINED_PATH_RECOMPUTE_LIMIT: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct RetainedPathState {
    nodes: [Vec3; RETAINED_PATH_NODE_CAPACITY],
    node_count: u8,
    current_node: u8,
    target: Vec3,
    target_revision: u64,
    target_epoch: Option<u64>,
    has_target: bool,
    last_position: Vec3,
    has_last_position: bool,
    no_progress_ticks: u8,
    recomputations: u8,
    was_moving: bool,
    stopped: bool,
}

impl Default for RetainedPathState {
    fn default() -> Self {
        Self {
            nodes: [Vec3::ZERO; RETAINED_PATH_NODE_CAPACITY],
            node_count: 0,
            current_node: 0,
            target: Vec3::ZERO,
            target_revision: 0,
            target_epoch: None,
            has_target: false,
            last_position: Vec3::ZERO,
            has_last_position: false,
            no_progress_ticks: 0,
            recomputations: 0,
            was_moving: false,
            stopped: false,
        }
    }
}

impl RetainedPathState {
    fn current_target(self) -> Option<Vec3> {
        (self.current_node < self.node_count).then(|| self.nodes[usize::from(self.current_node)])
    }

    fn clear_nodes(&mut self) {
        self.node_count = 0;
        self.current_node = 0;
    }

    fn retain_direct_target(&mut self) {
        self.nodes[0] = self.target;
        self.node_count = 1;
        self.current_node = 0;
    }

    fn retain_detour(&mut self, current: Vec3, direction: Vec3) {
        self.nodes[0] = Vec3 {
            x: current.x + direction.x * RETAINED_PATH_NODE_DISTANCE,
            y: current.y,
            z: current.z + direction.z * RETAINED_PATH_NODE_DISTANCE,
        };
        self.nodes[1] = self.target;
        self.node_count = RETAINED_PATH_NODE_CAPACITY as u8;
        self.current_node = 0;
    }
}

#[derive(Debug, Clone, PartialEq)]
struct GoalPathingRequest {
    id: EntityId,
    expected_position: Vec3,
    expected_rotation: Rotation,
    expected_velocity: Vec3,
    expected_on_ground: bool,
    expected_goal: GoalState,
    expected_path: RetainedPathState,
    target: Vec3,
    target_epoch: Option<u64>,
    speed: f64,
}

#[derive(Debug, Clone, PartialEq)]
struct GoalPathingResult {
    request: GoalPathingRequest,
    decision: PathingDecision,
    next_path: RetainedPathState,
}

impl GoalPathingResult {
    fn matches(
        &self,
        position: Vec3,
        rotation: Rotation,
        velocity: Vec3,
        on_ground: bool,
        goal: &GoalState,
        path: &RetainedPathState,
    ) -> bool {
        self.request.expected_position == position
            && self.request.expected_rotation == rotation
            && self.request.expected_velocity == velocity
            && self.request.expected_on_ground == on_ground
            && &self.request.expected_goal == goal
            && &self.request.expected_path == path
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreparedGoalTick {
    tick: u64,
    active_ids: Option<HashSet<EntityId>>,
    pathing_requests: Vec<GoalPathingRequest>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedGoalTick {
    tick: u64,
    active_ids: Option<HashSet<EntityId>>,
    pathing_results: BTreeMap<EntityId, GoalPathingResult>,
}

impl PreparedGoalTick {
    #[must_use]
    pub fn pathing_request_count(&self) -> usize {
        self.pathing_requests.len()
    }

    pub fn visit_pathing_probe_positions(
        &self,
        budget: PathingBudget,
        mut visitor: impl FnMut(EntityId, Vec3),
    ) {
        for request in &self.pathing_requests {
            if request.speed <= 0.0 {
                continue;
            }
            let navigation_target = request
                .expected_path
                .current_target()
                .unwrap_or(request.target);
            visit_bounded_pathing_probe_positions(
                request.expected_position,
                navigation_target,
                request.speed,
                budget,
                |position| visitor(request.id, position),
            );
            if navigation_target != request.target {
                visit_bounded_pathing_probe_positions(
                    request.expected_position,
                    request.target,
                    request.speed,
                    budget,
                    |position| visitor(request.id, position),
                );
            }
        }
    }

    #[must_use]
    pub fn resolve(self, probe: &dyn PathingProbe, budget: PathingBudget) -> ResolvedGoalTick {
        let pathing_results = self
            .pathing_requests
            .into_iter()
            .map(|request| {
                let id = request.id;
                let (decision, next_path) = resolve_retained_pathing(&request, probe, budget);
                (
                    id,
                    GoalPathingResult {
                        request,
                        decision,
                        next_path,
                    },
                )
            })
            .collect();
        ResolvedGoalTick {
            tick: self.tick,
            active_ids: self.active_ids,
            pathing_results,
        }
    }
}

/// Observable counters from an AI goal tick.
///
/// These counters intentionally describe the read-only decision/application
/// boundary: every alive entity with a goal produces one applied decision, while
/// despawning entities are skipped and missing follow targets are reported
/// without mutating unrelated entities.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GoalTickStats {
    pub alive_entities: usize,
    pub decisions_applied: usize,
    pub skipped_non_alive: usize,
    pub missing_follow_targets: usize,
    pub pathing_moves: usize,
    pub pathing_blocked: usize,
    pub pathing_unloaded: usize,
}

/// Entity storage backed by the ECS runtime.
#[derive(Debug, Default)]
pub struct EntityStore {
    next_id: i32,
    runtime: EntityRuntime,
}

impl EntityStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_next_id(next_id: i32) -> Self {
        Self {
            next_id,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.runtime.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.runtime.is_empty()
    }

    #[must_use]
    pub fn batch_ranges(&self, batch_size: usize) -> Vec<Range<usize>> {
        let batch_size = batch_size.max(1);
        (0..self.len())
            .step_by(batch_size)
            .map(|start| start..(start + batch_size).min(self.len()))
            .collect()
    }

    pub fn spawn(&mut self, entity: SpawnEntity) -> EntityId {
        let id = self.allocate_id();
        let uuid = entity.uuid.unwrap_or_else(|| deterministic_uuid(id));
        assert!(!self.contains_uuid(uuid), "entity UUID already exists");
        let snapshot = snapshot_from_spawn(id, uuid, entity);

        {
            let inserted = self.insert_runtime_snapshot(snapshot);
            debug_assert!(inserted, "fresh entity id must be vacant in ECS");
        }

        id
    }

    pub fn spawn_batch(
        &mut self,
        entities: impl IntoIterator<Item = SpawnEntity>,
    ) -> Vec<EntityId> {
        let mut pending = Vec::new();
        let mut pending_uuids = HashSet::new();
        for entity in entities {
            let id = self.allocate_id();
            let uuid = entity.uuid.unwrap_or_else(|| deterministic_uuid(id));
            assert!(
                !self.contains_uuid(uuid) && pending_uuids.insert(uuid),
                "entity UUID already exists"
            );
            let mut snapshot = snapshot_from_spawn(id, uuid, entity);
            let vehicle = snapshot.vehicle.take();
            self.runtime
                .queue_input(EntityInputCommand::Insert(Box::new(snapshot)));
            pending.push((id, vehicle));
        }
        if pending.is_empty() {
            return Vec::new();
        }

        self.runtime.run_stage(EntityStage::InputAi);
        let mut ids = Vec::with_capacity(pending.len());
        for &(id, _) in &pending {
            assert!(
                self.runtime.contains(id),
                "fresh entity id must be present in ECS after batch insert"
            );

            ids.push(id);
        }

        let mut queued_vehicle = false;
        for (id, requested) in pending {
            if requested.is_none() {
                continue;
            }
            let vehicle = self.sanitized_snapshot_vehicle(id, EntityLifecycle::Alive, requested);
            self.runtime
                .queue_input(EntityInputCommand::SetVehicle { id, vehicle });
            queued_vehicle = true;
        }
        if queued_vehicle {
            self.runtime.run_stage(EntityStage::InputAi);
        }
        ids
    }

    #[must_use]
    pub fn contains_uuid(&self, uuid: Uuid) -> bool {
        self.runtime.contains_uuid(uuid)
    }

    pub fn insert_snapshot(&mut self, snapshot: EntitySnapshot) -> bool {
        if self.contains(snapshot.id) || self.contains_uuid(snapshot.uuid) {
            return false;
        }
        self.next_id = self.next_id.max(snapshot.id.0);

        self.insert_runtime_snapshot(snapshot)
    }

    pub(crate) fn restore_snapshot_in_place(&mut self, snapshot: EntitySnapshot) -> bool {
        self.runtime.restore_snapshot_in_place(snapshot)
    }

    pub(crate) fn effect_checkpoint(
        &self,
        id: EntityId,
    ) -> Option<runtime::EntityEffectCheckpoint> {
        self.runtime.effect_checkpoint(id)
    }

    pub(crate) fn restore_effect_checkpoint(
        &mut self,
        checkpoint: runtime::EntityEffectCheckpoint,
    ) -> bool {
        self.runtime.restore_effect_checkpoint(checkpoint)
    }

    pub fn apply_effect(
        &mut self,
        id: EntityId,
        request: EntityEffectRequest,
    ) -> EntityEffectResult {
        self.runtime.apply_effect(id, request)
    }

    pub fn insert_snapshots_batch(
        &mut self,
        snapshots: impl IntoIterator<Item = EntitySnapshot>,
    ) -> bool {
        let snapshots = snapshots.into_iter().collect::<Vec<_>>();
        let mut pending_ids = HashSet::new();
        let mut pending_uuids = HashSet::new();
        if snapshots.iter().any(|snapshot| {
            self.contains(snapshot.id)
                || self.contains_uuid(snapshot.uuid)
                || !pending_ids.insert(snapshot.id)
                || !pending_uuids.insert(snapshot.uuid)
        }) {
            return false;
        }
        if !self.vehicle_graph_accepts(&snapshots) {
            return false;
        }
        if snapshots.is_empty() {
            return true;
        }

        let mut pending = Vec::with_capacity(snapshots.len());
        let mut max_id = self.next_id;
        for mut snapshot in snapshots {
            max_id = max_id.max(snapshot.id.0);
            let id = snapshot.id;
            let vehicle = snapshot.vehicle.take();
            self.runtime
                .queue_input(EntityInputCommand::Insert(Box::new(snapshot)));
            pending.push((id, vehicle));
        }
        self.runtime.run_stage(EntityStage::InputAi);
        for &(id, _) in &pending {
            assert!(
                self.runtime.contains(id),
                "preflighted batch snapshot must enter ECS authority"
            );
        }
        let mut queued_vehicle = false;
        for (id, requested) in pending {
            if requested.is_none() {
                continue;
            }
            let lifecycle = self
                .runtime
                .snapshot(id)
                .expect("inserted batch snapshot")
                .lifecycle;
            let vehicle = self.sanitized_snapshot_vehicle(id, lifecycle, requested);
            self.runtime
                .queue_input(EntityInputCommand::SetVehicle { id, vehicle });
            queued_vehicle = true;
        }
        if queued_vehicle {
            self.runtime.run_stage(EntityStage::InputAi);
        }
        self.next_id = max_id;
        true
    }

    fn vehicle_graph_accepts(&self, pending: &[EntitySnapshot]) -> bool {
        let mut entities = self
            .snapshots()
            .map(|snapshot| (snapshot.id, snapshot))
            .collect::<HashMap<_, _>>();
        entities.extend(
            pending
                .iter()
                .cloned()
                .map(|snapshot| (snapshot.id, snapshot)),
        );
        let mut passenger_owners = HashMap::new();
        for entity in entities.values() {
            let Some(vehicle) = entity.vehicle else {
                continue;
            };
            if entity.lifecycle != EntityLifecycle::Alive {
                return false;
            }
            let Some(passenger) = vehicle.passenger else {
                continue;
            };
            if passenger == entity.id
                || entities
                    .get(&passenger)
                    .is_none_or(|passenger| passenger.lifecycle != EntityLifecycle::Alive)
                || passenger_owners.insert(passenger, entity.id).is_some()
            {
                return false;
            }
        }
        for &start in entities.keys() {
            let mut current = Some(start);
            let mut visited = HashSet::new();
            while let Some(id) = current {
                if !visited.insert(id) {
                    return false;
                }
                current = entities
                    .get(&id)
                    .and_then(|entity| entity.vehicle)
                    .and_then(|vehicle| vehicle.passenger);
            }
        }
        true
    }

    #[must_use]
    pub fn contains(&self, id: EntityId) -> bool {
        self.runtime.contains(id)
    }

    #[must_use]
    pub fn snapshot(&self, id: EntityId) -> Option<EntitySnapshot> {
        self.runtime.snapshot(id)
    }

    #[must_use]
    pub fn motion_state(&self, id: EntityId) -> Option<EntityMotionState> {
        self.runtime.motion_state(id)
    }

    pub fn alive_kinematics_for_ids(&mut self, ids: &HashSet<EntityId>) -> Vec<EntityKinematics> {
        self.runtime.alive_kinematics_for_ids(ids)
    }

    #[must_use]
    pub fn view(&self, id: EntityId) -> Option<EntityView<'_>> {
        self.runtime.view(id)
    }

    pub fn snapshots(&self) -> impl Iterator<Item = EntitySnapshot> + '_ {
        let snapshots = self.runtime.normalized_snapshots();
        snapshots.into_iter()
    }

    #[cfg(test)]
    fn input_ai_stage_runs_for_test(&self) -> usize {
        self.runtime.input_ai_stage_runs()
    }

    #[cfg(test)]
    fn physics_apply_stage_runs_for_test(&self) -> usize {
        self.runtime.physics_apply_stage_runs()
    }

    fn insert_runtime_snapshot(&mut self, mut snapshot: EntitySnapshot) -> bool {
        snapshot.vehicle =
            self.sanitized_snapshot_vehicle(snapshot.id, snapshot.lifecycle, snapshot.vehicle);
        let id = snapshot.id;
        self.runtime
            .queue_input(EntityInputCommand::Insert(Box::new(snapshot)));
        self.runtime.run_stage(EntityStage::InputAi);
        if self.runtime.snapshot(id).is_none() {
            return false;
        }
        true
    }

    pub fn views(&self) -> impl Iterator<Item = EntityView<'_>> + '_ {
        self.runtime.views()
    }

    pub fn visit_simulation_entities(&self, mut visitor: impl FnMut(EntityView<'_>)) {
        self.runtime.visit_entities(&mut visitor);
    }

    pub fn visit_breeding_tick_entities(&self, mut visitor: impl FnMut(EntityView<'_>)) {
        self.runtime.visit_breeding_tick_entities(&mut visitor);
    }

    pub fn visit_sheep_entities_for_ids(
        &self,
        ids: &HashSet<EntityId>,
        mut visitor: impl FnMut(EntityView<'_>),
    ) {
        self.runtime.visit_sheep_entities_for_ids(ids, &mut visitor);
    }

    pub fn visit_simulation_entities_for_ids(
        &self,
        ids: &HashSet<EntityId>,
        mut visitor: impl FnMut(EntityView<'_>),
    ) {
        {
            let mut ordered_ids = ids.iter().copied().collect::<Vec<_>>();
            ordered_ids.sort_unstable();
            for id in ordered_ids {
                self.runtime.visit_entity(id, &mut visitor);
            }
        }
    }

    fn is_runtime_entity(&self, id: EntityId) -> bool {
        self.runtime.contains(id)
    }

    pub fn apply_kinematics(
        &mut self,
        states: impl IntoIterator<Item = EntityKinematics>,
    ) -> usize {
        let mut applied = 0;
        let mut physics_queued = false;
        for state in states {
            if !state.is_finite() {
                continue;
            }
            if self.is_runtime_entity(state.id) {
                if !self.runtime.contains(state.id) {
                    continue;
                }
                self.runtime.queue_physics(EntityPhysicsResult {
                    id: state.id,
                    position: state.position,
                    rotation: state.rotation,
                    velocity: state.velocity,
                    on_ground: state.on_ground,
                });
                applied += 1;
                physics_queued = true;
                continue;
            }
        }
        if physics_queued {
            self.runtime.run_stage(EntityStage::PhysicsApply);
        }
        applied
    }

    pub fn mark_despawning(&mut self, id: EntityId) -> bool {
        if self.is_runtime_entity(id) {
            let Some(snapshot) = self.runtime.snapshot(id) else {
                return false;
            };
            if snapshot.lifecycle == EntityLifecycle::Despawning {
                return true;
            }
            self.runtime
                .queue_combat(EntityCombatCommand::MarkDespawning { id });
            self.runtime.run_stage(EntityStage::CombatLifecycle);
            return true;
        }

        false
    }

    pub fn remove(&mut self, id: EntityId) -> Option<EntitySnapshot> {
        if self.is_runtime_entity(id) {
            let removed = self.runtime.snapshot(id)?;
            self.clear_vehicle_passenger_refs(id);
            self.runtime
                .queue_combat(EntityCombatCommand::Remove { id });
            self.runtime.run_stage(EntityStage::CombatLifecycle);
            return Some(removed);
        }

        None
    }

    pub fn set_position(&mut self, id: EntityId, position: Vec3) -> bool {
        if !position.is_finite() {
            return false;
        }
        if self.is_runtime_entity(id) {
            let Some(snapshot) = self.runtime.snapshot(id) else {
                return false;
            };
            self.runtime
                .queue_input(EntityInputCommand::ResetPath { id });
            self.runtime.run_stage(EntityStage::InputAi);
            self.runtime.queue_physics(EntityPhysicsResult {
                id,
                position,
                rotation: snapshot.rotation,
                velocity: snapshot.velocity,
                on_ground: snapshot.on_ground,
            });
            self.runtime.run_stage(EntityStage::PhysicsApply);
            return true;
        }

        false
    }

    pub fn set_velocity(&mut self, id: EntityId, velocity: Vec3) -> bool {
        if !velocity.is_finite() {
            return false;
        }
        if self.is_runtime_entity(id) {
            let Some(snapshot) = self.runtime.snapshot(id) else {
                return false;
            };
            self.runtime.queue_physics(EntityPhysicsResult {
                id,
                position: snapshot.position,
                rotation: snapshot.rotation,
                velocity,
                on_ground: snapshot.on_ground,
            });
            self.runtime.run_stage(EntityStage::PhysicsApply);
            return true;
        }

        false
    }

    pub fn set_on_ground(&mut self, id: EntityId, on_ground: bool) -> bool {
        if self.is_runtime_entity(id) {
            let Some(snapshot) = self.runtime.snapshot(id) else {
                return false;
            };
            self.runtime.queue_physics(EntityPhysicsResult {
                id,
                position: snapshot.position,
                rotation: snapshot.rotation,
                velocity: snapshot.velocity,
                on_ground,
            });
            self.runtime.run_stage(EntityStage::PhysicsApply);
            return true;
        }

        false
    }

    pub fn set_item_stack(&mut self, id: EntityId, item_stack: Option<EntityItemStack>) -> bool {
        if self.is_runtime_entity(id) {
            self.runtime.queue_input(EntityInputCommand::SetItemStack {
                id,
                stack: item_stack,
            });
            self.runtime.run_stage(EntityStage::InputAi);
            return true;
        }

        false
    }

    pub fn set_goal(&mut self, id: EntityId, goal: GoalState) -> bool {
        self.set_goals([(id, goal)]) == 1
    }

    pub fn set_goals(&mut self, goals: impl IntoIterator<Item = (EntityId, GoalState)>) -> usize {
        let mut updated = 0;
        let mut queued_input = false;
        for (id, goal) in goals {
            if self.is_runtime_entity(id) {
                if self.runtime.goal_matches(id, &goal) {
                    updated += 1;
                    continue;
                }
                self.runtime
                    .queue_input(EntityInputCommand::SetGoal { id, goal });
                queued_input = true;
                updated += 1;
                continue;
            }
        }
        if queued_input {
            self.runtime.run_stage(EntityStage::InputAi);
        }
        updated
    }

    pub fn set_animal_state(&mut self, id: EntityId, animal: AnimalBreedingState) -> bool {
        self.set_animal_states([(id, animal)]) == 1
    }

    pub fn set_animal_states(
        &mut self,
        states: impl IntoIterator<Item = (EntityId, AnimalBreedingState)>,
    ) -> usize {
        let mut applied = 0;
        let mut ecs_queued = false;
        for (id, animal) in states {
            if self.is_runtime_entity(id) {
                if self
                    .runtime
                    .snapshot(id)
                    .is_none_or(|snapshot| snapshot.animal.is_none())
                {
                    continue;
                }
                self.runtime
                    .queue_input(EntityInputCommand::SetAnimalState { id, animal });
                applied += 1;
                ecs_queued = true;
                continue;
            }
        }
        if ecs_queued {
            self.runtime.run_stage(EntityStage::InputAi);
        }
        applied
    }

    pub fn damage(&mut self, id: EntityId, request: EntityDamageRequest) -> Option<EntityDamage> {
        if !request.is_valid() {
            return None;
        }
        if self.is_runtime_entity(id) {
            if self.runtime.snapshot(id)?.lifecycle != EntityLifecycle::Alive {
                return None;
            }
            self.runtime
                .queue_combat(EntityCombatCommand::Damage { id, request });
            self.runtime.run_stage(EntityStage::CombatLifecycle);
            let snapshot = self.runtime.snapshot(id)?;
            return Some(EntityDamage {
                killed: snapshot.lifecycle == EntityLifecycle::Despawning,
                snapshot,
            });
        }

        None
    }

    pub fn mount_vehicle(
        &mut self,
        vehicle: EntityId,
        passenger: EntityId,
    ) -> Result<(), VehicleError> {
        if vehicle == passenger {
            return Err(VehicleError::SelfMount);
        }
        let vehicle_snapshot = self.snapshot(vehicle).ok_or(VehicleError::MissingVehicle)?;
        let passenger_snapshot = self
            .snapshot(passenger)
            .ok_or(VehicleError::MissingPassenger)?;
        if vehicle_snapshot.lifecycle != EntityLifecycle::Alive
            || passenger_snapshot.lifecycle != EntityLifecycle::Alive
        {
            return Err(VehicleError::InvalidLifecycle);
        }
        if self.vehicle_for_passenger(passenger).is_some() {
            return Err(VehicleError::PassengerAlreadyMounted);
        }
        if self.passenger_chain_contains(passenger, vehicle) {
            return Err(VehicleError::Cycle);
        }
        let mut state = vehicle_snapshot.vehicle.ok_or(VehicleError::NotVehicle)?;
        if state.passenger.is_some() {
            return Err(VehicleError::AlreadyMounted);
        }
        state.passenger = Some(passenger);
        self.set_vehicle_state(vehicle, Some(state));
        Ok(())
    }

    pub fn dismount_vehicle(
        &mut self,
        vehicle: EntityId,
        passenger: EntityId,
    ) -> Result<(), VehicleError> {
        let mut state = self
            .snapshot(vehicle)
            .ok_or(VehicleError::MissingVehicle)?
            .vehicle
            .ok_or(VehicleError::NotVehicle)?;
        if state.passenger != Some(passenger) {
            return Err(VehicleError::PassengerMismatch);
        }
        state.passenger = None;
        self.set_vehicle_state(vehicle, Some(state));
        Ok(())
    }

    pub fn apply_vehicle_input(
        &mut self,
        vehicle: EntityId,
        passenger: EntityId,
        input: VehicleInput,
    ) -> Result<(), VehicleError> {
        let vehicle_snapshot = self.snapshot(vehicle).ok_or(VehicleError::MissingVehicle)?;
        let passenger_snapshot = self
            .snapshot(passenger)
            .ok_or(VehicleError::MissingPassenger)?;
        if vehicle_snapshot.lifecycle != EntityLifecycle::Alive
            || passenger_snapshot.lifecycle != EntityLifecycle::Alive
        {
            return Err(VehicleError::InvalidLifecycle);
        }
        let state = vehicle_snapshot.vehicle.ok_or(VehicleError::NotVehicle)?;
        if state.passenger != Some(passenger) {
            return Err(VehicleError::PassengerMismatch);
        }
        let mut rotation = vehicle_snapshot.rotation;
        let mut velocity = vehicle_snapshot.velocity;
        match state.kind {
            VehicleKind::Boat => {
                let yaw_delta = match (input.left, input.right) {
                    (true, false) => -4.0,
                    (false, true) => 4.0,
                    _ => 0.0,
                };
                rotation.yaw += yaw_delta;
                rotation.head_yaw = rotation.yaw;

                let speed = match (input.forward, input.backward) {
                    (true, false) => 0.35,
                    (false, true) => -0.12,
                    _ => 0.0,
                };
                let radians = f64::from(rotation.yaw).to_radians();
                velocity.x = -radians.sin() * speed;
                velocity.z = radians.cos() * speed;
                velocity.y = 0.0;
            }
            VehicleKind::Minecart => return Err(VehicleError::UnsupportedSteering),
        }
        self.apply_kinematics([EntityKinematics {
            id: vehicle,
            position: vehicle_snapshot.position,
            rotation,
            velocity,
            on_ground: vehicle_snapshot.on_ground,
        }]);
        Ok(())
    }

    #[must_use]
    pub fn vehicle_for_passenger(&self, passenger: EntityId) -> Option<EntityId> {
        self.snapshots()
            .find_map(|entity| (entity.vehicle?.passenger == Some(passenger)).then_some(entity.id))
    }

    fn passenger_chain_contains(&self, start: EntityId, target: EntityId) -> bool {
        let mut current = Some(start);
        for _ in 0..self.len() {
            let Some(id) = current else {
                return false;
            };
            if id == target {
                return true;
            }
            let Some(snapshot) = self.snapshot(id) else {
                return false;
            };
            current = snapshot.vehicle.and_then(|state| state.passenger);
        }
        false
    }

    fn set_vehicle_state(&mut self, id: EntityId, vehicle: Option<VehicleState>) -> bool {
        if self.is_runtime_entity(id) {
            self.runtime
                .queue_input(EntityInputCommand::SetVehicle { id, vehicle });
            self.runtime.run_stage(EntityStage::InputAi);
            return true;
        }

        false
    }

    fn sanitized_snapshot_vehicle(
        &self,
        id: EntityId,
        lifecycle: EntityLifecycle,
        vehicle: Option<VehicleState>,
    ) -> Option<VehicleState> {
        if lifecycle != EntityLifecycle::Alive {
            return None;
        }
        let state = vehicle?;
        let Some(passenger) = state.passenger else {
            return Some(state);
        };
        if passenger == id || self.vehicle_for_passenger(passenger).is_some() {
            return None;
        }

        let mut current = Some(passenger);
        for _ in 0..=self.len() {
            let Some(current_id) = current else {
                return Some(state);
            };
            if current_id == id {
                return None;
            }
            let current_snapshot = self.snapshot(current_id)?;
            if current_snapshot.lifecycle != EntityLifecycle::Alive {
                return None;
            }
            current = current_snapshot.vehicle.and_then(|state| state.passenger);
        }
        None
    }

    fn clear_vehicle_passenger_refs(&mut self, passenger: EntityId) {
        let changed = self
            .snapshots()
            .filter_map(|entity| {
                let mut vehicle = entity.vehicle?;
                if vehicle.passenger != Some(passenger) {
                    return None;
                }
                vehicle.passenger = None;
                Some((entity.id, vehicle))
            })
            .collect::<Vec<_>>();
        for (id, vehicle) in changed {
            self.set_vehicle_state(id, Some(vehicle));
        }
    }

    pub fn attributes_mut(&mut self, id: EntityId) -> Option<&mut AttributeSet> {
        if self.is_runtime_entity(id) {
            return self.runtime.attributes_mut(id);
        }

        None
    }

    pub fn tick_goals(&mut self, tick: u64) {
        let _ = self.tick_goals_with_stats(tick);
    }

    pub fn tick_goals_with_stats(&mut self, tick: u64) -> GoalTickStats {
        self.tick_goals_internal(tick, None, None)
    }

    pub fn tick_goals_with_pathing<P: PathingProbe>(
        &mut self,
        tick: u64,
        probe: &P,
        budget: PathingBudget,
    ) -> GoalTickStats {
        self.tick_goals_internal(tick, Some((probe, budget)), None)
    }

    pub fn tick_goals_with_pathing_for_ids<P: PathingProbe>(
        &mut self,
        tick: u64,
        probe: &P,
        budget: PathingBudget,
        active_ids: &HashSet<EntityId>,
    ) -> GoalTickStats {
        self.tick_goals_internal(tick, Some((probe, budget)), Some(active_ids))
    }

    pub fn prepare_goal_tick_with_pathing_for_ids(
        &mut self,
        tick: u64,
        active_ids: &HashSet<EntityId>,
    ) -> PreparedGoalTick {
        self.prepare_goal_tick(tick, Some(active_ids))
    }

    fn prepare_goal_tick(
        &mut self,
        tick: u64,
        active_ids: Option<&HashSet<EntityId>>,
    ) -> PreparedGoalTick {
        let active_ids = active_ids.filter(|active_ids| {
            active_ids.len() != self.len() || !active_ids.iter().all(|id| self.contains(*id))
        });
        let mut pathing_requests = Vec::new();

        pathing_requests.extend(self.runtime.pathing_requests(tick, active_ids));
        PreparedGoalTick {
            tick,
            active_ids: active_ids.cloned(),
            pathing_requests,
        }
    }

    pub fn apply_prepared_goal_tick(&mut self, resolved: ResolvedGoalTick) -> GoalTickStats {
        let ResolvedGoalTick {
            tick,
            active_ids,
            pathing_results,
        } = resolved;
        self.apply_goal_tick(tick, true, &pathing_results, active_ids.as_ref(), None)
    }

    pub(crate) fn apply_prepared_goal_tick_with_follow_targets(
        &mut self,
        resolved: ResolvedGoalTick,
        follow_targets: &HashMap<EntityId, Vec3>,
    ) -> GoalTickStats {
        let ResolvedGoalTick {
            tick,
            active_ids,
            pathing_results,
        } = resolved;
        self.apply_goal_tick(
            tick,
            true,
            &pathing_results,
            active_ids.as_ref(),
            Some(follow_targets),
        )
    }

    fn tick_goals_internal(
        &mut self,
        tick: u64,
        pathing: Option<(&dyn PathingProbe, PathingBudget)>,
        active_ids: Option<&HashSet<EntityId>>,
    ) -> GoalTickStats {
        if let Some((probe, budget)) = pathing {
            let prepared = self.prepare_goal_tick(tick, active_ids);
            return self.apply_prepared_goal_tick(prepared.resolve(probe, budget));
        }
        self.apply_goal_tick(tick, false, &BTreeMap::new(), active_ids, None)
    }

    fn apply_goal_tick(
        &mut self,
        tick: u64,
        pathing_enabled: bool,
        pathing_results: &BTreeMap<EntityId, GoalPathingResult>,
        active_ids: Option<&HashSet<EntityId>>,
        external_follow_targets: Option<&HashMap<EntityId, Vec3>>,
    ) -> GoalTickStats {
        let mut stats = GoalTickStats::default();

        self.runtime.queue_goal_tick(
            tick,
            pathing_enabled,
            pathing_results.values().cloned(),
            active_ids,
            external_follow_targets,
        );
        self.runtime.run_stage(EntityStage::InputAi);
        let authoritative_stats = self.runtime.take_goal_tick_stats();
        stats.alive_entities += authoritative_stats.alive_entities;
        stats.decisions_applied += authoritative_stats.decisions_applied;
        stats.skipped_non_alive += authoritative_stats.skipped_non_alive;
        stats.missing_follow_targets += authoritative_stats.missing_follow_targets;
        stats.pathing_moves += authoritative_stats.pathing_moves;
        stats.pathing_blocked += authoritative_stats.pathing_blocked;
        stats.pathing_unloaded += authoritative_stats.pathing_unloaded;
        stats
    }

    pub fn tick_positions(&mut self, delta_seconds: f64) {
        {
            self.runtime.queue_position_tick(delta_seconds);
            self.runtime.run_stage(EntityStage::PhysicsApply);
        }
    }

    pub fn tick_positions_in_range(&mut self, range: Range<usize>, delta_seconds: f64) {
        {
            self.runtime
                .queue_position_tick_in_range(range, delta_seconds);
            self.runtime.run_stage(EntityStage::PhysicsApply);
        }
    }

    fn allocate_id(&mut self) -> EntityId {
        let mut next_id = self.next_id.max(0);
        loop {
            next_id = next_id
                .checked_add(1)
                .expect("entity runtime id space exhausted");

            let id = EntityId(next_id);
            if !self.contains(id) {
                self.next_id = next_id;
                return id;
            }
        }
    }
}

fn resolve_retained_pathing(
    request: &GoalPathingRequest,
    probe: &dyn PathingProbe,
    budget: PathingBudget,
) -> (PathingDecision, RetainedPathState) {
    let mut path = request.expected_path;
    let current = request.expected_position;
    let mut probes = BudgetedPathingProbe::new(probe, budget.max_candidates_per_entity);
    let mut no_progress_budget_exhausted = false;
    let target_changed = !path.has_target
        || path.target != request.target
        || path.target_epoch != request.target_epoch;
    if target_changed {
        path.target = request.target;
        path.target_epoch = request.target_epoch;
        path.has_target = true;
        path.target_revision = path.target_revision.saturating_add(1);
        path.clear_nodes();
        path.no_progress_ticks = 0;
        path.recomputations = 0;
        path.was_moving = false;
        path.stopped = false;
    } else if path.has_last_position {
        let progress = (current.x - path.last_position.x).hypot(current.z - path.last_position.z);
        if progress > RETAINED_PATH_PROGRESS_EPSILON {
            path.no_progress_ticks = 0;
            path.recomputations = 0;
            path.stopped = false;
        } else if path.was_moving {
            path.no_progress_ticks = path.no_progress_ticks.saturating_add(1);
            if path.no_progress_ticks >= RETAINED_PATH_NO_PROGRESS_LIMIT {
                path.no_progress_ticks = 0;
                path.recomputations = path.recomputations.saturating_add(1);
                path.clear_nodes();
                if path.recomputations >= RETAINED_PATH_RECOMPUTE_LIMIT {
                    path.stopped = true;
                    no_progress_budget_exhausted = true;
                }
            }
        }
    }
    path.last_position = current;
    path.has_last_position = true;

    if no_progress_budget_exhausted {
        path.stopped = false;
        path.recomputations = 0;
        path.was_moving = false;
        return (
            PathingDecision {
                velocity: Vec3::ZERO,
                kind: PathingDecisionKind::Blocked,
                direct: false,
            },
            path,
        );
    }
    if path.stopped {
        path.stopped = false;
        path.recomputations = 0;
        path.clear_nodes();
    }

    let reach = (request.speed * PathingBudget::TICK_SECONDS).max(0.01) * 1.25;
    while let Some(node) = path.current_target() {
        if (node.x - current.x).hypot(node.z - current.z) > reach {
            break;
        }
        path.current_node = path.current_node.saturating_add(1);
    }
    if path.current_target().is_none()
        && (request.target.x - current.x).hypot(request.target.z - current.z) <= reach
    {
        probes.direct_path_resolved(request.id);
        path.was_moving = false;
        return (
            PathingDecision {
                velocity: Vec3::ZERO,
                kind: PathingDecisionKind::Move,
                direct: true,
            },
            path,
        );
    }

    let following_detour = path.current_node.saturating_add(1) < path.node_count;
    let navigation_target = path.current_target().unwrap_or(request.target);
    let mut decision = bounded_pathing_step(
        request.id,
        current,
        navigation_target,
        request.speed,
        budget,
        &mut probes,
        !following_detour,
    );

    if following_detour && !decision.direct && decision.kind != PathingDecisionKind::Unloaded {
        path.recomputations = path.recomputations.saturating_add(1);
        if path.recomputations >= RETAINED_PATH_RECOMPUTE_LIMIT {
            path.stopped = true;
            path.was_moving = false;
            return (
                PathingDecision {
                    velocity: Vec3::ZERO,
                    kind: PathingDecisionKind::Blocked,
                    direct: false,
                },
                path,
            );
        }
        path.clear_nodes();
        decision = bounded_pathing_step(
            request.id,
            current,
            request.target,
            request.speed,
            budget,
            &mut probes,
            true,
        );
    }

    match decision.kind {
        PathingDecisionKind::Move => {
            path.was_moving = decision.velocity != Vec3::ZERO;
            if following_detour && decision.direct {
                // Keep following the retained detour until its node is reached.
            } else if decision.direct {
                path.retain_direct_target();
            } else {
                path.retain_detour(current, decision.velocity);
            }
        }
        PathingDecisionKind::Blocked => {
            path.was_moving = false;
            path.recomputations = path.recomputations.saturating_add(1);
            path.clear_nodes();
            if path.recomputations >= RETAINED_PATH_RECOMPUTE_LIMIT {
                path.stopped = true;
            }
        }
        PathingDecisionKind::Unloaded => {
            path.was_moving = false;
        }
    }
    (decision, path)
}

fn bounded_pathing_step(
    entity_id: EntityId,
    current: Vec3,
    target: Vec3,
    speed: f64,
    budget: PathingBudget,
    probes: &mut BudgetedPathingProbe<'_>,
    allow_detours: bool,
) -> PathingDecision {
    if speed <= 0.0 {
        return PathingDecision {
            velocity: Vec3::ZERO,
            kind: PathingDecisionKind::Blocked,
            direct: false,
        };
    }
    let goal = Vec3 {
        x: target.x - current.x,
        y: 0.0,
        z: target.z - current.z,
    };
    let direct = goal.horizontal_normalized();
    if direct == Vec3::ZERO {
        probes.direct_path_resolved(entity_id);
        return PathingDecision {
            velocity: Vec3::ZERO,
            kind: PathingDecisionKind::Move,
            direct: true,
        };
    }

    let (candidates, limit) = bounded_pathing_candidates(current, direct, speed, budget);
    let mut saw_unloaded = false;
    let mut best: Option<(f64, Vec3)> = None;
    let candidate_limit = if allow_detours { limit } else { limit.min(1) };
    for (candidate_index, candidate) in candidates.into_iter().take(candidate_limit).enumerate() {
        let flat = candidate.position;
        let Some(flat_result) = probes.call(entity_id, flat) else {
            break;
        };
        let accepted = match flat_result {
            PathingProbeResult::Walkable => Some(Vec3 {
                x: candidate.direction.x,
                y: 0.0,
                z: candidate.direction.z,
            }),
            PathingProbeResult::Blocked => {
                let stepped = Vec3 {
                    y: flat.y + budget.step_height.max(0.0),
                    ..flat
                };
                let Some(stepped_result) = probes.call(entity_id, stepped) else {
                    break;
                };
                match stepped_result {
                    PathingProbeResult::Walkable => Some(Vec3 {
                        x: candidate.direction.x,
                        y: budget.step_height.max(0.0),
                        z: candidate.direction.z,
                    }),
                    PathingProbeResult::Unloaded => {
                        saw_unloaded = true;
                        None
                    }
                    PathingProbeResult::Blocked => None,
                }
            }
            PathingProbeResult::Unloaded => {
                saw_unloaded = true;
                None
            }
        };
        let Some(velocity) = accepted else {
            continue;
        };
        if candidate_index == 0 {
            probes.direct_path_resolved(entity_id);
            return PathingDecision {
                velocity,
                kind: PathingDecisionKind::Move,
                direct: true,
            };
        }
        let next = Vec3 {
            x: current.x + velocity.x * speed * PathingBudget::TICK_SECONDS,
            y: current.y,
            z: current.z + velocity.z * speed * PathingBudget::TICK_SECONDS,
        };
        let score = (target.x - next.x).hypot(target.z - next.z);
        if best.is_none_or(|(best_score, _)| score < best_score) {
            best = Some((score, velocity));
        }
    }

    if let Some((_, velocity)) = best {
        PathingDecision {
            velocity,
            kind: PathingDecisionKind::Move,
            direct: false,
        }
    } else {
        PathingDecision {
            velocity: Vec3::ZERO,
            kind: if saw_unloaded {
                PathingDecisionKind::Unloaded
            } else {
                PathingDecisionKind::Blocked
            },
            direct: false,
        }
    }
}

struct BudgetedPathingProbe<'a> {
    probe: &'a dyn PathingProbe,
    remaining: usize,
}

impl<'a> BudgetedPathingProbe<'a> {
    fn new(probe: &'a dyn PathingProbe, limit: usize) -> Self {
        Self {
            probe,
            remaining: limit,
        }
    }

    fn call(&mut self, entity_id: EntityId, position: Vec3) -> Option<PathingProbeResult> {
        self.remaining = self.remaining.checked_sub(1)?;
        Some(self.probe.can_entity_stand_at(entity_id, position))
    }

    fn direct_path_resolved(&self, entity_id: EntityId) {
        self.probe.direct_path_resolved(entity_id);
    }
}

#[derive(Debug, Clone, Copy)]
struct PathingProbeCandidate {
    direction: Vec3,
    position: Vec3,
}

fn visit_bounded_pathing_probe_positions(
    current: Vec3,
    target: Vec3,
    speed: f64,
    budget: PathingBudget,
    mut visitor: impl FnMut(Vec3),
) {
    let direct = Vec3 {
        x: target.x - current.x,
        y: 0.0,
        z: target.z - current.z,
    }
    .horizontal_normalized();
    if direct == Vec3::ZERO {
        return;
    }
    let (candidates, limit) = bounded_pathing_candidates(current, direct, speed, budget);
    for candidate in candidates.into_iter().take(limit) {
        visitor(candidate.position);
        let stepped = Vec3 {
            y: candidate.position.y + budget.step_height.max(0.0),
            ..candidate.position
        };
        if stepped != candidate.position {
            visitor(stepped);
        }
    }
}

fn bounded_pathing_candidates(
    current: Vec3,
    direct: Vec3,
    speed: f64,
    budget: PathingBudget,
) -> ([PathingProbeCandidate; 8], usize) {
    let lookahead = speed * PathingBudget::TICK_SECONDS;
    let candidates = pathing_candidates(direct).map(|direction| PathingProbeCandidate {
        direction,
        position: Vec3 {
            x: current.x + direction.x * lookahead,
            y: current.y,
            z: current.z + direction.z * lookahead,
        },
    });
    let limit = budget.max_candidates_per_entity.min(candidates.len());
    (candidates, limit)
}

fn pathing_candidates(direct: Vec3) -> [Vec3; 8] {
    let side = Vec3 {
        x: -direct.z,
        y: 0.0,
        z: direct.x,
    };
    [
        direct,
        Vec3 {
            x: direct.x + side.x,
            y: 0.0,
            z: direct.z + side.z,
        }
        .horizontal_normalized(),
        Vec3 {
            x: direct.x - side.x,
            y: 0.0,
            z: direct.z - side.z,
        }
        .horizontal_normalized(),
        Vec3 {
            x: -direct.x,
            y: 0.0,
            z: -direct.z,
        },
        side,
        Vec3 {
            x: -side.x,
            y: 0.0,
            z: -side.z,
        },
        Vec3 {
            x: direct.x + 0.5 * side.x,
            y: 0.0,
            z: direct.z + 0.5 * side.z,
        }
        .horizontal_normalized(),
        Vec3 {
            x: direct.x - 0.5 * side.x,
            y: 0.0,
            z: direct.z - 0.5 * side.z,
        }
        .horizontal_normalized(),
    ]
}

fn snapshot_from_spawn(id: EntityId, uuid: Uuid, entity: SpawnEntity) -> EntitySnapshot {
    let health = entity
        .attributes
        .base(&AttributeKind::MaxHealth)
        .unwrap_or(20.0)
        .max(1.0) as f32;
    EntitySnapshot {
        id,
        uuid,
        type_id: entity.type_id,
        type_name: entity.type_name,
        position: entity.position,
        rotation: entity.rotation,
        velocity: entity.velocity,
        on_ground: entity.on_ground,
        item_stack: entity.item_stack,
        experience_value: entity.experience_value,
        block_state: entity.block_state,
        lifecycle: EntityLifecycle::Alive,
        health,
        attributes: entity.attributes,
        goal: entity.goal,
        vehicle: entity.vehicle,
        animal: entity.animal,
        retained: entity.retained,
    }
}

fn deterministic_uuid(id: EntityId) -> Uuid {
    let low = id.0 as u32 as u128;
    Uuid::from_u128(0x5f1a_0000_0000_0000_0000_0000_0000_0000 | low)
}

fn deterministic_angle(id: EntityId, phase: u64) -> f64 {
    let mixed = splitmix64((id.0 as u32 as u64) ^ phase.wrapping_mul(0x9e37_79b9_7f4a_7c15));
    (mixed as f64 / u64::MAX as f64) * TAU
}

fn wander_pathing_target(
    id: EntityId,
    position: Vec3,
    path: RetainedPathState,
    tick: u64,
    period_ticks: u32,
) -> (Vec3, u64) {
    let period = u64::from(period_ticks.max(1));
    let epoch = tick / period;
    if path.has_target && path.target_epoch == Some(epoch) {
        return (path.target, epoch);
    }
    let angle = deterministic_angle(id, epoch);
    (
        Vec3 {
            x: position.x + angle.cos(),
            y: position.y,
            z: position.z + angle.sin(),
        },
        epoch,
    )
}

fn deterministic_wave(id: EntityId, phase: u64) -> f64 {
    deterministic_angle(id, phase.wrapping_add(0x41)).sin()
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn yaw_from_velocity(velocity: Vec3) -> f32 {
    if velocity.x == 0.0 && velocity.z == 0.0 {
        0.0
    } else {
        velocity.z.atan2(velocity.x).to_degrees() as f32 - 90.0
    }
}

fn aquatic_rotation_from_velocity(velocity: Vec3) -> Rotation {
    let yaw = yaw_from_velocity(velocity) + 90.0;
    let horizontal = velocity.horizontal_len();
    let pitch = if horizontal <= f64::EPSILON && velocity.y == 0.0 {
        0.0
    } else {
        (-velocity.y).atan2(horizontal).to_degrees() as f32
    };
    Rotation {
        yaw,
        pitch: pitch.clamp(-35.0, 35.0),
        head_yaw: yaw,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    use super::*;

    #[derive(Debug)]
    struct TestPathingProbe {
        default: PathingProbeResult,
        overrides: HashMap<(i64, i64, i64), PathingProbeResult>,
    }

    impl TestPathingProbe {
        fn new(default: PathingProbeResult) -> Self {
            Self {
                default,
                overrides: HashMap::new(),
            }
        }

        fn with(mut self, x: f64, y: f64, z: f64, result: PathingProbeResult) -> Self {
            self.overrides
                .insert(pathing_test_key(Vec3::new(x, y, z)), result);
            self
        }
    }

    impl PathingProbe for TestPathingProbe {
        fn can_stand_at(&self, position: Vec3) -> PathingProbeResult {
            let key = pathing_test_key(position);
            self.overrides.get(&key).copied().unwrap_or(self.default)
        }
    }

    fn pathing_test_key(position: Vec3) -> (i64, i64, i64) {
        let quantize = |value: f64| (value * 1_000_000.0).round() as i64;
        (
            quantize(position.x),
            quantize(position.y),
            quantize(position.z),
        )
    }

    fn cow(position: Vec3) -> SpawnEntity {
        SpawnEntity::new(144, "minecraft:cow", position)
    }

    fn vehicle_snapshot(
        id: EntityId,
        lifecycle: EntityLifecycle,
        passenger: Option<EntityId>,
    ) -> EntitySnapshot {
        EntitySnapshot {
            id,
            uuid: deterministic_uuid(id),
            type_id: 15,
            type_name: "minecraft:oak_boat".to_owned(),
            position: Vec3::new(0.0, 63.0, 0.0),
            rotation: Rotation::ZERO,
            velocity: Vec3::ZERO,
            on_ground: true,
            item_stack: None,
            experience_value: None,
            block_state: None,
            lifecycle,
            health: if lifecycle == EntityLifecycle::Alive {
                20.0
            } else {
                0.0
            },
            attributes: AttributeSet::new(),
            goal: GoalState::Idle,
            vehicle: Some(VehicleState {
                kind: VehicleKind::Boat,
                passenger,
            }),
            animal: None,
            retained: EntityRetainedState::default(),
        }
    }

    #[test]
    fn spawn_assigns_stable_dense_ids() {
        let mut store = EntityStore::new();
        let a = store.spawn(cow(Vec3::new(1.0, 64.0, 1.0)));
        let b = store.spawn(cow(Vec3::new(2.0, 64.0, 2.0)));

        assert_eq!(a, EntityId(1));
        assert_eq!(b, EntityId(2));
        assert_eq!(store.len(), 2);
        assert!(store.contains(a));
        assert_eq!(
            store.snapshot(b).unwrap().position,
            Vec3::new(2.0, 64.0, 2.0)
        );
    }

    #[test]
    fn insert_snapshot_rejects_duplicate_uuid_without_partial_insert() {
        let mut store = EntityStore::new();
        let id = store.spawn(cow(Vec3::new(1.0, 64.0, 1.0)));
        let original = store.snapshot(id).unwrap();
        let mut duplicate = original.clone();
        duplicate.id = EntityId(99);
        duplicate.position = Vec3::new(9.0, 64.0, 9.0);

        assert!(!store.insert_snapshot(duplicate));
        assert_eq!(store.len(), 1);
        assert_eq!(store.snapshot(id), Some(original));
        assert_eq!(store.snapshot(EntityId(99)), None);
    }

    #[test]
    fn non_finite_kinematics_are_rejected_without_mutation() {
        let mut store = EntityStore::new();
        let id = store.spawn(cow(Vec3::new(1.0, 64.0, 1.0)));
        let original = store.snapshot(id).unwrap();

        assert!(!store.set_position(id, Vec3::new(f64::NAN, 64.0, 1.0)));
        assert!(!store.set_velocity(id, Vec3::new(0.0, f64::INFINITY, 0.0)));
        assert_eq!(
            store.apply_kinematics([EntityKinematics {
                id,
                position: Vec3::new(2.0, 64.0, 2.0),
                rotation: Rotation {
                    yaw: f32::NAN,
                    pitch: 0.0,
                    head_yaw: 0.0,
                },
                velocity: Vec3::ZERO,
                on_ground: true,
            }]),
            0
        );
        assert_eq!(store.snapshot(id), Some(original));
    }

    #[test]
    #[should_panic(expected = "entity runtime id space exhausted")]
    fn allocate_id_panics_on_runtime_id_overflow() {
        let mut store = EntityStore::with_next_id(i32::MAX);
        let _ = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    }

    #[test]
    fn allocate_id_normalizes_negative_seed() {
        let mut store = EntityStore::with_next_id(-5);
        let id = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));

        assert_eq!(id, EntityId(1));
    }

    #[test]
    fn item_stack_payload_round_trips_through_snapshots() {
        let mut store = EntityStore::new();
        let mut item = SpawnEntity::new(1, "minecraft:item", Vec3::new(0.5, 64.5, 0.5));
        item.item_stack = Some(EntityItemStack::new(42, 3));

        let id = store.spawn(item);

        assert_eq!(
            store.snapshot(id).and_then(|snapshot| snapshot.item_stack),
            Some(EntityItemStack::new(42, 3))
        );
        assert!(store.set_item_stack(id, Some(EntityItemStack::new(42, 1))));
        assert_eq!(
            store.snapshot(id).and_then(|snapshot| snapshot.item_stack),
            Some(EntityItemStack::new(42, 1))
        );
    }

    #[test]
    fn ordinary_store_routes_every_family_to_ecs_runtime() {
        let mut store = EntityStore::new();
        let mut item = SpawnEntity::new(1, "minecraft:item", Vec3::new(0.5, 64.5, 0.5));
        item.item_stack = Some(EntityItemStack::new(42, 3));
        let item_id = store.spawn(item);
        let mut xp = SpawnEntity::new(2, "minecraft:experience_orb", Vec3::new(1.5, 64.5, 0.5));
        xp.experience_value = Some(7);
        let xp_id = store.spawn(xp);
        let cow_id = store.spawn(cow(Vec3::new(2.5, 64.0, 0.5)));

        assert_eq!(store.len(), 3);
        assert!(store.contains(item_id));
        assert!(store.contains(xp_id));
        assert!(store.contains(cow_id));
        assert!(store.motion_state(item_id).unwrap().is_item);
        assert!(!store.motion_state(item_id).unwrap().is_experience);
        assert!(store.motion_state(xp_id).unwrap().is_experience);
        assert!(!store.motion_state(xp_id).unwrap().is_item);
        assert_eq!(
            store.snapshot(item_id).unwrap().item_stack,
            Some(EntityItemStack::new(42, 3))
        );
        assert_eq!(store.snapshot(xp_id).unwrap().experience_value, Some(7));
        assert_eq!(
            store.apply_kinematics([EntityKinematics {
                id: item_id,
                position: Vec3::new(0.75, 64.5, 0.5),
                rotation: Rotation::ZERO,
                velocity: Vec3::ZERO,
                on_ground: true,
            }]),
            1
        );
        assert_eq!(
            store.snapshot(item_id).unwrap().position,
            Vec3::new(0.75, 64.5, 0.5)
        );
        assert_eq!(store.snapshots().count(), 3);
    }

    #[test]
    fn simulation_views_enumerate_sole_ecs_authority_once() {
        let mut store = EntityStore::new();
        let first_id = store.spawn(cow(Vec3::new(1.0, 64.0, 1.0)));
        let second_id = store.spawn(cow(Vec3::new(2.0, 64.0, 2.0)));

        let mut seen = Vec::new();
        store.visit_simulation_entities(|entity| {
            seen.push((entity.id, entity.type_name.to_owned(), entity.position));
        });
        seen.sort_unstable_by_key(|entity| entity.0);

        assert_eq!(
            seen,
            vec![
                (
                    first_id,
                    "minecraft:cow".to_owned(),
                    Vec3::new(1.0, 64.0, 1.0)
                ),
                (
                    second_id,
                    "minecraft:cow".to_owned(),
                    Vec3::new(2.0, 64.0, 2.0)
                ),
            ]
        );
    }

    #[test]
    fn simulation_views_for_ids_only_visit_requested_entities_in_id_order() {
        let mut store = EntityStore::new();
        let first_id = store.spawn(cow(Vec3::new(1.0, 64.0, 1.0)));
        let skipped_id = store.spawn(cow(Vec3::new(2.0, 64.0, 2.0)));
        let last_id = store.spawn(cow(Vec3::new(3.0, 64.0, 3.0)));

        let mut seen = Vec::new();
        store.visit_simulation_entities_for_ids(
            &HashSet::from([last_id, EntityId(99_999), first_id]),
            |entity| seen.push(entity.id),
        );

        assert_eq!(seen, vec![first_id, last_id]);
        assert!(!seen.contains(&skipped_id));
    }

    #[test]
    fn alive_kinematics_for_ids_skips_despawning_entities() {
        let mut store = EntityStore::new();
        let first_id = store.spawn(cow(Vec3::new(1.0, 64.0, 1.0)));
        let second_id = store.spawn(cow(Vec3::new(2.0, 64.0, 2.0)));
        let despawning_id = store.spawn(cow(Vec3::new(3.0, 64.0, 3.0)));
        assert!(store.mark_despawning(despawning_id));

        let mut states =
            store.alive_kinematics_for_ids(&HashSet::from([first_id, second_id, despawning_id]));
        states.sort_unstable_by_key(|state| state.id);

        assert_eq!(
            states,
            vec![
                EntityKinematics {
                    id: first_id,
                    position: Vec3::new(1.0, 64.0, 1.0),
                    rotation: Rotation::ZERO,
                    velocity: Vec3::ZERO,
                    on_ground: true,
                },
                EntityKinematics {
                    id: second_id,
                    position: Vec3::new(2.0, 64.0, 2.0),
                    rotation: Rotation::ZERO,
                    velocity: Vec3::ZERO,
                    on_ground: true,
                },
            ]
        );
    }

    #[test]
    fn projectile_and_falling_block_families_round_trip_through_ecs_runtime() {
        let mut store = EntityStore::new();
        let arrow_id = store.spawn(SpawnEntity::new(
            1,
            "minecraft:arrow",
            Vec3::new(0.5, 66.0, 0.5),
        ));
        let mut falling = SpawnEntity::new(2, "minecraft:falling_block", Vec3::new(1.5, 70.0, 0.5));
        falling.block_state = Some(91);
        let falling_id = store.spawn(falling);
        store.spawn(cow(Vec3::new(2.5, 64.0, 0.5)));

        assert_eq!(store.len(), 3);
        assert_eq!(
            store.snapshot(arrow_id).unwrap().type_name,
            "minecraft:arrow"
        );
        assert_eq!(store.snapshot(falling_id).unwrap().block_state, Some(91));
    }

    #[test]
    fn ordinary_passive_mob_runs_goal_tick_in_ecs() {
        let mut store = EntityStore::new();
        let mut entity = cow(Vec3::new(2.5, 64.0, 0.5));
        entity.goal = GoalState::Wander {
            speed: 0.2,
            period_ticks: 20,
        };
        let id = store.spawn(entity);

        let stats = store.tick_goals_with_stats(20);

        assert_eq!(stats.alive_entities, 1);
        assert_eq!(stats.decisions_applied, 1);
        assert_ne!(store.snapshot(id).unwrap().velocity, Vec3::ZERO);

        let snapshot = store.snapshot(id).unwrap();
        let mut restored = EntityStore::new();
        assert!(restored.insert_snapshot(snapshot.clone()));
        assert_eq!(restored.snapshot(id), Some(snapshot));
    }

    #[test]
    fn batch_spawn_inserts_all_entities_into_ecs_runtime() {
        let mut store = EntityStore::new();

        let ids = store.spawn_batch([
            cow(Vec3::new(0.5, 64.0, 0.5)),
            cow(Vec3::new(1.5, 64.0, 0.5)),
            cow(Vec3::new(2.5, 64.0, 0.5)),
        ]);

        assert_eq!(ids, vec![EntityId(1), EntityId(2), EntityId(3)]);
        assert_eq!(store.len(), 3);
        for id in ids {
            assert_eq!(store.snapshot(id).unwrap().type_name, "minecraft:cow");
        }
    }

    #[test]
    fn damage_reduces_health_and_marks_killed_entities() {
        let mut store = EntityStore::new();
        let id = store.spawn(cow(Vec3::new(1.0, 64.0, 1.0)));
        let initial = store.snapshot(id).unwrap();

        for invalid in [
            EntityDamageRequest {
                amount: 0.0,
                tick: 1,
                death_remove_tick: 21,
            },
            EntityDamageRequest {
                amount: -1.0,
                tick: 1,
                death_remove_tick: 21,
            },
            EntityDamageRequest {
                amount: f32::NAN,
                tick: 1,
                death_remove_tick: 21,
            },
            EntityDamageRequest {
                amount: 1.0,
                tick: 21,
                death_remove_tick: 20,
            },
        ] {
            assert!(store.damage(id, invalid).is_none());
            assert_eq!(store.snapshot(id), Some(initial.clone()));
        }

        let hit = store
            .damage(
                id,
                EntityDamageRequest {
                    amount: 5.0,
                    tick: 1,
                    death_remove_tick: 21,
                },
            )
            .unwrap();
        assert!(!hit.killed);
        assert_eq!(hit.snapshot.health, 15.0);
        assert_eq!(hit.snapshot.lifecycle, EntityLifecycle::Alive);
        assert_eq!(hit.snapshot.retained.last_damage_tick, Some(1));
        assert_eq!(hit.snapshot.retained.death_remove_tick, None);

        let lethal = store
            .damage(
                id,
                EntityDamageRequest {
                    amount: 20.0,
                    tick: 2,
                    death_remove_tick: 22,
                },
            )
            .unwrap();
        assert!(lethal.killed);
        assert_eq!(lethal.snapshot.health, 0.0);
        assert_eq!(lethal.snapshot.lifecycle, EntityLifecycle::Despawning);
        assert_eq!(lethal.snapshot.retained.last_damage_tick, Some(2));
        assert_eq!(lethal.snapshot.retained.death_remove_tick, Some(22));
    }

    #[test]
    fn remove_keeps_remaining_entity_addressable() {
        let mut store = EntityStore::new();
        let a = store.spawn(cow(Vec3::new(1.0, 64.0, 1.0)));
        let b = store.spawn(cow(Vec3::new(2.0, 64.0, 2.0)));
        let c = store.spawn(cow(Vec3::new(3.0, 64.0, 3.0)));

        let removed = store.remove(b).unwrap();

        assert_eq!(removed.id, b);
        assert!(!store.contains(b));
        assert!(store.contains(a));
        assert!(store.contains(c));
        assert_eq!(store.len(), 2);
        assert_eq!(
            store.snapshot(c).unwrap().position,
            Vec3::new(3.0, 64.0, 3.0)
        );
    }

    #[test]
    fn attributes_expose_vanilla_names_and_base_values() {
        let mut attrs = AttributeSet::vanilla_mob_defaults();
        attrs.set_base(AttributeKind::AttackDamage, 3.0);

        assert_eq!(
            AttributeKind::MaxHealth.vanilla_name(),
            "minecraft:max_health"
        );
        assert_eq!(attrs.base(&AttributeKind::MaxHealth), Some(20.0));
        assert_eq!(attrs.base(&AttributeKind::AttackDamage), Some(3.0));
        assert_eq!(attrs.iter().count(), 4);
    }

    #[test]
    fn position_ticks_can_be_split_into_ranges() {
        let mut store = EntityStore::new();
        let a = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        let b = store.spawn(cow(Vec3::new(10.0, 64.0, 10.0)));
        store.set_velocity(a, Vec3::new(1.0, 0.0, 0.0));
        store.set_velocity(b, Vec3::new(0.0, 0.0, 1.0));

        assert_eq!(store.batch_ranges(1), vec![0..1, 1..2]);
        store.tick_positions_in_range(0..1, 0.5);

        assert_eq!(
            store.snapshot(a).unwrap().position,
            Vec3::new(0.5, 64.0, 0.0)
        );
        assert_eq!(
            store.snapshot(b).unwrap().position,
            Vec3::new(10.0, 64.0, 10.0)
        );
    }

    #[test]
    fn wander_goal_is_deterministic() {
        let mut a = EntityStore::new();
        let mut b = EntityStore::new();
        let entity_a = a.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        let entity_b = b.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        a.set_goal(
            entity_a,
            GoalState::Wander {
                speed: 0.2,
                period_ticks: 20,
            },
        );
        b.set_goal(
            entity_b,
            GoalState::Wander {
                speed: 0.2,
                period_ticks: 20,
            },
        );

        a.tick_goals(40);
        b.tick_goals(40);

        assert_eq!(
            a.snapshot(entity_a).unwrap().velocity,
            b.snapshot(entity_b).unwrap().velocity
        );
    }

    #[test]
    fn authoritative_goal_batch_runs_one_ecs_input_stage() {
        let mut store = EntityStore::new();
        let first = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        let second = store.spawn(cow(Vec3::new(2.0, 64.0, 0.0)));
        let before = store.input_ai_stage_runs_for_test();

        let goals = [
            (
                first,
                GoalState::Wander {
                    speed: 0.2,
                    period_ticks: 20,
                },
            ),
            (
                second,
                GoalState::FollowPosition {
                    target: Vec3::new(8.0, 64.0, 0.0),
                    speed: 0.3,
                },
            ),
        ];

        assert_eq!(store.set_goals(goals.clone()), 2);

        assert_eq!(store.input_ai_stage_runs_for_test() - before, 1);
        assert!(matches!(
            store.snapshot(first).unwrap().goal,
            GoalState::Wander { .. }
        ));
        assert!(matches!(
            store.snapshot(second).unwrap().goal,
            GoalState::FollowPosition { .. }
        ));

        let after_change = store.input_ai_stage_runs_for_test();
        assert_eq!(store.set_goals(goals), 2);
        assert_eq!(store.input_ai_stage_runs_for_test(), after_change);
    }

    #[test]
    fn aquatic_wander_sets_3d_motion_and_pitch() {
        let mut store = EntityStore::new();
        let mut fish = SpawnEntity::new(2, "minecraft:cod", Vec3::new(0.0, 50.0, 0.0));
        fish.goal = GoalState::AquaticWander {
            speed: 0.2,
            vertical_speed: 0.1,
            period_ticks: 20,
        };
        let id = store.spawn(fish);

        store.tick_goals(40);
        let snapshot = store.snapshot(id).unwrap();

        assert!(snapshot.velocity.horizontal_len() > 0.0);
        assert!(snapshot.velocity.y.abs() > 0.0);
        assert!(!snapshot.on_ground);
        assert_ne!(snapshot.rotation.pitch, 0.0);
        assert_eq!(snapshot.rotation.yaw, snapshot.rotation.head_yaw);
    }

    #[test]
    fn follow_target_sets_horizontal_velocity() {
        let mut store = EntityStore::new();
        let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        let target = store.spawn(cow(Vec3::new(3.0, 64.0, 4.0)));
        store.set_goal(follower, GoalState::FollowTarget { target, speed: 0.5 });

        store.tick_goals(1);

        let velocity = store.snapshot(follower).unwrap().velocity;
        assert!((velocity.x - 0.3).abs() < 0.000_001);
        assert!((velocity.z - 0.4).abs() < 0.000_001);
    }

    #[test]
    fn goal_tick_stats_report_applied_and_skipped_ai_decisions() {
        let mut store = EntityStore::new();
        let idle = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        let follower = store.spawn(cow(Vec3::new(2.0, 64.0, 0.0)));
        let despawning = store.spawn(cow(Vec3::new(4.0, 64.0, 0.0)));
        store.set_velocity(idle, Vec3::new(1.0, 2.0, 3.0));
        store.set_goal(
            follower,
            GoalState::FollowTarget {
                target: EntityId(99_999),
                speed: 0.5,
            },
        );
        store.mark_despawning(despawning);

        let stats = store.tick_goals_with_stats(1);

        assert_eq!(
            stats,
            GoalTickStats {
                alive_entities: 2,
                decisions_applied: 2,
                skipped_non_alive: 1,
                missing_follow_targets: 1,
                pathing_moves: 0,
                pathing_blocked: 0,
                pathing_unloaded: 0,
            }
        );
        assert_eq!(
            store.snapshot(idle).unwrap().velocity,
            Vec3::new(0.0, 2.0, 0.0)
        );
        assert_eq!(store.snapshot(follower).unwrap().velocity, Vec3::ZERO);
        assert_eq!(
            store.snapshot(despawning).unwrap().lifecycle,
            EntityLifecycle::Despawning
        );
    }

    #[test]
    fn follow_position_sets_horizontal_velocity() {
        let mut store = EntityStore::new();
        let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        store.set_velocity(follower, Vec3::new(0.0, -0.25, 0.0));
        store.set_goal(
            follower,
            GoalState::FollowPosition {
                target: Vec3::new(0.0, 65.0, 4.0),
                speed: 0.5,
            },
        );

        store.tick_goals(1);

        let velocity = store.snapshot(follower).unwrap().velocity;
        assert_eq!(velocity.x, 0.0);
        assert_eq!(velocity.y, -0.25);
        assert!((velocity.z - 0.5).abs() < 0.000_001);
    }

    #[test]
    fn boat_vehicle_mount_steer_and_dismount_updates_runtime_store() {
        let mut store = EntityStore::new();
        let passenger = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        let boat = store.spawn(SpawnEntity::vehicle(
            VehicleKind::Boat,
            15,
            "minecraft:oak_boat",
            Vec3::new(10.0, 63.0, 10.0),
        ));

        store.mount_vehicle(boat, passenger).unwrap();
        assert_eq!(store.vehicle_for_passenger(passenger), Some(boat));

        store
            .apply_vehicle_input(
                boat,
                passenger,
                VehicleInput {
                    right: true,
                    forward: true,
                    ..VehicleInput::default()
                },
            )
            .unwrap();
        let steered = store.snapshot(boat).unwrap();
        assert_eq!(steered.vehicle.unwrap().passenger, Some(passenger));
        assert_eq!(steered.rotation.yaw, 4.0);
        assert_eq!(steered.rotation.head_yaw, 4.0);
        assert!(steered.velocity.horizontal_len() > 0.0);

        store.tick_positions(1.0);
        let moved = store.snapshot(boat).unwrap();
        assert_ne!(moved.position, Vec3::new(10.0, 63.0, 10.0));

        store.dismount_vehicle(boat, passenger).unwrap();
        assert_eq!(store.vehicle_for_passenger(passenger), None);
        assert_eq!(
            store.snapshot(boat).unwrap().vehicle.unwrap().passenger,
            None
        );
    }

    #[test]
    fn authoritative_vehicle_mount_steer_and_dismount_updates_ecs() {
        let mut store = EntityStore::new();
        let passenger = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        let boat = store.spawn(SpawnEntity::vehicle(
            VehicleKind::Boat,
            15,
            "minecraft:oak_boat",
            Vec3::new(10.0, 63.0, 10.0),
        ));

        store.mount_vehicle(boat, passenger).unwrap();
        store
            .apply_vehicle_input(boat, passenger, VehicleInput::forward())
            .unwrap();
        assert!(store.snapshot(boat).unwrap().velocity.horizontal_len() > 0.0);

        store.dismount_vehicle(boat, passenger).unwrap();
        assert_eq!(store.vehicle_for_passenger(passenger), None);
    }

    #[test]
    fn authoritative_animal_breeding_state_round_trips_through_ecs_snapshot() {
        let mut store = EntityStore::new();
        let mut entity = cow(Vec3::new(2.0, 64.0, 3.0));
        entity.animal = Some(AnimalBreedingState {
            age_ticks: BABY_START_AGE_TICKS,
            love_ticks: 0,
            sheep_wool: None,
        });

        let id = store.spawn(entity);
        assert_eq!(
            store.snapshot(id).unwrap().animal,
            Some(AnimalBreedingState {
                age_ticks: BABY_START_AGE_TICKS,
                love_ticks: 0,
                sheep_wool: None,
            })
        );

        assert!(store.set_animal_state(
            id,
            AnimalBreedingState {
                age_ticks: 0,
                love_ticks: ANIMAL_LOVE_DURATION_TICKS,
                sheep_wool: None,
            },
        ));
        assert_eq!(
            store.snapshot(id).unwrap().animal,
            Some(AnimalBreedingState {
                age_ticks: 0,
                love_ticks: ANIMAL_LOVE_DURATION_TICKS,
                sheep_wool: None,
            })
        );
    }

    #[test]
    fn breeding_tick_index_tracks_state_and_lifecycle_changes() {
        let mut store = EntityStore::new();
        let mut idle = cow(Vec3::new(1.0, 64.0, 1.0));
        idle.animal = Some(AnimalBreedingState::adult());
        let idle_id = store.spawn(idle);
        let mut baby = cow(Vec3::new(2.0, 64.0, 2.0));
        baby.animal = Some(AnimalBreedingState::baby());
        let baby_id = store.spawn(baby);
        let mut in_love = cow(Vec3::new(3.0, 64.0, 3.0));
        in_love.animal = Some(AnimalBreedingState {
            age_ticks: 0,
            love_ticks: ANIMAL_LOVE_DURATION_TICKS,
            sheep_wool: None,
        });
        let love_id = store.spawn(in_love);

        let mut seen = Vec::new();
        store.visit_breeding_tick_entities(|entity| seen.push(entity.id));
        assert_eq!(seen, vec![baby_id, love_id]);

        assert!(store.set_animal_state(
            idle_id,
            AnimalBreedingState {
                age_ticks: 0,
                love_ticks: ANIMAL_LOVE_DURATION_TICKS,
                sheep_wool: None,
            },
        ));
        assert!(store.set_animal_state(baby_id, AnimalBreedingState::adult()));
        assert!(store.mark_despawning(love_id));

        let mut seen = Vec::new();
        store.visit_breeding_tick_entities(|entity| seen.push(entity.id));
        assert_eq!(seen, vec![idle_id]);

        assert!(store.remove(idle_id).is_some());
        let mut seen = Vec::new();
        store.visit_breeding_tick_entities(|entity| seen.push(entity.id));
        assert!(seen.is_empty());
    }

    #[test]
    fn sheep_index_intersects_candidates_and_tracks_state_changes() {
        let mut store = EntityStore::new();
        let mut near_sheep = cow(Vec3::new(1.0, 64.0, 1.0));
        near_sheep.type_id = 7;
        near_sheep.type_name = "minecraft:sheep".to_owned();
        near_sheep.animal = Some(AnimalBreedingState::adult_sheep(SheepColor::White));
        let near_sheep_id = store.spawn(near_sheep);
        let mut far_sheep = cow(Vec3::new(160.0, 64.0, 1.0));
        far_sheep.type_id = 7;
        far_sheep.type_name = "minecraft:sheep".to_owned();
        far_sheep.animal = Some(AnimalBreedingState::adult_sheep(SheepColor::Black));
        let far_sheep_id = store.spawn(far_sheep);
        let mut cow = cow(Vec3::new(2.0, 64.0, 1.0));
        cow.animal = Some(AnimalBreedingState::adult());
        let cow_id = store.spawn(cow);

        let mut seen = Vec::new();
        store.visit_sheep_entities_for_ids(&HashSet::from([near_sheep_id, cow_id]), |entity| {
            seen.push(entity.id)
        });
        assert_eq!(seen, vec![near_sheep_id]);
        assert!(!seen.contains(&far_sheep_id));

        assert!(store.set_animal_state(near_sheep_id, AnimalBreedingState::adult()));
        assert!(
            store.set_animal_state(cow_id, AnimalBreedingState::adult_sheep(SheepColor::White),)
        );
        let mut seen = Vec::new();
        store.visit_sheep_entities_for_ids(&HashSet::from([near_sheep_id, cow_id]), |entity| {
            seen.push(entity.id)
        });
        assert!(seen.is_empty());

        assert!(store.set_animal_state(
            near_sheep_id,
            AnimalBreedingState::adult_sheep(SheepColor::White),
        ));
        assert!(store.mark_despawning(near_sheep_id));
        let mut seen = Vec::new();
        store.visit_sheep_entities_for_ids(&HashSet::from([near_sheep_id]), |entity| {
            seen.push(entity.id)
        });
        assert!(seen.is_empty());
    }

    #[test]
    fn authoritative_sheep_wool_state_round_trips_through_ecs_snapshot() {
        let mut store = EntityStore::new();
        let mut entity = SpawnEntity::new(7, "minecraft:sheep", Vec3::new(2.0, 64.0, 3.0));
        entity.animal = Some(AnimalBreedingState::adult_sheep(SheepColor::White));

        let id = store.spawn(entity);
        let mut animal = store.snapshot(id).unwrap().animal.unwrap();
        assert_eq!(
            animal.sheep_wool,
            Some(SheepWoolState {
                color: SheepColor::White,
                sheared: false,
            })
        );

        animal.sheep_wool.as_mut().unwrap().sheared = true;
        assert!(store.set_animal_state(id, animal));
        assert_eq!(
            store
                .snapshot(id)
                .unwrap()
                .animal
                .unwrap()
                .sheep_wool
                .unwrap()
                .packed_metadata(),
            0x10
        );
    }

    #[test]
    fn vehicle_releases_removed_passenger() {
        let mut store = EntityStore::new();
        let passenger = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        let boat = store.spawn(SpawnEntity::vehicle(
            VehicleKind::Boat,
            15,
            "minecraft:oak_boat",
            Vec3::new(10.0, 63.0, 10.0),
        ));

        store.mount_vehicle(boat, passenger).unwrap();
        assert_eq!(store.vehicle_for_passenger(passenger), Some(boat));
        assert_eq!(store.remove(passenger).unwrap().id, passenger);
        assert_eq!(store.vehicle_for_passenger(passenger), None);
    }

    #[test]
    fn vehicle_mount_rejects_non_vehicle_and_double_mount() {
        let mut store = EntityStore::new();
        let first_passenger = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        let second_passenger = store.spawn(cow(Vec3::new(1.0, 64.0, 0.0)));
        let cow = store.spawn(cow(Vec3::new(2.0, 64.0, 0.0)));
        let boat = store.spawn(SpawnEntity::vehicle(
            VehicleKind::Boat,
            15,
            "minecraft:oak_boat",
            Vec3::new(3.0, 63.0, 0.0),
        ));

        assert_eq!(
            store.mount_vehicle(cow, first_passenger),
            Err(VehicleError::NotVehicle)
        );
        store.mount_vehicle(boat, first_passenger).unwrap();
        assert_eq!(
            store.mount_vehicle(boat, second_passenger),
            Err(VehicleError::AlreadyMounted)
        );
        assert_eq!(
            store.mount_vehicle(cow, first_passenger),
            Err(VehicleError::PassengerAlreadyMounted)
        );
    }

    #[test]
    fn vehicle_mount_rejects_self_mount_and_cycles() {
        let mut store = EntityStore::new();
        let first_boat = store.spawn(SpawnEntity::vehicle(
            VehicleKind::Boat,
            15,
            "minecraft:oak_boat",
            Vec3::new(0.0, 63.0, 0.0),
        ));
        let second_boat = store.spawn(SpawnEntity::vehicle(
            VehicleKind::Boat,
            15,
            "minecraft:oak_boat",
            Vec3::new(3.0, 63.0, 0.0),
        ));

        assert_eq!(
            store.mount_vehicle(first_boat, first_boat),
            Err(VehicleError::SelfMount)
        );
        store.mount_vehicle(second_boat, first_boat).unwrap();
        assert_eq!(
            store.mount_vehicle(first_boat, second_boat),
            Err(VehicleError::Cycle)
        );
    }

    #[test]
    fn vehicle_mount_and_input_reject_non_alive_entities() {
        let mut store = EntityStore::new();
        let passenger = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        let despawning_passenger = store.spawn(cow(Vec3::new(1.0, 64.0, 0.0)));
        let boat = store.spawn(SpawnEntity::vehicle(
            VehicleKind::Boat,
            15,
            "minecraft:oak_boat",
            Vec3::new(3.0, 63.0, 0.0),
        ));
        let despawning_boat = store.spawn(SpawnEntity::vehicle(
            VehicleKind::Boat,
            15,
            "minecraft:oak_boat",
            Vec3::new(6.0, 63.0, 0.0),
        ));

        store.mark_despawning(despawning_passenger);
        store.mark_despawning(despawning_boat);

        assert_eq!(
            store.mount_vehicle(boat, despawning_passenger),
            Err(VehicleError::InvalidLifecycle)
        );
        assert_eq!(
            store.mount_vehicle(despawning_boat, passenger),
            Err(VehicleError::InvalidLifecycle)
        );

        store.mount_vehicle(boat, passenger).unwrap();
        store.mark_despawning(passenger);
        assert_eq!(
            store.apply_vehicle_input(boat, passenger, VehicleInput::forward()),
            Err(VehicleError::InvalidLifecycle)
        );
    }

    #[test]
    fn insert_snapshot_keeps_valid_vehicle_graphs() {
        let mut store = EntityStore::new();
        let passenger = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        let vehicle = EntityId(40);

        assert!(store.insert_snapshot(vehicle_snapshot(
            vehicle,
            EntityLifecycle::Alive,
            Some(passenger),
        )));

        assert_eq!(store.vehicle_for_passenger(passenger), Some(vehicle));
        assert_eq!(
            store.snapshot(vehicle).unwrap().vehicle.unwrap().passenger,
            Some(passenger)
        );
    }

    #[test]
    fn insert_snapshot_keeps_authoritative_passenger_mount() {
        let mut store = EntityStore::new();
        let passenger = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        let vehicle = EntityId(40);

        assert!(store.insert_snapshot(vehicle_snapshot(
            vehicle,
            EntityLifecycle::Alive,
            Some(passenger),
        )));

        assert_eq!(store.vehicle_for_passenger(passenger), Some(vehicle));
    }

    #[test]
    fn insert_snapshot_drops_invalid_vehicle_graphs() {
        let mut store = EntityStore::new();
        let alive_passenger = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        let despawning_passenger = store.spawn(cow(Vec3::new(1.0, 64.0, 0.0)));
        store.mark_despawning(despawning_passenger);

        let cases = [
            (EntityId(40), EntityLifecycle::Alive, Some(EntityId(40))),
            (EntityId(41), EntityLifecycle::Alive, Some(EntityId(999))),
            (
                EntityId(42),
                EntityLifecycle::Alive,
                Some(despawning_passenger),
            ),
            (
                EntityId(43),
                EntityLifecycle::Despawning,
                Some(alive_passenger),
            ),
        ];

        for (vehicle, lifecycle, passenger) in cases {
            assert!(store.insert_snapshot(vehicle_snapshot(vehicle, lifecycle, passenger)));
            assert_eq!(store.snapshot(vehicle).unwrap().vehicle, None);
        }
        assert_eq!(store.vehicle_for_passenger(alive_passenger), None);
        assert_eq!(store.vehicle_for_passenger(despawning_passenger), None);
    }

    #[test]
    fn insert_snapshot_drops_vehicle_graph_cycles() {
        let mut store = EntityStore::new();
        let existing_vehicle = store.spawn(SpawnEntity::vehicle(
            VehicleKind::Boat,
            15,
            "minecraft:oak_boat",
            Vec3::new(3.0, 63.0, 0.0),
        ));
        let inserted_vehicle = EntityId(40);
        let mut existing_state = store.snapshot(existing_vehicle).unwrap().vehicle.unwrap();
        existing_state.passenger = Some(inserted_vehicle);
        assert!(store.set_vehicle_state(existing_vehicle, Some(existing_state)));

        assert!(store.insert_snapshot(vehicle_snapshot(
            inserted_vehicle,
            EntityLifecycle::Alive,
            Some(existing_vehicle),
        )));

        assert_eq!(store.snapshot(inserted_vehicle).unwrap().vehicle, None);
    }

    #[test]
    fn removing_passenger_clears_vehicle_mount() {
        let mut store = EntityStore::new();
        let passenger = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        let boat = store.spawn(SpawnEntity::vehicle(
            VehicleKind::Boat,
            15,
            "minecraft:oak_boat",
            Vec3::new(3.0, 63.0, 0.0),
        ));
        store.mount_vehicle(boat, passenger).unwrap();

        store.remove(passenger).unwrap();

        assert_eq!(
            store.snapshot(boat).unwrap().vehicle.unwrap().passenger,
            None
        );
        assert_eq!(store.vehicle_for_passenger(passenger), None);
    }

    #[test]
    fn minecart_mount_exists_but_steering_is_explicitly_unsupported() {
        let mut store = EntityStore::new();
        let passenger = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        let minecart = store.spawn(SpawnEntity::vehicle(
            VehicleKind::Minecart,
            63,
            "minecraft:minecart",
            Vec3::new(3.0, 63.0, 0.0),
        ));
        store.mount_vehicle(minecart, passenger).unwrap();

        assert_eq!(
            store.apply_vehicle_input(minecart, passenger, VehicleInput::forward()),
            Err(VehicleError::UnsupportedSteering)
        );
    }

    #[test]
    fn bounded_pathing_moves_over_flat_loaded_terrain() {
        let mut store = EntityStore::new();
        let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        store.set_velocity(follower, Vec3::new(0.0, -0.25, 0.0));
        store.set_goal(
            follower,
            GoalState::FollowPosition {
                target: Vec3::new(4.0, 64.0, 0.0),
                speed: 0.5,
            },
        );
        let probe = TestPathingProbe::new(PathingProbeResult::Walkable);

        let stats = store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);

        let velocity = store.snapshot(follower).unwrap().velocity;
        assert_eq!(stats.pathing_moves, 1);
        assert!((velocity.x - 0.5).abs() < 0.000_001);
        assert_eq!(velocity.y, -0.25);
        assert_eq!(velocity.z, 0.0);
    }

    #[test]
    fn bounded_pathing_probes_speed_scaled_next_position() {
        let mut store = EntityStore::new();
        let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        store.set_goal(
            follower,
            GoalState::FollowPosition {
                target: Vec3::new(4.0, 64.0, 0.0),
                speed: 0.25,
            },
        );
        let probe = TestPathingProbe::new(PathingProbeResult::Unloaded).with(
            0.0125,
            64.0,
            0.0,
            PathingProbeResult::Walkable,
        );

        let stats = store.tick_goals_with_pathing(
            1,
            &probe,
            PathingBudget {
                max_candidates_per_entity: 1,
                ..PathingBudget::DEFAULT
            },
        );

        let velocity = store.snapshot(follower).unwrap().velocity;
        assert_eq!(stats.pathing_moves, 1);
        assert!((velocity.x - 0.25).abs() < 0.000_001);
        assert_eq!(velocity.y, 0.0);
        assert_eq!(velocity.z, 0.0);
    }

    #[test]
    fn prepared_goal_tick_exposes_exact_probe_positions() {
        let mut store = EntityStore::new();
        let follower = store.spawn(cow(Vec3::new(15.9, 64.0, 0.5)));
        store.set_goal(
            follower,
            GoalState::FollowPosition {
                target: Vec3::new(20.0, 64.0, 0.5),
                speed: 4.0,
            },
        );
        let prepared = store.prepare_goal_tick_with_pathing_for_ids(1, &HashSet::from([follower]));
        let mut positions = Vec::new();

        prepared.visit_pathing_probe_positions(
            PathingBudget {
                max_candidates_per_entity: 1,
                ..PathingBudget::DEFAULT
            },
            |entity, position| positions.push((entity, position)),
        );

        assert_eq!(
            positions,
            vec![
                (follower, Vec3::new(16.1, 64.0, 0.5)),
                (follower, Vec3::new(16.1, 65.0, 0.5)),
            ]
        );
    }

    #[test]
    fn prepared_goal_tick_declares_every_position_resolve_may_probe() {
        struct DeclaredOnlyProbe {
            declared: HashSet<(i64, i64, i64)>,
        }

        impl PathingProbe for DeclaredOnlyProbe {
            fn can_stand_at(&self, position: Vec3) -> PathingProbeResult {
                assert!(
                    self.declared.contains(&pathing_test_key(position)),
                    "resolve probed undeclared position {position:?}"
                );
                if position.y > 64.0 {
                    PathingProbeResult::Walkable
                } else {
                    PathingProbeResult::Blocked
                }
            }
        }

        let mut store = EntityStore::new();
        let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        store.set_goal(
            follower,
            GoalState::FollowPosition {
                target: Vec3::new(4.0, 64.0, 0.0),
                speed: 0.5,
            },
        );
        let prepared = store.prepare_goal_tick_with_pathing_for_ids(1, &HashSet::from([follower]));
        let mut declared = HashSet::new();
        prepared.visit_pathing_probe_positions(PathingBudget::DEFAULT, |_, position| {
            declared.insert(pathing_test_key(position));
        });

        let probe = DeclaredOnlyProbe { declared };
        let _ = prepared.resolve(&probe, PathingBudget::DEFAULT);
    }

    #[test]
    fn prepared_goal_tick_declares_retained_node_and_fallback_positions() {
        struct DeclaredFallbackProbe {
            declared: HashSet<(i64, i64, i64)>,
            calls: Cell<usize>,
        }

        impl PathingProbe for DeclaredFallbackProbe {
            fn can_stand_at(&self, position: Vec3) -> PathingProbeResult {
                assert!(
                    self.declared.contains(&pathing_test_key(position)),
                    "fallback resolve probed undeclared position {position:?}"
                );
                let call = self.calls.get();
                self.calls.set(call + 1);
                if call < 2 {
                    PathingProbeResult::Blocked
                } else {
                    PathingProbeResult::Walkable
                }
            }
        }

        let id = EntityId(7);
        let target = Vec3::new(4.0, 64.0, 0.0);
        let retained = RetainedPathState {
            nodes: [Vec3::new(1.5, 64.0, 1.5), target],
            node_count: 2,
            target,
            target_revision: 1,
            has_target: true,
            ..RetainedPathState::default()
        };
        let prepared = PreparedGoalTick {
            tick: 2,
            active_ids: None,
            pathing_requests: vec![GoalPathingRequest {
                id,
                expected_position: Vec3::new(0.0, 64.0, 0.0),
                expected_rotation: Rotation::ZERO,
                expected_velocity: Vec3::ZERO,
                expected_on_ground: true,
                expected_goal: GoalState::FollowPosition { target, speed: 1.0 },
                expected_path: retained,
                target,
                target_epoch: None,
                speed: 1.0,
            }],
        };
        let mut declared = HashSet::new();
        prepared.visit_pathing_probe_positions(PathingBudget::DEFAULT, |_, position| {
            declared.insert(pathing_test_key(position));
        });
        let probe = DeclaredFallbackProbe {
            declared,
            calls: Cell::new(0),
        };

        let _ = prepared.resolve(&probe, PathingBudget::DEFAULT);

        assert_eq!(
            probe.calls.get(),
            3,
            "retained flat and step probes must precede the fallback direct probe"
        );
    }

    #[test]
    fn bounded_pathing_probes_the_next_tick_position() {
        struct RecordingProbe(std::sync::Mutex<Vec<Vec3>>);

        impl PathingProbe for RecordingProbe {
            fn can_stand_at(&self, position: Vec3) -> PathingProbeResult {
                self.0.lock().unwrap().push(position);
                PathingProbeResult::Walkable
            }
        }

        let mut store = EntityStore::new();
        let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        store.set_goal(
            follower,
            GoalState::FollowPosition {
                target: Vec3::new(4.0, 64.0, 0.0),
                speed: 2.0,
            },
        );
        let probe = RecordingProbe(std::sync::Mutex::new(Vec::new()));

        store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);

        let positions = probe.0.into_inner().unwrap();
        assert_eq!(positions.len(), 1);
        assert!((positions[0].x - 0.1).abs() < 1.0e-9);
        assert_eq!(positions[0].y, 64.0);
        assert_eq!(positions[0].z, 0.0);
    }

    #[test]
    fn bounded_pathing_steps_up_one_block_when_flat_route_is_blocked() {
        let mut store = EntityStore::new();
        let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        store.set_goal(
            follower,
            GoalState::FollowPosition {
                target: Vec3::new(4.0, 64.0, 0.0),
                speed: 0.5,
            },
        );
        let probe = TestPathingProbe::new(PathingProbeResult::Blocked).with(
            0.025,
            65.0,
            0.0,
            PathingProbeResult::Walkable,
        );

        let stats = store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);

        let velocity = store.snapshot(follower).unwrap().velocity;
        assert_eq!(stats.pathing_moves, 1);
        assert!((velocity.x - 0.5).abs() < 0.000_001);
        assert!((velocity.y - 0.5).abs() < 0.000_001);
        assert_eq!(velocity.z, 0.0);
    }

    #[test]
    fn bounded_pathing_detours_around_blocked_direct_step() {
        let mut store = EntityStore::new();
        let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        store.set_goal(
            follower,
            GoalState::FollowPosition {
                target: Vec3::new(4.0, 64.0, 0.0),
                speed: 0.5,
            },
        );
        let side = 0.025 / 2.0_f64.sqrt();
        let probe = TestPathingProbe::new(PathingProbeResult::Blocked).with(
            side,
            64.0,
            side,
            PathingProbeResult::Walkable,
        );

        let stats = store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);

        let velocity = store.snapshot(follower).unwrap().velocity;
        assert_eq!(stats.pathing_moves, 1);
        assert!(velocity.x > 0.0);
        assert!(velocity.z.abs() > 0.0);
        assert!(velocity.horizontal_len() <= 0.5 + 0.000_001);
    }

    struct TwoBlockWallPathingProbe;

    impl PathingProbe for TwoBlockWallPathingProbe {
        fn can_stand_at(&self, position: Vec3) -> PathingProbeResult {
            if (0.15..=2.2).contains(&position.x) && position.z.abs() < 0.1 {
                PathingProbeResult::Blocked
            } else {
                PathingProbeResult::Walkable
            }
        }
    }

    #[test]
    fn retained_path_routes_around_two_block_wall_and_rejoins_target() {
        let mut store = EntityStore::new();
        let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        let target = Vec3::new(4.0, 64.0, 0.0);
        store.set_goal(follower, GoalState::FollowPosition { target, speed: 4.0 });
        let mut retained_direction = None;

        for tick in 1..=64 {
            store.tick_goals_with_pathing(tick, &TwoBlockWallPathingProbe, PathingBudget::DEFAULT);
            let snapshot = store.snapshot(follower).expect("follower snapshot");
            let direction = snapshot.velocity.horizontal_normalized();
            if tick == 1 {
                assert!(direction.z.abs() > 0.0, "wall must force a detour");
                retained_direction = Some(direction);
            } else if tick <= 4 {
                let retained = retained_direction.expect("initial detour direction");
                assert!(
                    direction.x * retained.x + direction.z * retained.z > 0.999,
                    "retained route must not oscillate while clearing the wall"
                );
            }
            store.tick_positions(0.05);
            let position = store.snapshot(follower).expect("moved follower").position;
            if (target.x - position.x).hypot(target.z - position.z) <= 0.25 {
                assert!(position.z.abs() <= 0.25, "route must rejoin the target");
                return;
            }
        }

        panic!("retained route did not reach the target within its bounded path budget");
    }

    #[test]
    fn wander_retains_its_absolute_target_within_one_period() {
        let mut store = EntityStore::new();
        let mut entity = cow(Vec3::new(0.0, 64.0, 0.0));
        entity.goal = GoalState::Wander {
            speed: 4.0,
            period_ticks: 40,
        };
        let id = store.spawn(entity);
        let probe = TestPathingProbe::new(PathingProbeResult::Walkable);

        store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);
        store.tick_positions(0.05);
        let prepared = store.prepare_goal_tick_with_pathing_for_ids(2, &HashSet::from([id]));
        let request = &prepared.pathing_requests[0];

        assert_eq!(request.expected_path.target_revision, 1);
        assert_eq!(
            request.target, request.expected_path.target,
            "moving within one Wander period must not replace the retained absolute target"
        );
    }

    #[test]
    fn wander_stops_after_reaching_its_retained_target() {
        let mut store = EntityStore::new();
        let mut entity = cow(Vec3::new(0.0, 64.0, 0.0));
        entity.goal = GoalState::Wander {
            speed: 3.0,
            period_ticks: 40,
        };
        let id = store.spawn(entity);
        let probe = TestPathingProbe::new(PathingProbeResult::Walkable);

        for tick in 1..=6 {
            store.tick_goals_with_pathing(tick, &probe, PathingBudget::DEFAULT);
            store.tick_positions(PathingBudget::TICK_SECONDS);
        }

        let prepared = store.prepare_goal_tick_with_pathing_for_ids(7, &HashSet::from([id]));
        let request = &prepared.pathing_requests[0];
        let position = store.snapshot(id).expect("wanderer snapshot").position;
        let reach = 3.0 * PathingBudget::TICK_SECONDS * 1.25;
        assert!(
            (request.target.x - position.x).hypot(request.target.z - position.z) <= reach,
            "fixture must enter the retained target radius"
        );
        let rotation = store.snapshot(id).unwrap().rotation;

        store.apply_prepared_goal_tick(prepared.resolve(&probe, PathingBudget::DEFAULT));

        assert_eq!(store.snapshot(id).unwrap().velocity, Vec3::ZERO);
        assert_eq!(store.snapshot(id).unwrap().rotation, rotation);
        store.tick_positions(PathingBudget::TICK_SECONDS);
        assert_eq!(store.snapshot(id).unwrap().position, position);
    }

    #[test]
    fn wander_replaces_its_target_at_the_period_boundary() {
        let mut store = EntityStore::new();
        let mut entity = cow(Vec3::new(0.0, 64.0, 0.0));
        entity.goal = GoalState::Wander {
            speed: 4.0,
            period_ticks: 40,
        };
        let id = store.spawn(entity);
        let probe = TestPathingProbe::new(PathingProbeResult::Walkable);

        store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);
        let prepared = store.prepare_goal_tick_with_pathing_for_ids(40, &HashSet::from([id]));
        let request = &prepared.pathing_requests[0];

        assert_eq!(request.expected_path.target_revision, 1);
        assert_ne!(request.target, request.expected_path.target);

        store.apply_prepared_goal_tick(prepared.resolve(&probe, PathingBudget::DEFAULT));
        let next = store.prepare_goal_tick_with_pathing_for_ids(41, &HashSet::from([id]));
        assert_eq!(next.pathing_requests[0].expected_path.target_revision, 2);
        assert_eq!(
            next.pathing_requests[0].target,
            next.pathing_requests[0].expected_path.target
        );
    }

    #[test]
    fn path_probe_budget_is_global_across_retained_fallback() {
        struct CountingBlockedProbe(Cell<usize>);

        impl PathingProbe for CountingBlockedProbe {
            fn can_stand_at(&self, _position: Vec3) -> PathingProbeResult {
                self.0.set(self.0.get() + 1);
                PathingProbeResult::Blocked
            }
        }

        let id = EntityId(7);
        let target = Vec3::new(4.0, 64.0, 0.0);
        let retained = RetainedPathState {
            nodes: [Vec3::new(1.5, 64.0, 1.5), target],
            node_count: 2,
            target,
            target_revision: 1,
            has_target: true,
            ..RetainedPathState::default()
        };
        let prepared = PreparedGoalTick {
            tick: 2,
            active_ids: None,
            pathing_requests: vec![GoalPathingRequest {
                id,
                expected_position: Vec3::new(0.0, 64.0, 0.0),
                expected_rotation: Rotation::ZERO,
                expected_velocity: Vec3::ZERO,
                expected_on_ground: true,
                expected_goal: GoalState::FollowPosition { target, speed: 1.0 },
                expected_path: retained,
                target,
                target_epoch: None,
                speed: 1.0,
            }],
        };
        let probe = CountingBlockedProbe(Cell::new(0));
        let budget = PathingBudget {
            max_candidates_per_entity: 3,
            ..PathingBudget::DEFAULT
        };

        let _ = prepared.resolve(&probe, budget);

        assert_eq!(
            probe.0.get(),
            budget.max_candidates_per_entity,
            "the configured per-entity bound must be the actual probe-call ceiling"
        );
    }

    #[test]
    fn retreat_candidate_is_reached_within_the_actual_probe_budget() {
        struct RetreatProbe(RefCell<Vec<Vec3>>);

        impl PathingProbe for RetreatProbe {
            fn can_stand_at(&self, position: Vec3) -> PathingProbeResult {
                self.0.borrow_mut().push(position);
                if position.x < 0.0 && position.y == 64.0 {
                    PathingProbeResult::Walkable
                } else {
                    PathingProbeResult::Blocked
                }
            }
        }

        let mut store = EntityStore::new();
        let mut entity = cow(Vec3::new(0.0, 64.0, 0.0));
        entity.goal = GoalState::FollowPosition {
            target: Vec3::new(4.0, 64.0, 0.0),
            speed: 1.0,
        };
        let id = store.spawn(entity);
        let probe = RetreatProbe(RefCell::new(Vec::new()));

        let stats = store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);

        assert_eq!(stats.pathing_moves, 1);
        assert!(store.snapshot(id).unwrap().velocity.x < 0.0);
        let visited = probe.0.borrow();
        assert!(visited[6].x < 0.0, "the seventh probe must be the retreat");
        assert_eq!(
            visited.len(),
            PathingBudget::DEFAULT.max_candidates_per_entity
        );
    }

    struct MutableRetainedNodeProbe {
        initial_wall: Cell<bool>,
        blocked_next: Cell<Option<Vec3>>,
        visited: RefCell<Vec<Vec3>>,
    }

    impl PathingProbe for MutableRetainedNodeProbe {
        fn can_stand_at(&self, position: Vec3) -> PathingProbeResult {
            self.visited.borrow_mut().push(position);
            if self.initial_wall.get()
                && (position.x - 0.2).abs() < 0.000_001
                && position.z.abs() < 0.000_001
            {
                return PathingProbeResult::Blocked;
            }
            if self.blocked_next.get().is_some_and(|blocked| {
                (position.x - blocked.x).abs() < 0.000_001
                    && (position.z - blocked.z).abs() < 0.000_001
            }) {
                return PathingProbeResult::Blocked;
            }
            PathingProbeResult::Walkable
        }
    }

    #[test]
    fn retained_path_recomputes_when_next_node_becomes_blocked() {
        let mut store = EntityStore::new();
        let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        let target = Vec3::new(4.0, 64.0, 0.0);
        store.set_goal(follower, GoalState::FollowPosition { target, speed: 4.0 });
        let probe = MutableRetainedNodeProbe {
            initial_wall: Cell::new(true),
            blocked_next: Cell::new(None),
            visited: RefCell::new(Vec::new()),
        };

        store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);
        let first_velocity = store.snapshot(follower).expect("first decision").velocity;
        assert!(first_velocity.z.abs() > 0.0, "wall must force a detour");
        store.tick_positions(0.05);

        let position = store.snapshot(follower).expect("moved follower").position;
        let retained_next = Vec3::new(
            position.x + first_velocity.x * 0.05,
            position.y,
            position.z + first_velocity.z * 0.05,
        );
        probe.initial_wall.set(false);
        probe.blocked_next.set(Some(retained_next));
        probe.visited.borrow_mut().clear();

        store.tick_goals_with_pathing(2, &probe, PathingBudget::DEFAULT);

        let visited = probe.visited.borrow();
        assert_eq!(
            visited.first().copied(),
            Some(retained_next),
            "the retained next node must be validated before recomputation"
        );
        let velocity = store
            .snapshot(follower)
            .expect("recomputed decision")
            .velocity;
        assert!(velocity.x > 0.0);
        assert!(velocity.z.abs() < first_velocity.z.abs());
    }

    #[test]
    fn retained_path_stops_after_bounded_no_progress() {
        let mut store = EntityStore::new();
        let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        store.set_goal(
            follower,
            GoalState::FollowPosition {
                target: Vec3::new(4.0, 64.0, 0.0),
                speed: 1.0,
            },
        );
        let probe = TestPathingProbe::new(PathingProbeResult::Walkable);
        let mut stopped = false;

        for tick in 1..=64 {
            let stats = store.tick_goals_with_pathing(tick, &probe, PathingBudget::DEFAULT);
            if stats.pathing_blocked == 1 {
                stopped = true;
                break;
            }
        }

        assert!(stopped, "no-progress recovery must have a finite bound");
        assert_eq!(store.snapshot(follower).unwrap().velocity, Vec3::ZERO);
    }

    #[test]
    fn retained_follow_position_recovers_after_temporary_obstacle_clears() {
        struct MutableProbe(Cell<PathingProbeResult>);

        impl PathingProbe for MutableProbe {
            fn can_stand_at(&self, _position: Vec3) -> PathingProbeResult {
                self.0.get()
            }
        }

        let mut store = EntityStore::new();
        let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        store.set_goal(
            follower,
            GoalState::FollowPosition {
                target: Vec3::new(4.0, 64.0, 0.0),
                speed: 1.0,
            },
        );
        let probe = MutableProbe(Cell::new(PathingProbeResult::Blocked));

        for tick in 1..=RETAINED_PATH_RECOMPUTE_LIMIT.into() {
            store.tick_goals_with_pathing(tick, &probe, PathingBudget::DEFAULT);
        }
        assert_eq!(store.snapshot(follower).unwrap().velocity, Vec3::ZERO);

        probe.0.set(PathingProbeResult::Walkable);
        let stats = store.tick_goals_with_pathing(5, &probe, PathingBudget::DEFAULT);

        assert_eq!(stats.pathing_moves, 1);
        assert!(store.snapshot(follower).unwrap().velocity.x > 0.0);
    }

    #[test]
    fn teleport_and_goal_change_invalidate_retained_path() {
        let mut store = EntityStore::new();
        let mut entity = cow(Vec3::new(0.0, 64.0, 0.0));
        entity.goal = GoalState::FollowPosition {
            target: Vec3::new(4.0, 64.0, 0.0),
            speed: 1.0,
        };
        let id = store.spawn(entity);
        store.tick_goals_with_pathing(
            1,
            &TestPathingProbe::new(PathingProbeResult::Walkable),
            PathingBudget::DEFAULT,
        );

        assert!(store.set_position(id, Vec3::new(2.0, 64.0, 2.0)));
        let after_teleport = store.prepare_goal_tick_with_pathing_for_ids(2, &HashSet::from([id]));
        assert_eq!(
            after_teleport.pathing_requests[0].expected_path,
            RetainedPathState::default()
        );

        store.tick_goals_with_pathing(
            2,
            &TestPathingProbe::new(PathingProbeResult::Walkable),
            PathingBudget::DEFAULT,
        );
        assert!(store.set_goal(
            id,
            GoalState::FollowPosition {
                target: Vec3::new(4.0, 64.0, 0.0),
                speed: 2.0,
            },
        ));
        let after_goal_change =
            store.prepare_goal_tick_with_pathing_for_ids(3, &HashSet::from([id]));
        assert_eq!(
            after_goal_change.pathing_requests[0].expected_path,
            RetainedPathState::default()
        );
    }

    #[test]
    fn dense_ecs_pathing_requests_are_stably_ordered() {
        let mut store = EntityStore::new();
        let spawn_follower = |store: &mut EntityStore| {
            let mut entity = cow(Vec3::new(0.0, 64.0, 0.0));
            entity.goal = GoalState::FollowPosition {
                target: Vec3::new(4.0, 64.0, 0.0),
                speed: 1.0,
            };
            store.spawn(entity)
        };
        let first = spawn_follower(&mut store);
        let _second = spawn_follower(&mut store);
        let _third = spawn_follower(&mut store);
        store.remove(first).expect("first entity exists");
        let _fourth = spawn_follower(&mut store);

        let prepared = store.prepare_goal_tick_with_pathing_for_ids(
            1,
            &store.snapshots().map(|snapshot| snapshot.id).collect(),
        );
        let ids = prepared
            .pathing_requests
            .iter()
            .map(|request| request.id)
            .collect::<Vec<_>>();
        let mut sorted = ids.clone();
        sorted.sort_unstable();

        assert_eq!(ids, sorted);
    }

    #[test]
    fn bounded_pathing_detours_around_unloaded_direct_step() {
        let mut store = EntityStore::new();
        let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        store.set_goal(
            follower,
            GoalState::FollowPosition {
                target: Vec3::new(4.0, 64.0, 0.0),
                speed: 0.5,
            },
        );
        let probe = TestPathingProbe::new(PathingProbeResult::Walkable).with(
            0.025,
            64.0,
            0.0,
            PathingProbeResult::Unloaded,
        );

        let stats = store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);

        let velocity = store.snapshot(follower).unwrap().velocity;
        assert_eq!(stats.pathing_moves, 1);
        assert!(velocity.x > 0.0);
        assert!(velocity.z.abs() > 0.0);
        assert!(velocity.horizontal_len() <= 0.5 + 0.000_001);
    }

    #[test]
    fn bounded_pathing_refuses_blocked_terrain() {
        let mut store = EntityStore::new();
        let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        store.set_goal(
            follower,
            GoalState::FollowPosition {
                target: Vec3::new(4.0, 64.0, 0.0),
                speed: 0.5,
            },
        );
        let probe = TestPathingProbe::new(PathingProbeResult::Blocked);

        let stats = store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);

        assert_eq!(stats.pathing_blocked, 1);
        assert_eq!(store.snapshot(follower).unwrap().velocity, Vec3::ZERO);
    }

    #[test]
    fn bounded_pathing_refuses_unloaded_terrain() {
        let mut store = EntityStore::new();
        let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
        store.set_goal(
            follower,
            GoalState::FollowPosition {
                target: Vec3::new(4.0, 64.0, 0.0),
                speed: 0.5,
            },
        );
        let probe = TestPathingProbe::new(PathingProbeResult::Unloaded);

        let stats = store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);

        assert_eq!(stats.pathing_unloaded, 1);
        assert_eq!(store.snapshot(follower).unwrap().velocity, Vec3::ZERO);
    }

    #[test]
    fn wander_uses_bounded_pathing_instead_of_walking_into_blocked_terrain() {
        let mut store = EntityStore::new();
        let mut entity = cow(Vec3::new(0.0, 64.0, 0.0));
        entity.goal = GoalState::Wander {
            speed: 0.2,
            period_ticks: 20,
        };
        let id = store.spawn(entity);
        let probe = TestPathingProbe::new(PathingProbeResult::Blocked);

        let stats = store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);

        assert_eq!(stats.pathing_blocked, 1);
        assert_eq!(store.snapshot(id).unwrap().velocity, Vec3::ZERO);
    }

    #[test]
    #[ignore = "explicit debug active-subset ECS benchmark"]
    fn active_subset_ecs_density_benchmark_report() {
        const ENTITIES: usize = 10_000;
        const ACTIVE: usize = 32;
        const TICKS: u64 = 200;

        let mut store = EntityStore::new();
        let mut active_ids = HashSet::with_capacity(ACTIVE);
        for index in 0..ENTITIES {
            let mut entity = cow(Vec3::new(index as f64, 64.0, (index % 32) as f64));
            entity.goal = GoalState::Wander {
                speed: 0.2,
                period_ticks: 20,
            };
            let id = store.spawn(entity);
            if index < ACTIVE {
                active_ids.insert(id);
            }
        }
        let probe = TestPathingProbe::new(PathingProbeResult::Walkable);

        let started = std::time::Instant::now();
        for tick in 1..=TICKS {
            let prepared = store.prepare_goal_tick_with_pathing_for_ids(tick, &active_ids);
            store.apply_prepared_goal_tick(prepared.resolve(&probe, PathingBudget::DEFAULT));
            std::hint::black_box(store.alive_kinematics_for_ids(&active_ids));
        }
        let elapsed = started.elapsed();

        println!(
            "ENTITY_ACTIVE_SUBSET_BENCH entities={ENTITIES} active={ACTIVE} ticks={TICKS} total_us={} us_per_tick={}",
            elapsed.as_micros(),
            elapsed.as_micros() / u128::from(TICKS),
        );
    }

    fn runtime_density_store(entities: usize) -> EntityStore {
        let mut store = EntityStore::new();
        for index in 0..entities {
            let mut entity = cow(Vec3::new(index as f64, 64.0, (index % 32) as f64));
            entity.goal = GoalState::Wander {
                speed: 0.2,
                period_ticks: 20,
            };
            store.spawn(entity);
        }
        store
    }

    #[test]
    #[ignore = "explicit debug dense active-set ECS benchmark"]
    fn dense_active_set_ecs_benchmark_report() {
        const ENTITIES: usize = 1_000;
        const TICKS: u64 = 200;

        let mut full_query = runtime_density_store(ENTITIES);
        let mut indexed = runtime_density_store(ENTITIES);
        let active_ids = indexed
            .snapshots()
            .map(|snapshot| snapshot.id)
            .collect::<HashSet<_>>();
        let probe = TestPathingProbe::new(PathingProbeResult::Walkable);

        let full_started = std::time::Instant::now();
        for tick in 1..=TICKS {
            let prepared = full_query.prepare_goal_tick(tick, None);
            full_query.apply_prepared_goal_tick(prepared.resolve(&probe, PathingBudget::DEFAULT));
        }
        let full_elapsed = full_started.elapsed();

        let indexed_started = std::time::Instant::now();
        for tick in 1..=TICKS {
            let prepared = indexed.prepare_goal_tick_with_pathing_for_ids(tick, &active_ids);
            indexed.apply_prepared_goal_tick(prepared.resolve(&probe, PathingBudget::DEFAULT));
        }
        let indexed_elapsed = indexed_started.elapsed();

        assert_eq!(
            full_query.snapshots().collect::<Vec<_>>(),
            indexed.snapshots().collect::<Vec<_>>()
        );
        println!(
            "ENTITY_DENSE_ACTIVE_BENCH entities={ENTITIES} ticks={TICKS} full_us_per_tick={} indexed_us_per_tick={}",
            full_elapsed.as_micros() / u128::from(TICKS),
            indexed_elapsed.as_micros() / u128::from(TICKS),
        );
    }
}
