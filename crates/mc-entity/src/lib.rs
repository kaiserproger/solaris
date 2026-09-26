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

pub mod aquatic_motion;
pub mod attributes_26_1_2;
pub mod dragon_26_1_2;
pub mod effects_26_1_2;
mod entity_projections;
mod entity_scale_26_1_2;
mod entity_vehicle;
pub mod equipment_26_1_2;
pub mod fire_26_1_2;
pub mod group_orders;
pub mod living_26_1_2;
mod lock_policy;
pub mod mob_control_26_1_2;
pub mod natural_spawn_26_1_2;
pub mod navigation_26_1_2;
pub mod player_combat_26_1_2;
pub mod player_survival_26_1_2;
pub mod projectile_26_1_2;
mod regional;
mod runtime;
pub mod runtime_26_1_2;
pub mod synced_data_26_1_2;
pub mod villager_26_1_2;
pub mod villager_gossip_26_1_2;
pub mod villager_merchant_26_1_2;
pub mod villager_population_26_1_2;
pub mod zombie_villager_26_1_2;

#[cfg(test)]
#[path = "entity_scale_26_1_2_tests.rs"]
mod entity_scale_26_1_2_tests;

#[cfg(test)]
#[path = "natural_spawn_26_1_2_tests.rs"]
mod natural_spawn_26_1_2_tests;

#[cfg(test)]
#[path = "entity_vehicle_tests.rs"]
mod entity_vehicle_tests;

#[cfg(test)]
mod animal_panic_tests;

#[cfg(test)]
#[path = "villager_behavior_tests.rs"]
mod villager_behavior_tests;

pub use entity_projections::{EntityDespawnProjection, EntitySimulationProjection};
pub use entity_scale_26_1_2::{EntityScale26_1_2, EntityScaleError};
pub use group_orders::{
    FormationKind, FormationPlacement, FormationSlot, FormationSlots, GroupAdmission,
    GroupAdmissionPhase, GroupAdmissionRejection, GroupApplyOutcome, GroupMemberFailure,
    GroupMemberFence, GroupMemberObservation, GroupMemberRejection, TargetCandidate,
    TargetCategory, TargetPolicy, select_target, select_targets,
};
pub use lock_policy::{
    LockPoisonMetricsSnapshot, authoritative_lock_poison_from_panic, lock_poison_metrics_snapshot,
};
pub use regional::{
    CompactEntityKinematicsFence, ItemPickupClaimResolution, LaneCommitTimings,
    PreparedSnapshotMutation, REGION_SIZE_CHUNKS, RegionEntityStoreError, RegionEpoch, RegionKey,
    RegionLease, RegionOwnerBatch, RegionOwnerCompletion, RegionOwnerLaneError,
    RegionOwnerLaneStartError, RegionOwnerMutation, RegionOwnership, RegionOwnershipError,
    RegionPhase, RegionalCommitDecision, RegionalDecisionJournal, RegionalDecisionJournalError,
    RegionalEntityAuthority, RegionalEntityPhysicsOutput, RegionalEntityStore,
    RegionalEntityTickInput, RegionalEntityTickOutput, RegionalGoalTickInputs,
    RegionalKinematicsApply, RegionalOwnerCoordinator, RegionalOwnerCutoverError,
    RegionalOwnerHandle, RegionalOwnerLane, RegionalOwnerRuntime,
    RegionalOwnerRuntimeShutdownError, RegionalOwnerSaveSnapshot, RegionalOwnerShutdownError,
    RegionalOwnerStatus, RegionalPreparedEntityPhysics, RegionalPreparedGoalTick,
    RegionalResolvedGoalTick, RegionalTickWorld, RegionalVillagerGoalTickInputs,
    RegionalVillagerProfessionOffer, SequencedRegionMutation, TransferApply, TransferDecision,
    TransferId, VersionedEntityKinematics, VersionedEntitySnapshots, VersionedKinematicsCommit,
    VillagerBirthCommit, VillagerCourtshipCommit, VillagerFoodShareCommit,
    VillagerInventoryPickupCommit, VillagerNoBedCommit, warm_physics_caches,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_name: Option<Box<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_model: Option<Box<mc_data::Identifier>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stew_effects: Vec<mc_data::item_stack::StewEffect>,
}

impl EntityItemStack {
    #[must_use]
    pub const fn new(item_id: u32, count: i32) -> Self {
        Self {
            item_id,
            count,
            damage: None,
            enchantments: Vec::new(),
            custom_name: None,
            item_model: None,
            stew_effects: Vec::new(),
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
    pub fn with_custom_name(mut self, name: impl Into<String>) -> Self {
        self.custom_name = Some(Box::new(name.into()));
        self
    }

    #[must_use]
    pub fn with_item_model(mut self, model: mc_data::Identifier) -> Self {
        self.item_model = Some(Box::new(model));
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
    /// Allocator-only id span reserved immediately after this entity.
    pub reserved_following_ids: u8,
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
            reserved_following_ids: 0,
        }
    }

    #[must_use]
    pub const fn reserve_following_ids(mut self, count: u8) -> Self {
        self.reserved_following_ids = count;
        self
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
    #[serde(default)]
    pub fall_distance: f64,
    #[serde(default)]
    pub remaining_fire_ticks: i32,
    pub active_effects: Option<EntityActiveEffectsState>,
    pub arrow_state: Option<projectile_26_1_2::ArrowState>,
    #[serde(default)]
    pub hurting_projectile_state: Option<projectile_26_1_2::HurtingProjectileState>,
    #[serde(skip)]
    pub throwable_projectile_state: Option<projectile_26_1_2::ThrowableState>,
    pub last_damage_tick: Option<u64>,
    /// Last durable plugin operation whose supply debit paid for native healing.
    #[serde(default)]
    pub treatment_decision_id: u64,
    pub death_remove_tick: Option<u64>,
    pub sheep_grazing_ticks: Option<u8>,
    pub spawn_tick: u64,
    pub item_pickup_ready_tick: Option<u64>,
    pub item_pickup_owner_block: Option<EntityItemPickupOwnerBlock>,
    /// Runtime-only cross-owner pickup reservation. Checkpoints intentionally
    /// omit it so an interrupted transaction restarts from the unchanged item.
    #[serde(skip)]
    pub item_pickup_claim: Option<u64>,
    /// Recipient for food thrown by one villager to another.
    #[serde(default)]
    pub villager_food_recipient: Option<EntityId>,
    pub primed_tnt: Option<EntityPrimedTntState>,
    #[serde(default)]
    pub pending_explosion: Option<EntityPendingExplosionState>,
    #[serde(default)]
    pub crossbow_attack: Option<EntityCrossbowAttackState>,
    /// Runtime-only RangedBowAttackGoal draw/cooldown. Vanilla does not persist goal-local timers.
    #[serde(skip)]
    pub bow_attack: Option<EntityBowAttackState>,
    /// Runtime-only BlazeAttackGoal step/deadline. Vanilla does not persist goal-local timers.
    #[serde(skip)]
    pub blaze_attack: Option<EntityBlazeAttackState>,
    /// Runtime-only GhastShootFireballGoal charge timer.
    #[serde(skip)]
    pub ghast_attack: Option<EntityGhastAttackState>,
    /// Runtime-only Breeze Shoot behavior state.
    #[serde(skip)]
    pub breeze_attack: Option<EntityBreezeAttackState>,
    /// Runtime-only Witch ranged-attack cadence.
    #[serde(skip)]
    pub witch_attack: Option<EntityWitchAttackState>,
    #[serde(default)]
    pub witch_potion: Option<EntityWitchPotionKind>,
    /// Runtime-only Ender Dragon D1 air-combat owner state.
    #[serde(skip)]
    pub dragon_air: Option<dragon_26_1_2::DragonAirState>,
    /// Runtime-only DragonFireball area-effect cloud lifecycle.
    #[serde(skip)]
    pub dragon_breath_cloud: Option<EntityDragonBreathCloudState>,
    /// Runtime attack-goal state. Vanilla does not persist the active beam target.
    #[serde(skip)]
    pub guardian_beam: Option<EntityGuardianBeamState>,
    /// Runtime-only Warden sonic-boom behavior state; Brain memories are not entity NBT here.
    #[serde(skip)]
    pub warden_sonic_boom: Option<EntityWardenSonicBoomState>,
    /// Runtime-only Shulker ranged-attack cadence.
    #[serde(skip)]
    pub shulker_attack: Option<EntityShulkerAttackState>,
    #[serde(default)]
    pub shulker_bullet: Option<EntityShulkerBulletState>,
    /// Runtime-only Evoker fangs spell cadence.
    #[serde(skip)]
    pub evoker_attack: Option<EntityEvokerAttackState>,
    #[serde(default)]
    pub evoker_fangs: Option<EntityEvokerFangState>,
    #[serde(default)]
    pub villager: Option<VillagerData>,
    #[serde(default)]
    pub villager_brain: Option<villager_26_1_2::VillagerBrainState>,
    #[serde(default)]
    pub villager_gossip: Option<villager_gossip_26_1_2::VillagerGossipState>,
    #[serde(default)]
    pub villager_merchant: Option<villager_merchant_26_1_2::VillagerMerchantState>,
    #[serde(default)]
    pub villager_population: Option<villager_population_26_1_2::VillagerPopulationState>,
    #[serde(default)]
    pub zombie_villager_conversion: Option<zombie_villager_26_1_2::ZombieVillagerConversionState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntityCrossbowAttackPhase {
    Aiming,
    Charging,
    Charged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityCrossbowAttackState {
    pub phase: EntityCrossbowAttackPhase,
    pub deadline_tick: u64,
}

impl EntityCrossbowAttackState {
    #[must_use]
    pub const fn new(phase: EntityCrossbowAttackPhase, deadline_tick: u64) -> Self {
        Self {
            phase,
            deadline_tick,
        }
    }

    #[must_use]
    pub const fn is_charging(self) -> bool {
        matches!(self.phase, EntityCrossbowAttackPhase::Charging)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityBowAttackPhase {
    Drawing,
    Cooldown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityBowAttackState {
    pub phase: EntityBowAttackPhase,
    pub deadline_tick: u64,
}

impl EntityBowAttackState {
    #[must_use]
    pub const fn new(phase: EntityBowAttackPhase, deadline_tick: u64) -> Self {
        Self {
            phase,
            deadline_tick,
        }
    }

    #[must_use]
    pub const fn is_drawing(self) -> bool {
        matches!(self.phase, EntityBowAttackPhase::Drawing)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityBlazeAttackState {
    pub attack_step: u8,
    pub deadline_tick: u64,
}

impl EntityBlazeAttackState {
    #[must_use]
    pub const fn new(attack_step: u8, deadline_tick: u64) -> Self {
        Self {
            attack_step,
            deadline_tick,
        }
    }

    #[must_use]
    pub const fn is_charged(self) -> bool {
        self.attack_step != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityGhastAttackState {
    pub charge_time: i32,
}

impl EntityGhastAttackState {
    #[must_use]
    pub const fn new(charge_time: i32) -> Self {
        Self { charge_time }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityBreezeAttackPhase {
    Charging,
    Recovery,
    Cooldown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityBreezeAttackState {
    pub phase: EntityBreezeAttackPhase,
    pub target_session: u64,
    pub target_entity_id: i32,
    pub deadline_tick: u64,
}

impl EntityBreezeAttackState {
    #[must_use]
    pub const fn new(
        phase: EntityBreezeAttackPhase,
        target_session: u64,
        target_entity_id: i32,
        deadline_tick: u64,
    ) -> Self {
        Self {
            phase,
            target_session,
            target_entity_id,
            deadline_tick,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityWitchAttackState {
    pub target_session: u64,
    pub target_entity_id: i32,
    pub deadline_tick: u64,
}

impl EntityWitchAttackState {
    #[must_use]
    pub const fn new(target_session: u64, target_entity_id: i32, deadline_tick: u64) -> Self {
        Self {
            target_session,
            target_entity_id,
            deadline_tick,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntityWitchPotionKind {
    Harming,
    Slowness,
    Poison,
    Weakness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityDragonBreathCloudVictim {
    pub session_id: u64,
    pub next_apply_tick: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntityDragonBreathCloudState {
    pub owner_entity_id: i32,
    pub age_ticks: u32,
    pub duration_ticks: u32,
    pub radius: f32,
    pub radius_per_tick: f32,
    pub amplifier: u8,
    pub reapplication_delay_ticks: u32,
    pub victims: Vec<EntityDragonBreathCloudVictim>,
}

impl EntityDragonBreathCloudState {
    #[must_use]
    pub fn dragon_fireball(owner_entity_id: i32) -> Self {
        Self {
            owner_entity_id,
            age_ticks: 0,
            duration_ticks: 600,
            radius: 3.0,
            radius_per_tick: (7.0 - 3.0) / 600.0,
            amplifier: 1,
            reapplication_delay_ticks: 20,
            victims: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityGuardianBeamPhase {
    Warmup,
    Beam,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityGuardianBeamState {
    pub phase: EntityGuardianBeamPhase,
    pub target_session: u64,
    pub target_entity_id: i32,
    pub deadline_tick: u64,
}

impl EntityGuardianBeamState {
    #[must_use]
    pub const fn new(
        phase: EntityGuardianBeamPhase,
        target_session: u64,
        target_entity_id: i32,
        deadline_tick: u64,
    ) -> Self {
        Self {
            phase,
            target_session,
            target_entity_id,
            deadline_tick,
        }
    }

    #[must_use]
    pub const fn active_target_entity_id(self) -> i32 {
        match self.phase {
            EntityGuardianBeamPhase::Warmup => 0,
            EntityGuardianBeamPhase::Beam => self.target_entity_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityWardenSonicBoomPhase {
    Charging,
    Recovery,
    Cooldown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityWardenSonicBoomState {
    pub phase: EntityWardenSonicBoomPhase,
    pub target_session: u64,
    pub target_entity_id: i32,
    pub deadline_tick: u64,
}

impl EntityWardenSonicBoomState {
    #[must_use]
    pub const fn new(
        phase: EntityWardenSonicBoomPhase,
        target_session: u64,
        target_entity_id: i32,
        deadline_tick: u64,
    ) -> Self {
        Self {
            phase,
            target_session,
            target_entity_id,
            deadline_tick,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityShulkerAttackState {
    pub deadline_tick: u64,
}

impl EntityShulkerAttackState {
    #[must_use]
    pub const fn new(deadline_tick: u64) -> Self {
        Self { deadline_tick }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityShulkerBulletState {
    pub target_entity_id: i32,
}

impl EntityShulkerBulletState {
    #[must_use]
    pub const fn new(target_entity_id: i32) -> Self {
        Self { target_entity_id }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityEvokerAttackPhase {
    Warmup,
    Casting,
    Cooldown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityEvokerAttackState {
    pub phase: EntityEvokerAttackPhase,
    pub target_session: u64,
    pub target_entity_id: i32,
    pub deadline_tick: u64,
}

impl EntityEvokerAttackState {
    #[must_use]
    pub const fn new(
        phase: EntityEvokerAttackPhase,
        target_session: u64,
        target_entity_id: i32,
        deadline_tick: u64,
    ) -> Self {
        Self {
            phase,
            target_session,
            target_entity_id,
            deadline_tick,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityEvokerFangState {
    pub owner_entity_id: i32,
    pub warmup_delay_ticks: i32,
    pub life_ticks: i32,
    pub sent_spike_event: bool,
}

impl EntityEvokerFangState {
    #[must_use]
    pub const fn new(owner_entity_id: i32, warmup_delay_ticks: i32) -> Self {
        Self {
            owner_entity_id,
            warmup_delay_ticks,
            life_ticks: 22,
            sent_spike_event: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VillagerData {
    pub kind: VillagerKind,
    pub profession: VillagerProfession,
    pub level: u8,
}

impl VillagerData {
    #[must_use]
    pub fn new(kind: VillagerKind, profession: VillagerProfession, level: u8) -> Self {
        Self {
            kind,
            profession,
            level: level.clamp(1, 5),
        }
    }
}

/// `VillagerType`, limited to the types the content cache's village pools
/// author: the five village structures' biome types. The variant order is not
/// the wire order — [`crate::VillagerKind`]'s protocol ids come from the
/// registry report (`desert` 0, `jungle` 1, `plains` 2, `savanna` 3, `snow` 4,
/// `swamp` 5, `taiga` 6; jungle and swamp have no village type here).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VillagerKind {
    Desert,
    Plains,
    Savanna,
    Snow,
    Taiga,
}

/// `VillagerProfession` in the bundled 26.1.2 `villager_profession` registry
/// order. Protocol ids come from the registry report: `none` 0, `armorer` 1,
/// `butcher` 2, `cartographer` 3, `cleric` 4, `farmer` 5, `fisherman` 6,
/// `fletcher` 7, `leatherworker` 8, `librarian` 9, `mason` 10, `nitwit` 11,
/// `shepherd` 12, `toolsmith` 13, `weaponsmith` 14.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VillagerProfession {
    None,
    Armorer,
    Butcher,
    Cartographer,
    Cleric,
    Farmer,
    Fisherman,
    Fletcher,
    Leatherworker,
    Librarian,
    Mason,
    Nitwit,
    Shepherd,
    Toolsmith,
    Weaponsmith,
}

impl VillagerProfession {
    /// Registry-report id sent in the `VillagerData` entity metadata.
    #[must_use]
    pub const fn profession_id(self) -> i32 {
        match self {
            Self::None => 0,
            Self::Armorer => 1,
            Self::Butcher => 2,
            Self::Cartographer => 3,
            Self::Cleric => 4,
            Self::Farmer => 5,
            Self::Fisherman => 6,
            Self::Fletcher => 7,
            Self::Leatherworker => 8,
            Self::Librarian => 9,
            Self::Mason => 10,
            Self::Nitwit => 11,
            Self::Shepherd => 12,
            Self::Toolsmith => 13,
            Self::Weaponsmith => 14,
        }
    }

    /// Registry name for a profession, as authored in data tables.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Armorer => "armorer",
            Self::Butcher => "butcher",
            Self::Cartographer => "cartographer",
            Self::Cleric => "cleric",
            Self::Farmer => "farmer",
            Self::Fisherman => "fisherman",
            Self::Fletcher => "fletcher",
            Self::Leatherworker => "leatherworker",
            Self::Librarian => "librarian",
            Self::Mason => "mason",
            Self::Nitwit => "nitwit",
            Self::Shepherd => "shepherd",
            Self::Toolsmith => "toolsmith",
            Self::Weaponsmith => "weaponsmith",
        }
    }

    /// Resolves a registry profession name (`none` included).
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "none" => Some(Self::None),
            "armorer" => Some(Self::Armorer),
            "butcher" => Some(Self::Butcher),
            "cartographer" => Some(Self::Cartographer),
            "cleric" => Some(Self::Cleric),
            "farmer" => Some(Self::Farmer),
            "fisherman" => Some(Self::Fisherman),
            "fletcher" => Some(Self::Fletcher),
            "leatherworker" => Some(Self::Leatherworker),
            "librarian" => Some(Self::Librarian),
            "mason" => Some(Self::Mason),
            "nitwit" => Some(Self::Nitwit),
            "shepherd" => Some(Self::Shepherd),
            "toolsmith" => Some(Self::Toolsmith),
            "weaponsmith" => Some(Self::Weaponsmith),
            _ => None,
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntityExplosionInteraction {
    Mob,
    Trigger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityPendingExplosionState {
    pub expires_tick: u64,
    pub power_bits: u32,
    pub interaction: EntityExplosionInteraction,
    pub damage_entities: bool,
    pub air_block_state: u32,
}

impl EntityPendingExplosionState {
    #[must_use]
    pub fn new(
        expires_tick: u64,
        power: f32,
        interaction: EntityExplosionInteraction,
        damage_entities: bool,
        air_block_state: u32,
    ) -> Option<Self> {
        (power.is_finite() && power > 0.0).then_some(Self {
            expires_tick,
            power_bits: power.to_bits(),
            interaction,
            damage_entities,
            air_block_state,
        })
    }

    #[must_use]
    pub const fn power(self) -> f32 {
        f32::from_bits(self.power_bits)
    }
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
pub(crate) struct EntityKinematicsFenceState {
    pub(crate) uuid: Uuid,
    pub(crate) lifecycle: EntityLifecycle,
    pub(crate) motion: EntityMotionState,
    pub(crate) pickup_claimed: bool,
    pub(crate) vehicle_attached: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityMotionState {
    pub id: EntityId,
    pub position: Vec3,
    pub rotation: Rotation,
    pub velocity: Vec3,
    pub on_ground: bool,
    pub fall_distance: f64,
    pub goal_fence: EntityGoalFence,
    pub is_item: bool,
    pub is_experience: bool,
    pub is_arrow: bool,
    pub arrow_revision: Option<u64>,
    pub arrow_embedded_block: Option<projectile_26_1_2::BlockPosition>,
    pub is_hurting_projectile: bool,
    pub hurting_projectile_revision: Option<u64>,
    pub is_throwable_projectile: bool,
    pub throwable_projectile_revision: Option<u64>,
    pub sends_velocity: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityTrackingMotion {
    pub id: EntityId,
    pub position: Vec3,
    pub rotation: Rotation,
    pub velocity: Vec3,
    pub on_ground: bool,
    pub is_item: bool,
    pub is_experience: bool,
    pub is_arrow: bool,
    pub sends_velocity: bool,
}

impl From<EntityMotionState> for EntityTrackingMotion {
    fn from(motion: EntityMotionState) -> Self {
        Self {
            id: motion.id,
            position: motion.position,
            rotation: motion.rotation,
            velocity: motion.velocity,
            on_ground: motion.on_ground,
            is_item: motion.is_item,
            is_experience: motion.is_experience,
            is_arrow: motion.is_arrow,
            sends_velocity: motion.sends_velocity,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityPhysicsStep {
    pub id: EntityId,
    pub position: Vec3,
    pub velocity: Vec3,
    pub on_ground: bool,
    pub horizontal_collision: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct EntityPhysicsQuery {
    pub id: EntityId,
    pub position: Vec3,
    pub velocity: Vec3,
    pub aabb: mc_physics::Aabb,
    pub on_ground: bool,
    pub fall_distance: f64,
    pub goal_fence: EntityGoalFence,
    pub kind: EntityPhysicsKind,
}

impl EntityPhysicsQuery {
    #[must_use]
    pub fn matches_motion(self, current: EntityMotionState) -> bool {
        let projectile_state_matches = match self.kind {
            EntityPhysicsKind::ArrowProjectile {
                revision,
                embedded_block,
            } => {
                current.is_arrow
                    && current.arrow_revision == revision
                    && current.arrow_embedded_block == embedded_block
            }
            EntityPhysicsKind::HurtingProjectile { revision, .. }
            | EntityPhysicsKind::ShulkerBullet { revision } => {
                current.is_hurting_projectile && current.hurting_projectile_revision == revision
            }
            EntityPhysicsKind::ThrowableProjectile { revision, .. } => {
                current.is_throwable_projectile && current.throwable_projectile_revision == revision
            }
            EntityPhysicsKind::Default
            | EntityPhysicsKind::Immobile
            | EntityPhysicsKind::ExternalFlight
            | EntityPhysicsKind::Living
            | EntityPhysicsKind::PowderSnowWalkableLiving
            | EntityPhysicsKind::FishLiving
            | EntityPhysicsKind::SquidLiving
            | EntityPhysicsKind::FallingBlock
            | EntityPhysicsKind::Item
            | EntityPhysicsKind::AquaticLiving => true,
        };
        current.position == self.position
            && current.velocity == self.velocity
            && current.on_ground == self.on_ground
            && current.fall_distance == self.fall_distance
            && current.goal_fence == self.goal_fence
            && projectile_state_matches
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EntitySimulationResult {
    pub physics: EntityPhysicsQuery,
    pub hostile: bool,
    pub rotation: Rotation,
    pub villager_population_active: bool,
    pub villager: bool,
    pub item: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityPhysicsKind {
    Default,
    Immobile,
    ExternalFlight,
    Living,
    PowderSnowWalkableLiving,
    FishLiving,
    SquidLiving,
    AquaticLiving,
    FallingBlock,
    /// Dropped item stacks (`minecraft:item`). Vanilla `ItemEntity` runs
    /// its own gentle gravity/float regime, distinct from both living
    /// bodies and falling blocks.
    Item,
    ArrowProjectile {
        revision: Option<u64>,
        embedded_block: Option<projectile_26_1_2::BlockPosition>,
    },
    ShulkerBullet {
        revision: Option<u64>,
    },
    HurtingProjectile {
        revision: Option<u64>,
        acceleration_power_bits: u64,
    },
    ThrowableProjectile {
        revision: Option<u64>,
        gravity_bits: u64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntityGoalCheckpoint {
    pub(crate) id: EntityId,
    pub(crate) position: Vec3,
    pub(crate) rotation: Rotation,
    pub(crate) velocity: Vec3,
    pub(crate) on_ground: bool,
    pub(crate) lifecycle: EntityLifecycle,
    pub(crate) goal: GoalState,
    pub(crate) path: RetainedPathState,
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
    pub villager_gossip_event: Option<villager_gossip_26_1_2::VillagerGossipEvent>,
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
    Scale,
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
            Self::Scale => "minecraft:scale",
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
        attrs.set_base(
            AttributeKind::Scale,
            f64::from(EntityScale26_1_2::DEFAULT.factor()),
        );
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

impl EntitySnapshot {
    /// Returns the effective live `minecraft:scale` carried by this snapshot.
    #[must_use]
    pub fn scale_26_1_2(&self) -> EntityScale26_1_2 {
        EntityScale26_1_2::from_attribute_value(self.attributes.base(&AttributeKind::Scale))
    }

    /// Updates the snapshot projection used by the existing owner CAS path.
    pub fn set_scale_26_1_2(&mut self, scale: EntityScale26_1_2) {
        self.attributes
            .set_base(AttributeKind::Scale, f64::from(scale.factor()));
    }
}

impl EntityView<'_> {
    /// Returns the effective live `minecraft:scale` carried by this ECS view.
    #[must_use]
    pub fn scale_26_1_2(&self) -> EntityScale26_1_2 {
        EntityScale26_1_2::from_attribute_value(self.attributes.base(&AttributeKind::Scale))
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
pub enum EntityGoalFence {
    Idle,
    Wander {
        speed_bits: u64,
        period_ticks: u32,
    },
    AquaticWander {
        speed_bits: u64,
        vertical_speed_bits: u64,
        period_ticks: u32,
    },
    FollowTarget {
        target: EntityId,
        speed_bits: u64,
    },
    FollowPosition {
        x_bits: u64,
        y_bits: u64,
        z_bits: u64,
        speed_bits: u64,
    },
}

impl EntityGoalFence {
    #[must_use]
    pub fn from_goal(goal: &GoalState) -> Self {
        match goal {
            GoalState::Idle => Self::Idle,
            GoalState::Wander {
                speed,
                period_ticks,
            } => Self::Wander {
                speed_bits: speed.to_bits(),
                period_ticks: *period_ticks,
            },
            GoalState::AquaticWander {
                speed,
                vertical_speed,
                period_ticks,
            } => Self::AquaticWander {
                speed_bits: speed.to_bits(),
                vertical_speed_bits: vertical_speed.to_bits(),
                period_ticks: *period_ticks,
            },
            GoalState::FollowTarget { target, speed } => Self::FollowTarget {
                target: *target,
                speed_bits: speed.to_bits(),
            },
            GoalState::FollowPosition { target, speed } => Self::FollowPosition {
                x_bits: target.x.to_bits(),
                y_bits: target.y.to_bits(),
                z_bits: target.z.to_bits(),
                speed_bits: speed.to_bits(),
            },
        }
    }
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

    /// Water-only, collision-free occupancy for aquatic navigation. Unknown
    /// terrain must not silently permit a swimmer to leave the water.
    fn can_entity_swim_at(&self, _entity_id: EntityId, _position: Vec3) -> PathingProbeResult {
        PathingProbeResult::Unloaded
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
/// Wander reach, in blocks: every rolled target sits `MIN..MIN + SPREAD` away
/// from the agent's current position.
///
/// Deliberately long-range: the owner asked for a world that is visibly alive
/// and in motion, which a vanilla-sized stroll (3..7 blocks) does not deliver.
/// Because a target is rolled relative to the *current* position there is no
/// home leash, so a longer reach becomes real roaming instead of a wider idle.
/// The reach stays bounded so a stroll remains a stroll, and it costs ticks
/// rather than work: pathing is a per-tick greedy step under `PathingBudget`,
/// so the probe cost per tick is unchanged, and a target that turns out to be
/// unreachable (terrain, unloaded chunk, water edge) is abandoned by the
/// retained-path no-progress budget and re-rolled on the next epoch.
const WANDER_MIN_DISTANCE: f64 = 6.0;
const WANDER_DISTANCE_SPREAD: f64 = 26.0;

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
    #[serde(default)]
    target_reached: bool,
    #[serde(default)]
    resume_tick: u64,
    #[serde(default)]
    swim_speed: f32,
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
            target_reached: false,
            resume_tick: 0,
            swim_speed: 0.0,
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
    aquatic: Option<aquatic_motion::Swimmer>,
}

impl GoalPathingRequest {
    fn into_checkpoint(self) -> EntityGoalCheckpoint {
        EntityGoalCheckpoint {
            id: self.id,
            position: self.expected_position,
            rotation: self.expected_rotation,
            velocity: self.expected_velocity,
            on_ground: self.expected_on_ground,
            lifecycle: EntityLifecycle::Alive,
            goal: self.expected_goal,
            path: self.expected_path,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct GoalPathingResult {
    request: Option<GoalPathingRequest>,
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
        self.request.as_ref().is_none_or(|request| {
            request.expected_position == position
                && request.expected_rotation == rotation
                && request.expected_velocity == velocity
                && request.expected_on_ground == on_ground
                && &request.expected_goal == goal
                && &request.expected_path == path
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntityVillagerGoalUpdate {
    pub(crate) expected: EntitySnapshot,
    pub(crate) villager: VillagerData,
    pub(crate) brain: villager_26_1_2::VillagerBrainState,
    pub(crate) gossip: Option<villager_gossip_26_1_2::VillagerGossipState>,
    pub(crate) merchant: Option<villager_merchant_26_1_2::VillagerMerchantState>,
}

#[derive(Debug)]
pub(crate) struct EntityGoalTickSelection {
    pub checkpoints: Vec<EntityGoalCheckpoint>,
    pub goal_tick: PreparedGoalTick,
    pub goal_overrides: HashMap<EntityId, GoalState>,
    pub pathing_aabbs: Vec<(EntityId, mc_physics::Aabb)>,
    pub snapshot_overrides: Vec<(EntitySnapshot, EntitySnapshot)>,
    pub cross_region_villager_candidates: Vec<EntityId>,
    pub villager_updates: Vec<EntityVillagerGoalUpdate>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreparedGoalTick {
    tick: u64,
    active_ids: Option<HashSet<EntityId>>,
    passive_decisions: usize,
    pathing_requests: Vec<GoalPathingRequest>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedGoalTick {
    tick: u64,
    active_ids: Option<HashSet<EntityId>>,
    passive_decisions: usize,
    pathing_results: HashMap<EntityId, GoalPathingResult>,
}

impl PreparedGoalTick {
    #[must_use]
    pub fn pathing_request_count(&self) -> usize {
        self.pathing_requests.len()
    }
    pub(crate) fn pathing_request_ids(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.pathing_requests.iter().map(|request| request.id)
    }

    pub fn visit_pathing_probe_positions(
        &self,
        budget: PathingBudget,
        mut visitor: impl FnMut(EntityId, Vec3),
    ) {
        for request in &self.pathing_requests {
            if request.aquatic.is_some() {
                aquatic_motion::visit_probe_positions(request, self.tick, budget, |position| {
                    visitor(request.id, position);
                });
                continue;
            }
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
        self.resolve_inner(probe, budget, false, false).0
    }

    fn resolve_for_regional(
        self,
        probe: &dyn PathingProbe,
        budget: PathingBudget,
    ) -> (ResolvedGoalTick, Vec<EntityGoalCheckpoint>) {
        self.resolve_inner(probe, budget, true, true)
    }
    fn resolve_for_owner(
        self,
        probe: &dyn PathingProbe,
        budget: PathingBudget,
    ) -> ResolvedGoalTick {
        self.resolve_inner(probe, budget, true, false).0
    }

    fn resolve_inner(
        self,
        probe: &dyn PathingProbe,
        budget: PathingBudget,
        trust_owner_fence: bool,
        collect_checkpoints: bool,
    ) -> (ResolvedGoalTick, Vec<EntityGoalCheckpoint>) {
        let tick = self.tick;
        let mut pathing_results = HashMap::with_capacity(self.pathing_requests.len());
        let mut checkpoints = Vec::with_capacity(if collect_checkpoints {
            self.pathing_requests.len()
        } else {
            0
        });
        for request in self.pathing_requests {
            let id = request.id;
            let (decision, next_path) = resolve_retained_pathing(&request, tick, probe, budget);
            let request = if trust_owner_fence {
                if collect_checkpoints {
                    checkpoints.push(request.into_checkpoint());
                }
                None
            } else {
                Some(request)
            };
            pathing_results.insert(
                id,
                GoalPathingResult {
                    request,
                    decision,
                    next_path,
                },
            );
        }
        (
            ResolvedGoalTick {
                tick: self.tick,
                active_ids: self.active_ids,
                passive_decisions: self.passive_decisions,
                pathing_results,
            },
            checkpoints,
        )
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

    pub(crate) fn convert_snapshot_in_place(&mut self, snapshot: EntitySnapshot) -> bool {
        self.runtime.convert_snapshot_in_place(snapshot)
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

    pub(crate) fn goal_checkpoints_for_ids(
        &self,
        ids: &HashSet<EntityId>,
    ) -> Vec<EntityGoalCheckpoint> {
        self.runtime.goal_checkpoints_for_ids(ids)
    }

    pub(crate) fn restore_goal_checkpoints(
        &mut self,
        checkpoints: Vec<EntityGoalCheckpoint>,
    ) -> bool {
        checkpoints
            .into_iter()
            .all(|checkpoint| self.runtime.restore_goal_checkpoint(checkpoint))
    }

    pub fn simulation_results_for_ids(
        &self,
        ids: &HashSet<EntityId>,
    ) -> Vec<EntitySimulationResult> {
        let mut ordered_ids = ids.iter().copied().collect::<Vec<_>>();
        ordered_ids.sort_unstable();
        ordered_ids
            .into_iter()
            .filter_map(|id| self.runtime.simulation_result(id))
            .collect()
    }

    pub fn alive_kinematics_for_ids(&mut self, ids: &HashSet<EntityId>) -> Vec<EntityKinematics> {
        self.runtime.alive_kinematics_for_ids(ids)
    }

    pub(crate) fn kinematics_fence_states(
        &self,
        ids: &HashSet<EntityId>,
    ) -> HashMap<EntityId, EntityKinematicsFenceState> {
        self.runtime.kinematics_fence_states(ids)
    }

    pub(crate) fn visit_simulation_fence_results_for_ordered_ids(
        &self,
        ids: &[EntityId],
        visitor: impl FnMut(EntityKinematicsFenceState, Option<EntitySimulationResult>),
    ) {
        self.runtime
            .visit_simulation_fence_results_for_ordered_ids(ids, visitor);
    }

    pub(crate) fn visit_simulation_fence_results(
        &mut self,
        visitor: impl FnMut(EntityKinematicsFenceState, Option<EntitySimulationResult>),
    ) {
        self.runtime.visit_simulation_fence_results(visitor);
    }
    pub(crate) fn visit_goal_tick_candidates(
        &mut self,
        visitor: impl FnMut(EntityKinematics, EntityLifecycle, bool, bool, Option<Vec3>),
    ) {
        self.runtime.visit_goal_tick_candidates(visitor);
    }

    pub(crate) fn kinematics_fence_state(
        &self,
        id: EntityId,
    ) -> Option<EntityKinematicsFenceState> {
        self.runtime.kinematics_fence_state(id)
    }
    pub(crate) fn passenger_ids(&self) -> HashSet<EntityId> {
        self.runtime.passenger_ids()
    }

    #[must_use]
    pub fn view(&self, id: EntityId) -> Option<EntityView<'_>> {
        self.runtime.view(id)
    }

    pub(crate) fn sheep_grazing_activity(&self, id: EntityId) -> Option<bool> {
        self.runtime.sheep_grazing_activity(id)
    }

    pub fn snapshots(&self) -> impl Iterator<Item = EntitySnapshot> + '_ {
        let snapshots = self.runtime.normalized_snapshots();
        snapshots.into_iter()
    }

    pub(crate) fn goal_tick_selection(
        &self,
        region: regional::RegionKey,
        tick: u64,
        candidate_ids: &HashSet<EntityId>,
        inputs: &regional::RegionalGoalTickInputs,
    ) -> EntityGoalTickSelection {
        self.runtime
            .goal_tick_selection(region, tick, candidate_ids, inputs)
    }
    pub(crate) fn goal_tick_selection_for_ordered_ids(
        &self,
        region: regional::RegionKey,
        tick: u64,
        candidate_ids: &[EntityId],
        inputs: &regional::RegionalGoalTickInputs,
    ) -> EntityGoalTickSelection {
        self.runtime
            .goal_tick_selection_for_ordered_ids(region, tick, candidate_ids, inputs)
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
        let applied = self.runtime.queue_kinematics(states);
        if applied > 0 {
            self.runtime.run_stage(EntityStage::PhysicsApply);
        }
        applied
    }

    /// Applies states already fenced by the owning region transaction.
    pub(crate) fn apply_kinematics_prevalidated(
        &mut self,
        states: impl IntoIterator<Item = EntityKinematics>,
    ) -> usize {
        let applied = self.runtime.queue_kinematics_prevalidated(states);
        if applied > 0 {
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
        self.prepare_goal_tick(tick, Some(active_ids), &HashMap::new())
    }

    fn prepare_goal_tick(
        &mut self,
        tick: u64,
        active_ids: Option<&HashSet<EntityId>>,
        goal_overrides: &HashMap<EntityId, GoalState>,
    ) -> PreparedGoalTick {
        let active_ids = active_ids.filter(|active_ids| !self.runtime.ids_cover_world(active_ids));
        let mut pathing_requests = Vec::new();

        pathing_requests.extend(
            self.runtime
                .pathing_requests(tick, active_ids, goal_overrides),
        );
        PreparedGoalTick {
            tick,
            active_ids: active_ids.cloned(),
            passive_decisions: 0,
            pathing_requests,
        }
    }

    pub fn apply_prepared_goal_tick(&mut self, resolved: ResolvedGoalTick) -> GoalTickStats {
        let ResolvedGoalTick {
            tick,
            active_ids,
            passive_decisions,
            pathing_results,
        } = resolved;
        self.apply_goal_tick(
            tick,
            true,
            pathing_results,
            active_ids.as_ref(),
            passive_decisions,
            None,
            None,
        )
        .0
    }

    pub(crate) fn apply_prepared_goal_tick_with_follow_targets(
        &mut self,
        resolved: ResolvedGoalTick,
        follow_targets: &HashMap<EntityId, Vec3>,
    ) -> GoalTickStats {
        self.apply_prepared_goal_tick_with_follow_targets_inner(resolved, follow_targets, None)
            .0
    }

    pub(crate) fn apply_prepared_goal_tick_with_follow_targets_and_simulation_results(
        &mut self,
        resolved: ResolvedGoalTick,
        follow_targets: &HashMap<EntityId, Vec3>,
        simulation_capture_ids: Vec<EntityId>,
    ) -> (GoalTickStats, Vec<runtime::GoalSimulationCandidate>) {
        self.apply_prepared_goal_tick_with_follow_targets_inner(
            resolved,
            follow_targets,
            Some(simulation_capture_ids),
        )
    }

    pub(crate) fn apply_prepared_owner_goal_tick(
        &mut self,
        resolved: ResolvedGoalTick,
        follow_targets: HashMap<EntityId, Vec3>,
        simulation_capture_ids: Vec<EntityId>,
    ) -> (GoalTickStats, runtime::OwnerGoalTickOutput) {
        let ResolvedGoalTick {
            tick,
            active_ids,
            passive_decisions,
            pathing_results,
        } = resolved;
        self.runtime.run_owner_goal_tick(runtime::GoalTickRequest {
            tick,
            pathing_enabled: true,
            pathing: pathing_results,
            active_ids,
            passive_decisions,
            external_follow_targets: follow_targets,
            external_follow_targets_complete: true,
            simulation_capture_ids: Some(simulation_capture_ids),
        })
    }

    fn apply_prepared_goal_tick_with_follow_targets_inner(
        &mut self,
        resolved: ResolvedGoalTick,
        follow_targets: &HashMap<EntityId, Vec3>,
        simulation_capture_ids: Option<Vec<EntityId>>,
    ) -> (GoalTickStats, Vec<runtime::GoalSimulationCandidate>) {
        let ResolvedGoalTick {
            tick,
            active_ids,
            passive_decisions,
            pathing_results,
        } = resolved;
        self.apply_goal_tick(
            tick,
            true,
            pathing_results,
            active_ids.as_ref(),
            passive_decisions,
            Some(follow_targets),
            simulation_capture_ids,
        )
    }

    fn tick_goals_internal(
        &mut self,
        tick: u64,
        pathing: Option<(&dyn PathingProbe, PathingBudget)>,
        active_ids: Option<&HashSet<EntityId>>,
    ) -> GoalTickStats {
        if let Some((probe, budget)) = pathing {
            let prepared = self.prepare_goal_tick(tick, active_ids, &HashMap::new());
            return self.apply_prepared_goal_tick(prepared.resolve(probe, budget));
        }
        self.apply_goal_tick(tick, false, HashMap::new(), active_ids, 0, None, None)
            .0
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_goal_tick(
        &mut self,
        tick: u64,
        pathing_enabled: bool,
        pathing_results: HashMap<EntityId, GoalPathingResult>,
        active_ids: Option<&HashSet<EntityId>>,
        passive_decisions: usize,
        external_follow_targets: Option<&HashMap<EntityId, Vec3>>,
        simulation_capture_ids: Option<Vec<EntityId>>,
    ) -> (GoalTickStats, Vec<runtime::GoalSimulationCandidate>) {
        self.runtime.run_goal_tick(runtime::GoalTickRequest {
            tick,
            pathing_enabled,
            pathing: pathing_results,
            active_ids: active_ids.cloned(),
            passive_decisions,
            external_follow_targets: external_follow_targets.cloned().unwrap_or_default(),
            external_follow_targets_complete: external_follow_targets.is_some(),
            simulation_capture_ids,
        })
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
    tick: u64,
    probe: &dyn PathingProbe,
    budget: PathingBudget,
) -> (PathingDecision, RetainedPathState) {
    if request.aquatic.is_some() {
        return aquatic_motion::resolve(request, tick, probe, budget);
    }
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
        path.target_reached = false;
        path.resume_tick = 0;
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
        if let GoalState::Wander { period_ticks, .. } = &request.expected_goal
            && !path.target_reached
        {
            path.target_reached = true;
            path.resume_tick = tick.saturating_add(wander_pause_ticks(
                request.id,
                request.target_epoch.unwrap_or_default(),
                *period_ticks,
            ));
        }
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

    let step_height = budget.step_height.max(0.0);
    let direct_position = Vec3 {
        x: current.x + direct.x * speed * PathingBudget::TICK_SECONDS,
        y: current.y,
        z: current.z + direct.z * speed * PathingBudget::TICK_SECONDS,
    };
    let Some(direct_result) = probes.call(entity_id, direct_position) else {
        return PathingDecision {
            velocity: Vec3::ZERO,
            kind: PathingDecisionKind::Blocked,
            direct: false,
        };
    };
    let mut saw_unloaded = false;
    let direct_velocity = match direct_result {
        PathingProbeResult::Walkable => Some(direct),
        PathingProbeResult::Blocked => {
            let stepped = Vec3 {
                y: direct_position.y + step_height,
                ..direct_position
            };
            match probes.call(entity_id, stepped) {
                Some(PathingProbeResult::Walkable) => Some(Vec3 {
                    y: step_height,
                    ..direct
                }),
                Some(PathingProbeResult::Unloaded) => {
                    saw_unloaded = true;
                    None
                }
                Some(PathingProbeResult::Blocked) | None => None,
            }
        }
        PathingProbeResult::Unloaded => {
            saw_unloaded = true;
            None
        }
    };
    if let Some(velocity) = direct_velocity {
        probes.direct_path_resolved(entity_id);
        return PathingDecision {
            velocity,
            kind: PathingDecisionKind::Move,
            direct: true,
        };
    }

    let (candidates, limit) = bounded_pathing_candidates(current, direct, speed, budget);
    // An entity already overlapping terrain cannot make target progress but must
    // still be allowed to step out; probe its own position once for that.
    let overlapping = matches!(
        probes.call(entity_id, current),
        Some(PathingProbeResult::Blocked)
    );
    let mut best: Option<(f64, Vec3)> = None;
    let candidate_limit = if allow_detours { limit } else { limit.min(1) };
    for candidate in candidates.into_iter().take(candidate_limit).skip(1) {
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
                    y: flat.y + step_height,
                    ..flat
                };
                let Some(stepped_result) = probes.call(entity_id, stepped) else {
                    break;
                };
                match stepped_result {
                    PathingProbeResult::Walkable => Some(Vec3 {
                        x: candidate.direction.x,
                        y: step_height,
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

    // A detour is only worth taking if it actually brings the entity closer to
    // its target. Accepting the least-bad candidate when every candidate is
    // farther away makes an unreachable target (e.g. a wander target buried in
    // tree leaves) walk the entity away and back forever, which reads in game
    // as spinning in place. An entity already overlapping terrain still needs
    // its escape step, so that case keeps the least-bad candidate.
    let current_distance = goal.horizontal_len();
    // An entity whose body already overlaps terrain cannot make progress with
    // half-step probes that stay inside the overlapped block. Try one full-block
    // step toward the four cardinal directions so it can actually leave.
    if overlapping && allow_detours && best.is_none() {
        let side = Vec3 {
            x: -direct.z,
            y: 0.0,
            z: direct.x,
        };
        let escapes = [
            direct,
            side,
            Vec3 {
                x: -side.x,
                y: 0.0,
                z: -side.z,
            },
            Vec3 {
                x: -direct.x,
                y: 0.0,
                z: -direct.z,
            },
        ];
        for escape in escapes {
            for dy in [0.0, -1.0] {
                let position = Vec3 {
                    x: current.x + escape.x,
                    y: current.y + dy,
                    z: current.z + escape.z,
                };
                match probes.call(entity_id, position) {
                    Some(PathingProbeResult::Walkable) => {
                        best = Some((current_distance, escape));
                        break;
                    }
                    Some(PathingProbeResult::Unloaded) => saw_unloaded = true,
                    Some(PathingProbeResult::Blocked) | None => {}
                }
            }
            if best.is_some() {
                break;
            }
        }
    }
    if let Some((score, velocity)) = best
        && (score < current_distance - RETAINED_PATH_PROGRESS_EPSILON || overlapping)
    {
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
    // Resolve probes the entity's own position to detect terrain overlap before
    // accepting a non-progress escape step.
    visitor(current);
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
    // Resolve may fall back to one full-block cardinal escape step when the body
    // overlaps terrain.
    let side = Vec3 {
        x: -direct.z,
        y: 0.0,
        z: direct.x,
    };
    for escape in [
        direct,
        side,
        Vec3 {
            x: -side.x,
            y: 0.0,
            z: -side.z,
        },
        Vec3 {
            x: -direct.x,
            y: 0.0,
            z: -direct.z,
        },
    ] {
        for dy in [0.0, -1.0] {
            visitor(Vec3 {
                x: current.x + escape.x,
                y: current.y + dy,
                z: current.z + escape.z,
            });
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
    _period_ticks: u32,
) -> (Vec3, u64) {
    if path.has_target && !path.stopped && (!path.target_reached || tick < path.resume_tick) {
        return (path.target, path.target_epoch.unwrap_or_default());
    }
    let epoch = path.target_epoch.map_or(0, |epoch| epoch.saturating_add(1));
    let angle = deterministic_angle(id, epoch);
    let distance = WANDER_MIN_DISTANCE
        + deterministic_unit(id, epoch.wrapping_add(0x2d)) * WANDER_DISTANCE_SPREAD;
    (
        Vec3 {
            x: position.x + angle.cos() * distance,
            y: position.y,
            z: position.z + angle.sin() * distance,
        },
        epoch,
    )
}

fn wander_pause_ticks(id: EntityId, epoch: u64, period_ticks: u32) -> u64 {
    let period = u64::from(period_ticks.clamp(1, 20));
    let minimum = (period / 4).max(1);
    minimum.saturating_add(
        splitmix64((id.0 as u32 as u64) ^ epoch.wrapping_mul(0x94d0_49bb_1331_11eb)) % period,
    )
}

fn deterministic_unit(id: EntityId, phase: u64) -> f64 {
    let mixed = splitmix64((id.0 as u32 as u64) ^ phase.wrapping_mul(0xbf58_476d_1ce4_e5b9));
    mixed as f64 / u64::MAX as f64
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

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
