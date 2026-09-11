use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity as EcsEntity;
use bevy_ecs::query::Without;
use bevy_ecs::resource::Resource;
use bevy_ecs::schedule::{ExecutorKind, IntoScheduleConfigs, Schedule};
use bevy_ecs::world::{EntityRef, World};
use uuid::Uuid;

use crate::effects_26_1_2::{
    ActiveEffects, AddOutcome, EffectAction, EffectId, EffectInstance, EffectLimitError,
    EffectLimits, EffectStoreError, TargetEffectContext,
};
use crate::living_26_1_2::{DamageContext, LivingLifecycle};
use crate::runtime_26_1_2::{
    EffectActionApplyError, PublicationFact, RuntimeScratch, RuntimeState, RuntimeStateError,
    TargetKind, TickInput, TickMode, apply_effect_action, apply_tick, prepare_tick,
};
use crate::{
    AnimalBreedingState, AttributeSet, EntityActiveEffectsState, EntityDamageRequest,
    EntityGoalCheckpoint, EntityId, EntityItemStack, EntityKinematics, EntityKinematicsFenceState,
    EntityLifecycle, EntityLivingRetainedState, EntityMotionState, EntityPhysicsKind,
    EntityPhysicsQuery, EntityRetainedState, EntitySimulationResult, EntitySnapshot,
    EntityTrackingMotion, EntityView, GoalPathingRequest, GoalPathingResult, GoalState,
    GoalTickStats, PathingDecisionKind, RetainedPathState, Rotation, Vec3, VehicleKind,
    VehicleState,
};

mod entity_projections;

#[derive(Component)]
struct StableIdentity {
    id: EntityId,
    uuid: Uuid,
}

#[derive(Component)]
struct EntityTypeState {
    protocol_id: i32,
    name: Arc<str>,
    geometry: crate::natural_spawn_26_1_2::EntityGeometry,
    hostile: bool,
    aquatic_physics: bool,
    powder_snow_walkable: bool,
    villager: bool,
    item: bool,
}

fn entity_type_state(protocol_id: i32, name: impl Into<Arc<str>>) -> EntityTypeState {
    let name = name.into();
    EntityTypeState {
        protocol_id,
        geometry: crate::natural_spawn_26_1_2::entity_geometry(&name, None),
        hostile: crate::natural_spawn_26_1_2::is_hostile_entity(&name),
        aquatic_physics: crate::natural_spawn_26_1_2::entity_type_uses_aquatic_physics(&name),
        powder_snow_walkable: crate::natural_spawn_26_1_2::entity_type_walks_on_powder_snow(&name),
        villager: name.as_ref() == "minecraft:villager",
        item: name.as_ref() == "minecraft:item",
        name,
    }
}

#[derive(Component)]
struct TransformState {
    position: Vec3,
    rotation: Rotation,
}

#[derive(Component)]
struct MotionState {
    velocity: Vec3,
    on_ground: bool,
    fall_distance: f64,
}

#[derive(Component)]
struct LifecycleState(EntityLifecycle);

#[derive(Component)]
struct LivingState {
    state: crate::living_26_1_2::LivingState,
    attributes: AttributeSet,
}

#[derive(Component, Clone)]
struct ActiveEffectsState {
    active: ActiveEffects,
    action_order: Vec<EffectId>,
}

const ENTITY_EFFECT_ACTIVE_CAPACITY: usize = 32;
const ENTITY_EFFECT_HIDDEN_CAPACITY: usize = 128;
const MOB_BODY_YAW_TURN_PER_TICK: f32 = 20.0;
const MOB_HEAD_YAW_TURN_PER_TICK: f32 = 30.0;

#[derive(Debug, Clone, PartialEq)]
pub enum EntityEffectOperation {
    ApplyAction {
        effect_id: EffectId,
        action: EffectAction,
        damage_context: Option<DamageContext>,
    },
    Add(EffectInstance),
    Tick {
        entity_tick_count: i32,
        target_context: TargetEffectContext,
        damage_context: DamageContext,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntityEffectRequest {
    pub operation: EntityEffectOperation,
    pub target_kind: TargetKind,
    pub death_remove_tick: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityEffectRejection {
    Missing,
    Stale,
    NonLiving,
    Dead,
    NonFiniteCurrentHealth,
    InvalidMaxHealth,
    AtMaxHealth,
    NoActiveEffects,
    EffectCapacity,
    InvalidRuntimeState,
    InvalidAction,
    UnresolvedDamageContext,
    TickPreparation,
    TickApply,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntityEffectApplied {
    pub snapshot: EntitySnapshot,
    pub publications: Vec<PublicationFact>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EntityEffectResult {
    Applied(Box<EntityEffectApplied>),
    Rejected(EntityEffectRejection),
}

#[derive(Clone)]
pub(crate) struct EntityEffectCheckpoint {
    entity: EcsEntity,
    living: crate::living_26_1_2::LivingState,
    lifecycle: EntityLifecycle,
    gameplay: GameplayDecisionState,
    effects: Option<ActiveEffectsState>,
}

#[derive(Component)]
struct AiGoalState(GoalState);

#[derive(Component, Default)]
struct AiPathState(RetainedPathState);

#[derive(Component, Clone)]
struct GameplayDecisionState {
    arrow_state: Option<crate::projectile_26_1_2::ArrowState>,
    hurting_projectile_state: Option<crate::projectile_26_1_2::HurtingProjectileState>,
    throwable_projectile_state: Option<crate::projectile_26_1_2::ThrowableState>,
    remaining_fire_ticks: i32,
    last_damage_tick: Option<u64>,
    death_remove_tick: Option<u64>,
    sheep_grazing_ticks: Option<u8>,
    spawn_tick: u64,
    item_pickup_ready_tick: Option<u64>,
    item_pickup_owner_block: Option<crate::EntityItemPickupOwnerBlock>,
    item_pickup_claim: Option<u64>,
    villager_food_recipient: Option<EntityId>,
    primed_tnt: Option<crate::EntityPrimedTntState>,
    pending_explosion: Option<crate::EntityPendingExplosionState>,
    crossbow_attack: Option<crate::EntityCrossbowAttackState>,
    bow_attack: Option<crate::EntityBowAttackState>,
    blaze_attack: Option<crate::EntityBlazeAttackState>,
    ghast_attack: Option<crate::EntityGhastAttackState>,
    breeze_attack: Option<crate::EntityBreezeAttackState>,
    witch_attack: Option<crate::EntityWitchAttackState>,
    witch_potion: Option<crate::EntityWitchPotionKind>,
    dragon_air: Option<crate::dragon_26_1_2::DragonAirState>,
    dragon_breath_cloud: Option<crate::EntityDragonBreathCloudState>,
    guardian_beam: Option<crate::EntityGuardianBeamState>,
    warden_sonic_boom: Option<crate::EntityWardenSonicBoomState>,
    shulker_attack: Option<crate::EntityShulkerAttackState>,
    shulker_bullet: Option<crate::EntityShulkerBulletState>,
    evoker_attack: Option<crate::EntityEvokerAttackState>,
    evoker_fangs: Option<crate::EntityEvokerFangState>,
    villager: Option<crate::VillagerData>,
    villager_brain: Option<crate::villager_26_1_2::VillagerBrainState>,
    villager_gossip: Option<crate::villager_gossip_26_1_2::VillagerGossipState>,
    villager_merchant: Option<crate::villager_merchant_26_1_2::VillagerMerchantState>,
    villager_population: Option<crate::villager_population_26_1_2::VillagerPopulationState>,
    zombie_villager_conversion:
        Option<crate::zombie_villager_26_1_2::ZombieVillagerConversionState>,
}

#[derive(Component)]
struct ItemStackState(EntityItemStack);

#[derive(Component)]
struct ExperienceState(i32);

#[derive(Component)]
struct FallingBlockState(u32);

#[derive(Component)]
struct ProjectileState;

#[derive(Component)]
struct VehicleKindState(VehicleKind);

#[derive(Component)]
struct PassengerState(EntityId);

#[derive(Component)]
struct AnimalState(AnimalBreedingState);

#[derive(Component)]
struct PersistentState;

#[derive(Component)]
struct VisibilityState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityStage {
    InputAi,
    SnapshotRequest,
    PhysicsApply,
    CombatLifecycle,
    PersistenceExtract,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EntityInputCommand {
    Insert(Box<EntitySnapshot>),
    SetGoal {
        id: EntityId,
        goal: GoalState,
    },
    ResetPath {
        id: EntityId,
    },
    SetItemStack {
        id: EntityId,
        stack: Option<EntityItemStack>,
    },
    SetVehicle {
        id: EntityId,
        vehicle: Option<VehicleState>,
    },
    SetAnimalState {
        id: EntityId,
        animal: AnimalBreedingState,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityPhysicsResult {
    pub id: EntityId,
    pub position: Vec3,
    pub rotation: Rotation,
    pub velocity: Vec3,
    pub on_ground: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EntityCombatCommand {
    Damage {
        id: EntityId,
        request: EntityDamageRequest,
    },
    MarkDespawning {
        id: EntityId,
    },
    Remove {
        id: EntityId,
    },
}

#[derive(Resource, Default)]
struct RuntimeEntityIndex(BTreeMap<EntityId, EcsEntity>);

#[derive(Resource, Default)]
struct RuntimeEntityUuids(HashSet<Uuid>);

#[derive(Resource, Default)]
struct BreedingTickEntities(BTreeSet<EntityId>);

#[derive(Resource, Default)]
struct SheepEntities(BTreeSet<EntityId>);

#[derive(Resource, Default)]
struct PendingInputCommands(Vec<EntityInputCommand>);

#[derive(Resource, Default)]
struct PendingPhysicsResults(Vec<EntityPhysicsResult>);

pub(crate) struct GoalTickRequest {
    pub(crate) tick: u64,
    pub(crate) pathing_enabled: bool,
    pub(crate) pathing: HashMap<EntityId, GoalPathingResult>,
    pub(crate) active_ids: Option<HashSet<EntityId>>,
    pub(crate) passive_decisions: usize,
    pub(crate) external_follow_targets: HashMap<EntityId, Vec3>,
    pub(crate) external_follow_targets_complete: bool,
    pub(crate) simulation_capture_ids: Option<Vec<EntityId>>,
}

pub(crate) struct GoalSimulationCandidate {
    pub id: EntityId,
    pub uuid: uuid::Uuid,
    pub lifecycle: EntityLifecycle,
    pub pickup_claimed: bool,
    pub vehicle_attached: bool,
    pub result: Option<EntitySimulationResult>,
}

#[derive(Debug, Default)]
pub(crate) struct OwnerGoalTickOutput {
    pub(crate) captured_count: usize,
    pub(crate) invalid_count: usize,
    pub(crate) active_hostile_ids: Vec<(EntityId, Vec3)>,
    pub(crate) villager_population_candidates: Vec<(EntityId, Vec3)>,
    pub(crate) villager_ids: Vec<(EntityId, Vec3)>,
    pub(crate) villager_proximity_seeds: Vec<(EntityId, Vec3)>,
    pub(crate) goal_committed_motion: Vec<EntityTrackingMotion>,
}

#[derive(Resource, Default)]
struct PendingPositionTick(Option<(f64, Option<HashSet<EntityId>>)>);

#[derive(Resource, Default)]
struct PendingCombatCommands(Vec<EntityCombatCommand>);

#[derive(Resource, Default)]
struct SnapshotRequest(bool);

#[derive(Resource, Default)]
struct SnapshotOutput(Vec<EntitySnapshot>);

#[derive(Resource, Default)]
struct PersistenceExtractRequest(bool);

#[derive(Resource, Default)]
struct PersistenceOutput(Vec<EntitySnapshot>);

struct EntitySchedules {
    input_ai: Schedule,
    snapshot_request: Schedule,
    physics_apply: Schedule,
    combat_lifecycle: Schedule,
    persistence_extract: Schedule,
}

/// ECS representation used for entity authority.
pub struct EntityRuntime {
    world: World,
    schedules: EntitySchedules,
    #[cfg(test)]
    input_ai_stage_runs: usize,
    #[cfg(test)]
    physics_apply_stage_runs: usize,
}

impl fmt::Debug for EntityRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EntityRuntime")
            .field(
                "entities",
                &self.world.resource::<RuntimeEntityIndex>().0.len(),
            )
            .finish_non_exhaustive()
    }
}

impl Default for EntityRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl EntityRuntime {
    #[must_use]
    pub fn new() -> Self {
        let mut world = World::new();
        world.init_resource::<RuntimeEntityIndex>();
        world.init_resource::<RuntimeEntityUuids>();
        world.init_resource::<BreedingTickEntities>();
        world.init_resource::<SheepEntities>();
        world.init_resource::<PendingInputCommands>();
        world.init_resource::<PendingPhysicsResults>();
        world.init_resource::<PendingPositionTick>();
        world.init_resource::<PendingCombatCommands>();
        world.init_resource::<SnapshotRequest>();
        world.init_resource::<SnapshotOutput>();
        world.init_resource::<PersistenceExtractRequest>();
        world.init_resource::<PersistenceOutput>();
        Self {
            world,
            schedules: EntitySchedules::new(),
            #[cfg(test)]
            input_ai_stage_runs: 0,
            #[cfg(test)]
            physics_apply_stage_runs: 0,
        }
    }

    pub fn insert_snapshot(&mut self, snapshot: EntitySnapshot) -> bool {
        insert_snapshot_into_world(&mut self.world, snapshot)
    }

    pub(crate) fn restore_snapshot_in_place(&mut self, snapshot: EntitySnapshot) -> bool {
        restore_snapshot_in_world(&mut self.world, snapshot, false)
    }

    pub(crate) fn convert_snapshot_in_place(&mut self, snapshot: EntitySnapshot) -> bool {
        restore_snapshot_in_world(&mut self.world, snapshot, true)
    }

    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.world.resource::<RuntimeEntityIndex>().0.len()
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.world.resource::<RuntimeEntityIndex>().0.is_empty()
    }

    #[must_use]
    pub fn snapshot(&self, id: EntityId) -> Option<EntitySnapshot> {
        snapshot_from_world(&self.world, id)
    }

    pub(crate) fn effect_checkpoint(&self, id: EntityId) -> Option<EntityEffectCheckpoint> {
        let entity = *self.world.resource::<RuntimeEntityIndex>().0.get(&id)?;
        let row = self.world.get_entity(entity).ok()?;
        Some(EntityEffectCheckpoint {
            entity,
            living: row.get::<LivingState>()?.state,
            lifecycle: row.get::<LifecycleState>()?.0,
            gameplay: row.get::<GameplayDecisionState>()?.clone(),
            effects: row.get::<ActiveEffectsState>().cloned(),
        })
    }

    pub(crate) fn restore_effect_checkpoint(&mut self, checkpoint: EntityEffectCheckpoint) -> bool {
        let Ok(mut row) = self.world.get_entity_mut(checkpoint.entity) else {
            return false;
        };
        {
            let Some(mut living) = row.get_mut::<LivingState>() else {
                return false;
            };
            living.state = checkpoint.living;
        }
        {
            let Some(mut lifecycle) = row.get_mut::<LifecycleState>() else {
                return false;
            };
            lifecycle.0 = checkpoint.lifecycle;
        }
        {
            let Some(mut gameplay) = row.get_mut::<GameplayDecisionState>() else {
                return false;
            };
            *gameplay = checkpoint.gameplay;
        }
        replace_optional_component(&mut row, checkpoint.effects);
        true
    }

    pub(crate) fn apply_effect(
        &mut self,
        id: EntityId,
        request: EntityEffectRequest,
    ) -> EntityEffectResult {
        let Some(entity) = self
            .world
            .resource::<RuntimeEntityIndex>()
            .0
            .get(&id)
            .copied()
        else {
            return EntityEffectResult::Rejected(EntityEffectRejection::Missing);
        };
        let Ok(row) = self.world.get_entity(entity) else {
            return EntityEffectResult::Rejected(EntityEffectRejection::Missing);
        };
        let Some(living) = row.get::<LivingState>() else {
            return EntityEffectResult::Rejected(EntityEffectRejection::Missing);
        };
        if !living.state.health.is_finite() {
            return EntityEffectResult::Rejected(EntityEffectRejection::NonFiniteCurrentHealth);
        }
        let Some(max_health) = living
            .attributes
            .base(&crate::AttributeKind::MaxHealth)
            .map(|value| value as f32)
            .filter(|value| value.is_finite() && *value > 0.0)
        else {
            return EntityEffectResult::Rejected(EntityEffectRejection::InvalidMaxHealth);
        };
        let Some(lifecycle) = row.get::<LifecycleState>().map(|state| state.0) else {
            return EntityEffectResult::Rejected(EntityEffectRejection::Missing);
        };
        if lifecycle != EntityLifecycle::Alive || living.state.health <= 0.0 {
            return EntityEffectResult::Rejected(EntityEffectRejection::Dead);
        }
        if let EntityEffectOperation::ApplyAction { action, .. } = request.operation
            && !effect_action_amount_is_valid(action)
        {
            return EntityEffectResult::Rejected(EntityEffectRejection::InvalidAction);
        }
        if matches!(
            request.operation,
            EntityEffectOperation::ApplyAction {
                action: EffectAction::Heal { .. } | EffectAction::HealIfBelowMax { .. },
                ..
            }
        ) && living.state.health >= max_health
        {
            return EntityEffectResult::Rejected(EntityEffectRejection::AtMaxHealth);
        }
        let current_living = living.state;
        let current_effects = row.get::<ActiveEffectsState>().cloned();

        let mut runtime = match RuntimeState::try_new(current_living, None) {
            Ok(runtime) => runtime,
            Err(RuntimeStateError::InvalidLiving(
                crate::living_26_1_2::StateError::NonFiniteHealth,
            )) => {
                return EntityEffectResult::Rejected(EntityEffectRejection::NonFiniteCurrentHealth);
            }
            Err(_) => {
                return EntityEffectResult::Rejected(EntityEffectRejection::InvalidRuntimeState);
            }
        };
        let (publications, next_effects) = match request.operation {
            EntityEffectOperation::ApplyAction {
                effect_id,
                action,
                damage_context,
            } => {
                let applied = match apply_effect_action(
                    &mut runtime,
                    effect_id,
                    action,
                    max_health,
                    request.target_kind,
                    damage_context,
                ) {
                    Ok(applied) => applied,
                    Err(EffectActionApplyError::InvalidMaxHealth) => {
                        return EntityEffectResult::Rejected(
                            EntityEffectRejection::InvalidMaxHealth,
                        );
                    }
                    Err(EffectActionApplyError::UnresolvedDamageContext) => {
                        return EntityEffectResult::Rejected(
                            EntityEffectRejection::UnresolvedDamageContext,
                        );
                    }
                    Err(EffectActionApplyError::InvalidDamage(_)) => {
                        return EntityEffectResult::Rejected(EntityEffectRejection::InvalidAction);
                    }
                };
                (applied.publications().to_vec(), current_effects)
            }
            EntityEffectOperation::Add(effect) => {
                let mut effects = match current_effects {
                    Some(effects) => effects,
                    None => match new_active_effects_state() {
                        Ok(effects) => effects,
                        Err(rejection) => return EntityEffectResult::Rejected(rejection),
                    },
                };
                let outcome = match effects.active.add(effect) {
                    Ok(outcome) => outcome,
                    Err(_) => {
                        return EntityEffectResult::Rejected(EntityEffectRejection::EffectCapacity);
                    }
                };
                if matches!(outcome, AddOutcome::Added { .. }) {
                    effects.action_order.push(effect.id);
                }
                (Vec::new(), Some(effects))
            }
            EntityEffectOperation::Tick {
                entity_tick_count,
                target_context,
                damage_context,
            } => {
                let Some(mut effects) = current_effects else {
                    return EntityEffectResult::Rejected(EntityEffectRejection::NoActiveEffects);
                };
                if effects.active.is_empty() {
                    return EntityEffectResult::Rejected(EntityEffectRejection::NoActiveEffects);
                }
                let mut scratch = match RuntimeScratch::try_new(effects.active.len(), 0) {
                    Ok(scratch) => scratch,
                    Err(_) => {
                        return EntityEffectResult::Rejected(
                            EntityEffectRejection::TickPreparation,
                        );
                    }
                };
                let mut prepared = match prepare_tick(
                    &runtime,
                    &effects.active,
                    TickInput {
                        entity_tick_count,
                        target_effect_context: target_context,
                        target_kind: request.target_kind,
                        mode: TickMode::Normal,
                        effect_action_order: &effects.action_order,
                        max_health,
                        invulnerability_clock: match request.target_kind {
                            TargetKind::Player => {
                                crate::living_26_1_2::InvulnerabilityClock::External
                            }
                            TargetKind::NonPlayer => {
                                crate::living_26_1_2::InvulnerabilityClock::Kernel
                            }
                        },
                        should_tick_death: false,
                        damage_inputs: &[],
                    },
                    &mut scratch,
                ) {
                    Ok(prepared) => prepared,
                    Err(_) => {
                        return EntityEffectResult::Rejected(
                            EntityEffectRejection::TickPreparation,
                        );
                    }
                };
                for action in prepared.actions_mut() {
                    if action.damage_source().is_some()
                        && action.resolve_damage(damage_context).is_err()
                    {
                        return EntityEffectResult::Rejected(
                            EntityEffectRejection::TickPreparation,
                        );
                    }
                }
                let publications = match apply_tick(&mut runtime, &mut effects.active, &mut scratch)
                {
                    Ok(applied) => applied.publications().to_vec(),
                    Err(_) => {
                        return EntityEffectResult::Rejected(EntityEffectRejection::TickApply);
                    }
                };
                effects
                    .action_order
                    .retain(|id| effects.active.get(*id).is_some());
                (publications, Some(effects))
            }
        };

        let next_living = runtime.living();
        let next_lifecycle = match next_living.lifecycle {
            LivingLifecycle::Alive => EntityLifecycle::Alive,
            LivingLifecycle::Dying | LivingLifecycle::Removed => EntityLifecycle::Despawning,
        };
        let killed = next_lifecycle == EntityLifecycle::Despawning;
        let Ok(mut row) = self.world.get_entity_mut(entity) else {
            return EntityEffectResult::Rejected(EntityEffectRejection::Missing);
        };
        {
            let Some(mut living) = row.get_mut::<LivingState>() else {
                return EntityEffectResult::Rejected(EntityEffectRejection::Missing);
            };
            living.state = next_living;
        }
        {
            let Some(mut lifecycle) = row.get_mut::<LifecycleState>() else {
                return EntityEffectResult::Rejected(EntityEffectRejection::Missing);
            };
            lifecycle.0 = next_lifecycle;
        }
        if killed {
            let Some(mut gameplay) = row.get_mut::<GameplayDecisionState>() else {
                return EntityEffectResult::Rejected(EntityEffectRejection::Missing);
            };
            gameplay.death_remove_tick = Some(request.death_remove_tick);
            gameplay.sheep_grazing_ticks = None;
        }
        replace_optional_component(&mut row, next_effects);
        if killed {
            self.world
                .resource_mut::<BreedingTickEntities>()
                .0
                .remove(&id);
            self.world.resource_mut::<SheepEntities>().0.remove(&id);
        }
        let snapshot = snapshot_from_world(&self.world, id)
            .expect("committed effect target remains indexed in ECS");
        EntityEffectResult::Applied(Box::new(EntityEffectApplied {
            snapshot,
            publications,
        }))
    }

    pub(crate) fn contains(&self, id: EntityId) -> bool {
        let Some(&entity) = self.world.resource::<RuntimeEntityIndex>().0.get(&id) else {
            return false;
        };
        self.world.get_entity(entity).is_ok()
    }

    pub(crate) fn ids_cover_world(&self, ids: &HashSet<EntityId>) -> bool {
        active_set_covers_world(&self.world, ids)
    }
    pub(crate) fn contains_uuid(&self, uuid: Uuid) -> bool {
        self.world
            .resource::<RuntimeEntityUuids>()
            .0
            .contains(&uuid)
    }

    pub(crate) fn sheep_grazing_activity(&self, id: EntityId) -> Option<bool> {
        let entity = *self.world.resource::<RuntimeEntityIndex>().0.get(&id)?;
        let gameplay = self.world.get::<GameplayDecisionState>(entity)?;
        Some(gameplay.sheep_grazing_ticks.is_some())
    }

    pub(crate) fn motion_state(&self, id: EntityId) -> Option<EntityMotionState> {
        let entity = *self.world.resource::<RuntimeEntityIndex>().0.get(&id)?;
        let entity = self.world.get_entity(entity).ok()?;
        let identity = entity.get::<StableIdentity>()?;
        let entity_type = entity.get::<EntityTypeState>()?;
        let transform = entity.get::<TransformState>()?;
        let motion = entity.get::<MotionState>()?;
        let goal = entity.get::<AiGoalState>()?;
        let gameplay = entity.get::<GameplayDecisionState>()?;
        Some(motion_state_from_components(
            identity,
            entity_type,
            transform,
            motion,
            goal,
            gameplay,
        ))
    }

    pub(crate) fn goal_checkpoint(&self, id: EntityId) -> Option<EntityGoalCheckpoint> {
        entity_goal_checkpoint_from_world(&self.world, id)
    }

    pub(crate) fn goal_checkpoints_for_ids(
        &self,
        ids: &HashSet<EntityId>,
    ) -> Vec<EntityGoalCheckpoint> {
        if ids.len() < 64 {
            let mut checkpoints = ids
                .iter()
                .filter_map(|&id| self.goal_checkpoint(id))
                .collect::<Vec<_>>();
            checkpoints.sort_unstable_by_key(|checkpoint| checkpoint.id);
            return checkpoints;
        }

        let index = self.world.resource::<RuntimeEntityIndex>();
        index
            .0
            .values()
            .filter_map(|&ecs_entity| {
                let entity = self.world.get_entity(ecs_entity).ok()?;
                let identity = entity.get::<StableIdentity>()?;
                ids.contains(&identity.id)
                    .then(|| entity_goal_checkpoint_from_entity(&entity))
                    .flatten()
            })
            .collect()
    }

    pub(crate) fn restore_goal_checkpoint(&mut self, checkpoint: EntityGoalCheckpoint) -> bool {
        let Some(entity) = self
            .world
            .resource::<RuntimeEntityIndex>()
            .0
            .get(&checkpoint.id)
            .copied()
        else {
            return false;
        };
        let Ok(mut entity) = self.world.get_entity_mut(entity) else {
            return false;
        };
        {
            let Some(mut transform) = entity.get_mut::<TransformState>() else {
                return false;
            };
            transform.position = checkpoint.position;
            transform.rotation = checkpoint.rotation;
        }
        {
            let Some(mut motion) = entity.get_mut::<MotionState>() else {
                return false;
            };
            motion.velocity = checkpoint.velocity;
            motion.on_ground = checkpoint.on_ground;
        }
        {
            let Some(mut lifecycle) = entity.get_mut::<LifecycleState>() else {
                return false;
            };
            lifecycle.0 = checkpoint.lifecycle;
        }
        {
            let Some(mut goal) = entity.get_mut::<AiGoalState>() else {
                return false;
            };
            goal.0 = checkpoint.goal;
        }
        let Some(mut path) = entity.get_mut::<AiPathState>() else {
            return false;
        };
        path.0 = checkpoint.path;
        true
    }

    pub(crate) fn simulation_result(&self, id: EntityId) -> Option<EntitySimulationResult> {
        entity_simulation_result_from_world(&self.world, id)
    }

    pub(crate) fn view(&self, id: EntityId) -> Option<EntityView<'_>> {
        entity_view_from_world(&self.world, id)
    }

    pub(crate) fn kinematics_fence_state(
        &self,
        id: EntityId,
    ) -> Option<EntityKinematicsFenceState> {
        let ecs_entity = *self.world.resource::<RuntimeEntityIndex>().0.get(&id)?;
        let entity = self.world.get_entity(ecs_entity).ok()?;
        let (identity, entity_type, transform, motion, lifecycle, _, goal, _, gameplay) = entity
            .get_components::<(
                &StableIdentity,
                &EntityTypeState,
                &TransformState,
                &MotionState,
                &LifecycleState,
                &LivingState,
                &AiGoalState,
                &AiPathState,
                &GameplayDecisionState,
            )>()
            .ok()?;
        let vehicle_attached = entity.get::<VehicleKindState>().is_some();
        Some(kinematics_fence_state_from_components(
            identity,
            entity_type,
            transform,
            motion,
            lifecycle,
            goal,
            gameplay,
            vehicle_attached,
        ))
    }

    pub(crate) fn kinematics_fence_states(
        &self,
        ids: &HashSet<EntityId>,
    ) -> HashMap<EntityId, EntityKinematicsFenceState> {
        if ids.len() < 64 {
            return ids
                .iter()
                .filter_map(|&id| self.kinematics_fence_state(id).map(|state| (id, state)))
                .collect();
        }
        let index = self.world.resource::<RuntimeEntityIndex>();
        index
            .0
            .values()
            .filter_map(|&ecs_entity| {
                let entity = self.world.get_entity(ecs_entity).ok()?;
                let (identity, entity_type, transform, motion, lifecycle, _, goal, _, gameplay) =
                    entity
                        .get_components::<(
                            &StableIdentity,
                            &EntityTypeState,
                            &TransformState,
                            &MotionState,
                            &LifecycleState,
                            &LivingState,
                            &AiGoalState,
                            &AiPathState,
                            &GameplayDecisionState,
                        )>()
                        .ok()?;
                if !ids.contains(&identity.id) {
                    return None;
                }
                let vehicle_attached = entity.get::<VehicleKindState>().is_some();
                Some((
                    identity.id,
                    kinematics_fence_state_from_components(
                        identity,
                        entity_type,
                        transform,
                        motion,
                        lifecycle,
                        goal,
                        gameplay,
                        vehicle_attached,
                    ),
                ))
            })
            .collect()
    }

    pub(crate) fn visit_simulation_fence_results_for_ordered_ids(
        &self,
        ids: &[EntityId],
        mut visitor: impl FnMut(EntityKinematicsFenceState, Option<EntitySimulationResult>),
    ) {
        if ids.len() < 64 {
            for &id in ids {
                let Some(state) = self.kinematics_fence_state(id) else {
                    continue;
                };
                let result = (!state.vehicle_attached)
                    .then(|| self.simulation_result(id))
                    .flatten();
                visitor(state, result);
            }
            return;
        }

        let index = self.world.resource::<RuntimeEntityIndex>();
        let mut cursor = 0;
        for (&id, &ecs_entity) in &index.0 {
            while cursor < ids.len() && ids[cursor] < id {
                cursor += 1;
            }
            if cursor == ids.len() {
                break;
            }
            if ids[cursor] != id {
                continue;
            }
            cursor += 1;
            let Ok(entity) = self.world.get_entity(ecs_entity) else {
                continue;
            };
            let Some((state, result)) = simulation_fence_result_from_entity(&entity) else {
                continue;
            };
            visitor(state, result);
        }
    }

    pub(crate) fn visit_simulation_fence_results(
        &mut self,
        mut visitor: impl FnMut(EntityKinematicsFenceState, Option<EntitySimulationResult>),
    ) {
        let mut query = self.world.query::<(
            &StableIdentity,
            &EntityTypeState,
            &TransformState,
            &MotionState,
            &LifecycleState,
            &LivingState,
            &AiGoalState,
            &AiPathState,
            &GameplayDecisionState,
            Option<&VehicleKindState>,
            Option<&ItemStackState>,
            Option<&ExperienceState>,
            Option<&FallingBlockState>,
            Option<&AnimalState>,
        )>();
        let index = self.world.resource::<RuntimeEntityIndex>();
        for (
            identity,
            entity_type,
            transform,
            motion,
            lifecycle,
            _,
            goal,
            _,
            gameplay,
            vehicle,
            item,
            experience,
            falling_block,
            animal,
        ) in query.iter_many(&self.world, index.0.values().copied())
        {
            let vehicle_attached = vehicle.is_some();
            let state = kinematics_fence_state_from_components(
                identity,
                entity_type,
                transform,
                motion,
                lifecycle,
                goal,
                gameplay,
                vehicle_attached,
            );
            let ordinary_living = item.is_none()
                && experience.is_none()
                && falling_block.is_none()
                && !vehicle_attached;
            let result = (state.lifecycle == EntityLifecycle::Alive && !state.vehicle_attached)
                .then(|| {
                    entity_simulation_result_from_motion(
                        entity_type,
                        gameplay,
                        animal.map(|animal| animal.0),
                        ordinary_living,
                        state.motion,
                    )
                });
            visitor(state, result);
        }
    }

    pub(crate) fn visit_goal_tick_candidates(
        &mut self,
        mut visitor: impl FnMut(EntityKinematics, EntityLifecycle, bool, bool, Option<Vec3>),
    ) {
        let mut query = self.world.query::<(
            &StableIdentity,
            &EntityTypeState,
            &TransformState,
            &MotionState,
            &LifecycleState,
            &LivingState,
            &AiGoalState,
            &AiPathState,
            &GameplayDecisionState,
            Option<&VehicleKindState>,
            Option<&ItemStackState>,
            Option<&ExperienceState>,
            Option<&FallingBlockState>,
        )>();
        let index = self.world.resource::<RuntimeEntityIndex>();
        for (
            identity,
            entity_type,
            transform,
            motion,
            lifecycle,
            _,
            _,
            _,
            gameplay,
            vehicle,
            item,
            experience,
            falling_block,
        ) in query.iter_many(&self.world, index.0.values().copied())
        {
            let vehicle_attached = vehicle.is_some();
            let ordinary_living = item.is_none()
                && experience.is_none()
                && falling_block.is_none()
                && !vehicle_attached;
            let local_living = lifecycle.0 == EntityLifecycle::Alive
                && gameplay.item_pickup_claim.is_none()
                && !vehicle_attached
                && matches!(
                    entity_physics_kind(entity_type, gameplay, ordinary_living),
                    EntityPhysicsKind::Living
                        | EntityPhysicsKind::PowderSnowWalkableLiving
                        | EntityPhysicsKind::FishLiving
                        | EntityPhysicsKind::SquidLiving
                        | EntityPhysicsKind::AquaticLiving
                );
            visitor(
                EntityKinematics {
                    id: identity.id,
                    position: transform.position,
                    rotation: transform.rotation,
                    velocity: motion.velocity,
                    on_ground: motion.on_ground,
                },
                lifecycle.0,
                local_living,
                entity_type.villager,
                villager_job_site(gameplay, transform.position),
            );
        }
    }

    pub(crate) fn vehicle_states(
        &self,
    ) -> impl Iterator<Item = (EntityId, EntityLifecycle, Option<VehicleState>)> + '_ {
        self.world
            .resource::<RuntimeEntityIndex>()
            .0
            .values()
            .filter_map(|&entity| {
                let entity = self.world.get_entity(entity).ok()?;
                let (identity, lifecycle, kind, passenger) = entity
                    .get_components::<(
                        &StableIdentity,
                        &LifecycleState,
                        Option<&VehicleKindState>,
                        Option<&PassengerState>,
                    )>()
                    .ok()?;
                Some((
                    identity.id,
                    lifecycle.0,
                    kind.map(|kind| VehicleState {
                        kind: kind.0,
                        passenger: passenger.map(|passenger| passenger.0),
                    }),
                ))
            })
    }

    pub(crate) fn passenger_ids(&self) -> HashSet<EntityId> {
        self.vehicle_states()
            .filter_map(|(_, _, vehicle)| vehicle?.passenger)
            .collect()
    }

    pub(crate) fn views(&self) -> impl Iterator<Item = EntityView<'_>> + '_ {
        self.world
            .resource::<RuntimeEntityIndex>()
            .0
            .keys()
            .filter_map(|&id| entity_view_from_world(&self.world, id))
    }

    pub(crate) fn attributes_mut(&mut self, id: EntityId) -> Option<&mut AttributeSet> {
        let entity = *self.world.resource::<RuntimeEntityIndex>().0.get(&id)?;
        let living = self.world.get_mut::<LivingState>(entity)?;
        Some(&mut living.into_inner().attributes)
    }

    pub(crate) fn visit_entities(&self, visitor: &mut impl FnMut(EntityView<'_>)) {
        for view in self.views() {
            visitor(view);
        }
    }

    pub(crate) fn visit_breeding_tick_entities(&self, visitor: &mut impl FnMut(EntityView<'_>)) {
        for &id in &self.world.resource::<BreedingTickEntities>().0 {
            if let Some(view) = entity_view_from_world(&self.world, id) {
                visitor(view);
            }
        }
    }

    pub(crate) fn visit_sheep_entities_for_ids(
        &self,
        candidate_ids: &HashSet<EntityId>,
        visitor: &mut impl FnMut(EntityView<'_>),
    ) {
        let sheep_ids = &self.world.resource::<SheepEntities>().0;
        if candidate_ids.len() < sheep_ids.len() {
            let mut ordered_ids = candidate_ids.iter().copied().collect::<Vec<_>>();
            ordered_ids.sort_unstable();
            for id in ordered_ids {
                if sheep_ids.contains(&id)
                    && let Some(view) = entity_view_from_world(&self.world, id)
                {
                    visitor(view);
                }
            }
            return;
        }
        for &id in sheep_ids {
            if candidate_ids.contains(&id)
                && let Some(view) = entity_view_from_world(&self.world, id)
            {
                visitor(view);
            }
        }
    }

    pub(crate) fn alive_kinematics_for_ids(
        &mut self,
        ids: &HashSet<EntityId>,
    ) -> Vec<EntityKinematics> {
        let covers_world = active_set_covers_world(&self.world, ids);
        if !active_set_is_sparse(&self.world, ids) {
            let mut query = self.world.query::<(
                &StableIdentity,
                &TransformState,
                &MotionState,
                &LifecycleState,
            )>();
            return query
                .iter(&self.world)
                .filter(|(identity, _, _, lifecycle)| {
                    lifecycle.0 == EntityLifecycle::Alive
                        && (covers_world || ids.contains(&identity.id))
                })
                .map(|(identity, transform, motion, _)| EntityKinematics {
                    id: identity.id,
                    position: transform.position,
                    rotation: transform.rotation,
                    velocity: motion.velocity,
                    on_ground: motion.on_ground,
                })
                .collect();
        }
        let mut ordered_ids = ids.iter().copied().collect::<Vec<_>>();
        ordered_ids.sort_unstable();
        let index = self.world.resource::<RuntimeEntityIndex>();
        let entities = ordered_ids
            .iter()
            .filter_map(|id| index.0.get(id).copied().map(|entity| (*id, entity)))
            .collect::<Vec<_>>();
        entities
            .into_iter()
            .filter_map(|(id, entity)| {
                let entity = self.world.get_entity(entity).ok()?;
                if entity.get::<LifecycleState>()?.0 != EntityLifecycle::Alive {
                    return None;
                }
                let transform = entity.get::<TransformState>()?;
                let motion = entity.get::<MotionState>()?;
                Some(EntityKinematics {
                    id,
                    position: transform.position,
                    rotation: transform.rotation,
                    velocity: motion.velocity,
                    on_ground: motion.on_ground,
                })
            })
            .collect()
    }

    pub(crate) fn goal_matches(&self, id: EntityId, goal: &GoalState) -> bool {
        let Some(&entity) = self.world.resource::<RuntimeEntityIndex>().0.get(&id) else {
            return false;
        };
        let Ok(entity) = self.world.get_entity(entity) else {
            return false;
        };
        entity
            .get::<AiGoalState>()
            .is_some_and(|current| &current.0 == goal)
    }

    #[must_use]
    pub fn normalized_snapshots(&self) -> Vec<EntitySnapshot> {
        normalized_snapshots_from_world(&self.world)
    }

    pub(crate) fn visit_entity(&self, id: EntityId, visitor: &mut impl FnMut(EntityView<'_>)) {
        if let Some(view) = entity_view_from_world(&self.world, id) {
            visitor(view);
        }
    }

    pub(crate) fn goal_tick_selection(
        &self,
        region: crate::RegionKey,
        tick: u64,
        candidate_ids: &HashSet<EntityId>,
        inputs: &crate::regional::RegionalGoalTickInputs,
    ) -> crate::EntityGoalTickSelection {
        let index = self.world.resource::<RuntimeEntityIndex>();
        let world_entity_count = index.0.len();
        let ordered_entities = index
            .0
            .iter()
            .filter(|(id, _)| candidate_ids.contains(id))
            .map(|(&id, &entity)| (id, entity))
            .collect::<Vec<_>>();
        self.goal_tick_selection_from_ordered_entities(
            region,
            tick,
            ordered_entities,
            world_entity_count,
            inputs,
        )
    }

    pub(crate) fn goal_tick_selection_for_ordered_ids(
        &self,
        region: crate::RegionKey,
        tick: u64,
        candidate_ids: &[EntityId],
        inputs: &crate::regional::RegionalGoalTickInputs,
    ) -> crate::EntityGoalTickSelection {
        debug_assert!(
            candidate_ids.windows(2).all(|ids| ids[0] < ids[1]),
            "ordered goal candidates must be unique"
        );
        let index = self.world.resource::<RuntimeEntityIndex>();
        let world_entity_count = index.0.len();
        let mut candidates = candidate_ids.iter().copied().peekable();
        let mut ordered_entities = Vec::with_capacity(candidate_ids.len());
        for (&id, &entity) in &index.0 {
            while candidates.peek().is_some_and(|candidate| *candidate < id) {
                candidates.next();
            }
            if candidates.peek().is_some_and(|candidate| *candidate == id) {
                candidates.next();
                ordered_entities.push((id, entity));
            }
        }
        self.goal_tick_selection_from_ordered_entities(
            region,
            tick,
            ordered_entities,
            world_entity_count,
            inputs,
        )
    }

    fn goal_tick_selection_from_ordered_entities(
        &self,
        region: crate::RegionKey,
        tick: u64,
        ordered_entities: Vec<(EntityId, EcsEntity)>,
        world_entity_count: usize,
        inputs: &crate::regional::RegionalGoalTickInputs,
    ) -> crate::EntityGoalTickSelection {
        let mut goal_ids = Vec::with_capacity(ordered_entities.len());
        let mut passive_decisions = 0;
        let (mut goal_overrides, villager_updates, cross_region_villager_candidates) =
            plan_region_villager_updates(
                &self.world,
                region,
                &ordered_entities,
                inputs.active_chunks.as_deref(),
                tick,
                inputs.villager.as_deref(),
            );
        let mut pathing_aabbs = Vec::new();
        let mut snapshot_overrides = Vec::new();
        let mut checkpoints = Vec::with_capacity(ordered_entities.len());
        let mut pathing_requests = Vec::with_capacity(ordered_entities.len());
        for (id, ecs_entity) in ordered_entities {
            let Ok(entity) = self.world.get_entity(ecs_entity) else {
                continue;
            };
            let Some(lifecycle) = entity.get::<LifecycleState>() else {
                continue;
            };
            let Some(entity_type) = entity.get::<EntityTypeState>() else {
                continue;
            };
            let Some(transform) = entity.get::<TransformState>() else {
                continue;
            };
            let Ok((identity, motion, goal, path)) =
                entity
                    .get_components::<(&StableIdentity, &MotionState, &AiGoalState, &AiPathState)>(
                    )
            else {
                continue;
            };
            let chunk = (
                (transform.position.x.floor() as i32).div_euclid(16),
                (transform.position.z.floor() as i32).div_euclid(16),
            );
            if lifecycle.0 != EntityLifecycle::Alive
                || inputs
                    .active_chunks
                    .as_ref()
                    .is_some_and(|active_chunks| !active_chunks.contains(&chunk))
            {
                continue;
            }

            let gameplay = entity.get::<GameplayDecisionState>();
            let panic_since = entity.get::<AnimalState>().and_then(|_| {
                gameplay
                    .and_then(|state| state.last_damage_tick)
                    .filter(|damage_tick| tick.saturating_sub(*damage_tick) < 100)
            });
            if inputs.terrain_pathing_entities.contains(&id)
                || crate::aquatic_motion::Swimmer::for_type(&entity_type.name)
                    != crate::aquatic_motion::Swimmer::Other
            {
                pathing_aabbs.push((
                    id,
                    crate::natural_spawn_26_1_2::entity_geometry(
                        &entity_type.name,
                        entity.get::<AnimalState>().map(|state| state.0),
                    )
                    .aabb,
                ));
            }
            if let Some(hostile_target_positions) = inputs.hostile_target_positions.as_deref()
                && entity_type.hostile
                && let Some(view) = entity_view_from_world(&self.world, id)
                && let Some(goal) = crate::natural_spawn_26_1_2::hostile_goal_for_entity(
                    &view,
                    hostile_target_positions,
                    &inputs.mob_behaviors,
                )
                && goal != *view.goal
            {
                goal_overrides.insert(id, goal);
            }
            let snapshot_overridden = if entity_type.name.as_ref() == "minecraft:shulker_bullet"
                && let Some(expected) = snapshot_from_world(&self.world, id)
                && let Some(next) =
                    retarget_shulker_bullet_snapshot(&expected, &inputs.combat_targets)
            {
                snapshot_overrides.push((expected, next));
                true
            } else {
                false
            };
            let goal_eligible = entity_type.name.as_ref() != "minecraft:ender_dragon"
                && gameplay.is_none_or(|state| state.sheep_grazing_ticks.is_none())
                && panic_since.is_none_or(|damage_tick| tick.saturating_sub(damage_tick) >= 5);
            let overridden_goal = goal_overrides.get(&id);
            let selected_goal = overridden_goal.unwrap_or(&goal.0);
            let goal_overridden = overridden_goal.is_some();
            let passive = goal_eligible
                && !goal_overridden
                && !snapshot_overridden
                && matches!(selected_goal, GoalState::Idle)
                && motion.velocity.x == 0.0
                && motion.velocity.z == 0.0;
            let goal_active = goal_eligible && !passive;
            if goal_active {
                goal_ids.push(id);
            } else if passive {
                passive_decisions += 1;
            }
            let pathing_requested = goal_active
                && goal_pathing_request(
                    identity,
                    transform,
                    motion,
                    selected_goal,
                    path,
                    tick,
                    panic_since,
                    &entity_type.name,
                )
                .is_some_and(|request| {
                    pathing_requests.push(request);
                    true
                });
            if !pathing_requested
                && (goal_active || goal_overridden)
                && let Some(checkpoint) = entity_goal_checkpoint_from_entity(&entity)
            {
                checkpoints.push(checkpoint);
            }
        }
        let active_ids =
            (goal_ids.len() != world_entity_count).then(|| goal_ids.into_iter().collect());
        crate::EntityGoalTickSelection {
            checkpoints,
            goal_tick: crate::PreparedGoalTick {
                tick,
                active_ids,
                passive_decisions,
                pathing_requests,
            },
            goal_overrides,
            pathing_aabbs,
            snapshot_overrides,
            villager_updates,
            cross_region_villager_candidates,
        }
    }

    pub(crate) fn pathing_requests(
        &mut self,
        tick: u64,
        active_ids: Option<&HashSet<EntityId>>,
        goal_overrides: &HashMap<EntityId, GoalState>,
    ) -> Vec<GoalPathingRequest> {
        if let Some(active_ids) = active_ids
            && active_set_is_sparse(&self.world, active_ids)
        {
            let mut ids = active_ids.iter().copied().collect::<Vec<_>>();
            ids.sort_unstable();
            return ids
                .into_iter()
                .filter_map(|id| self.pathing_request(id, tick, goal_overrides.get(&id)))
                .collect();
        }
        let active_filter =
            active_ids.filter(|active_ids| !active_set_covers_world(&self.world, active_ids));
        let mut query = self.world.query_filtered::<(
            &StableIdentity,
            &TransformState,
            &MotionState,
            &LifecycleState,
            &AiGoalState,
            &AiPathState,
            Option<&AnimalState>,
            Option<&GameplayDecisionState>,
            &EntityTypeState,
        ), (
            Without<ItemStackState>,
            Without<ExperienceState>,
            Without<FallingBlockState>,
            Without<ProjectileState>,
            Without<VehicleKindState>,
        )>();
        let mut requests = query
            .iter(&self.world)
            .filter(|(_, _, _, lifecycle, _, _, _, _, _)| lifecycle.0 == EntityLifecycle::Alive)
            .filter(|(identity, _, _, _, _, _, _, _, _)| {
                active_filter.is_none_or(|active_ids| active_ids.contains(&identity.id))
            })
            .filter_map(
                |(identity, transform, motion, _, goal, path, animal, gameplay, entity_type)| {
                    let goal = goal_overrides.get(&identity.id).unwrap_or(&goal.0);
                    let panic_since = animal.and_then(|_| {
                        gameplay
                            .and_then(|state| state.last_damage_tick)
                            .filter(|damage_tick| tick.saturating_sub(*damage_tick) < 100)
                    });
                    goal_pathing_request(
                        identity,
                        transform,
                        motion,
                        goal,
                        path,
                        tick,
                        panic_since,
                        &entity_type.name,
                    )
                },
            )
            .collect::<Vec<_>>();
        requests.sort_unstable_by_key(|request| request.id);
        requests
    }

    fn pathing_request(
        &self,
        id: EntityId,
        tick: u64,
        goal_override: Option<&GoalState>,
    ) -> Option<GoalPathingRequest> {
        let entity = *self.world.resource::<RuntimeEntityIndex>().0.get(&id)?;
        let entity = self.world.get_entity(entity).ok()?;
        if entity.contains::<ItemStackState>()
            || entity.contains::<ExperienceState>()
            || entity.contains::<FallingBlockState>()
            || entity.contains::<ProjectileState>()
            || entity.contains::<VehicleKindState>()
            || entity.get::<LifecycleState>()?.0 != EntityLifecycle::Alive
        {
            return None;
        }
        let identity = entity.get::<StableIdentity>()?;
        let transform = entity.get::<TransformState>()?;
        let motion = entity.get::<MotionState>()?;
        let goal = entity.get::<AiGoalState>()?;
        let path = entity.get::<AiPathState>()?;
        let panic_since = entity.get::<AnimalState>().and_then(|_| {
            entity
                .get::<GameplayDecisionState>()
                .and_then(|state| state.last_damage_tick)
                .filter(|damage_tick| tick.saturating_sub(*damage_tick) < 100)
        });
        goal_pathing_request(
            identity,
            transform,
            motion,
            goal_override.unwrap_or(&goal.0),
            path,
            tick,
            panic_since,
            &entity.get::<EntityTypeState>()?.name,
        )
    }

    pub fn remove(&mut self, id: EntityId) -> Option<EntitySnapshot> {
        remove_from_world(&mut self.world, id)
    }

    pub fn queue_input(&mut self, command: EntityInputCommand) {
        self.world
            .resource_mut::<PendingInputCommands>()
            .0
            .push(command);
    }

    pub fn queue_physics(&mut self, result: EntityPhysicsResult) {
        self.world
            .resource_mut::<PendingPhysicsResults>()
            .0
            .push(result);
    }

    pub(crate) fn queue_kinematics(
        &mut self,
        states: impl IntoIterator<Item = EntityKinematics>,
    ) -> usize {
        let mut pending = std::mem::take(&mut self.world.resource_mut::<PendingPhysicsResults>().0);
        let applied = {
            let entity_index = self.world.resource::<RuntimeEntityIndex>();
            let mut applied = 0;
            for state in states {
                if !state.is_finite() || !entity_index.0.contains_key(&state.id) {
                    continue;
                }
                pending.push(EntityPhysicsResult {
                    id: state.id,
                    position: state.position,
                    rotation: state.rotation,
                    velocity: state.velocity,
                    on_ground: state.on_ground,
                });
                applied += 1;
            }
            applied
        };
        self.world.resource_mut::<PendingPhysicsResults>().0 = pending;
        applied
    }

    pub(crate) fn queue_kinematics_prevalidated(
        &mut self,
        states: impl IntoIterator<Item = EntityKinematics>,
    ) -> usize {
        let mut pending = std::mem::take(&mut self.world.resource_mut::<PendingPhysicsResults>().0);
        let mut applied = 0;
        for state in states {
            if !state.is_finite() {
                continue;
            }
            pending.push(EntityPhysicsResult {
                id: state.id,
                position: state.position,
                rotation: state.rotation,
                velocity: state.velocity,
                on_ground: state.on_ground,
            });
            applied += 1;
        }
        self.world.resource_mut::<PendingPhysicsResults>().0 = pending;
        applied
    }

    pub(crate) fn run_goal_tick(
        &mut self,
        request: GoalTickRequest,
    ) -> (GoalTickStats, Vec<GoalSimulationCandidate>) {
        self.run_goal_tick_inner(request, None)
    }

    pub(crate) fn run_owner_goal_tick(
        &mut self,
        request: GoalTickRequest,
    ) -> (GoalTickStats, OwnerGoalTickOutput) {
        let mut output = OwnerGoalTickOutput::default();
        let (stats, captured) = self.run_goal_tick_inner(request, Some(&mut output));
        debug_assert!(captured.is_empty());
        (stats, output)
    }

    fn run_goal_tick_inner(
        &mut self,
        mut request: GoalTickRequest,
        owner_output: Option<&mut OwnerGoalTickOutput>,
    ) -> (GoalTickStats, Vec<GoalSimulationCandidate>) {
        #[cfg(feature = "load-bench")]
        let apply_split_tick = request.tick;
        #[cfg(feature = "load-bench")]
        let apply_split_started = std::time::Instant::now();
        if request
            .simulation_capture_ids
            .as_ref()
            .is_some_and(Vec::is_empty)
        {
            request.simulation_capture_ids = None;
        } else if let Some(ids) = &mut request.simulation_capture_ids {
            ids.sort_unstable();
        }
        #[cfg(feature = "load-bench")]
        let apply_sort_us =
            u64::try_from(apply_split_started.elapsed().as_micros()).unwrap_or(u64::MAX);
        #[cfg(feature = "load-bench")]
        let apply_stage_started = std::time::Instant::now();
        self.run_stage(EntityStage::InputAi);
        #[cfg(feature = "load-bench")]
        let apply_stage_us =
            u64::try_from(apply_stage_started.elapsed().as_micros()).unwrap_or(u64::MAX);
        #[cfg(feature = "load-bench")]
        let apply_commit_started = std::time::Instant::now();
        let output = apply_goal_tick(&mut self.world, request, owner_output);
        #[cfg(feature = "load-bench")]
        if apply_split_tick.is_multiple_of(10) {
            eprintln!(
                "APPLY_SPLIT tick={} sort_us={} stage_us={} commit_us={}",
                apply_split_tick,
                apply_sort_us,
                apply_stage_us,
                u64::try_from(apply_commit_started.elapsed().as_micros()).unwrap_or(u64::MAX),
            );
        }
        output
    }

    pub(crate) fn queue_position_tick(&mut self, delta_seconds: f64) {
        assert!(delta_seconds.is_finite() && delta_seconds >= 0.0);
        let previous = self
            .world
            .resource_mut::<PendingPositionTick>()
            .0
            .replace((delta_seconds, None));
        assert!(previous.is_none(), "position tick already queued");
    }

    pub(crate) fn queue_position_tick_in_range(&mut self, range: Range<usize>, delta_seconds: f64) {
        assert!(delta_seconds.is_finite() && delta_seconds >= 0.0);
        assert!(range.end <= self.len(), "entity tick range out of bounds");
        let ids = self
            .world
            .resource::<RuntimeEntityIndex>()
            .0
            .keys()
            .skip(range.start)
            .take(range.end - range.start)
            .copied()
            .collect::<HashSet<_>>();
        let previous = self
            .world
            .resource_mut::<PendingPositionTick>()
            .0
            .replace((delta_seconds, Some(ids)));
        assert!(previous.is_none(), "position tick already queued");
    }

    pub fn queue_combat(&mut self, command: EntityCombatCommand) {
        self.world
            .resource_mut::<PendingCombatCommands>()
            .0
            .push(command);
    }

    pub fn request_snapshots(&mut self) {
        self.world.resource_mut::<SnapshotRequest>().0 = true;
    }

    pub fn request_persistence_extract(&mut self) {
        self.world.resource_mut::<PersistenceExtractRequest>().0 = true;
    }

    pub fn run_stage(&mut self, stage: EntityStage) {
        match stage {
            EntityStage::InputAi => {
                #[cfg(test)]
                {
                    self.input_ai_stage_runs = self.input_ai_stage_runs.saturating_add(1);
                }
                self.schedules.input_ai.run(&mut self.world);
            }
            EntityStage::SnapshotRequest => {
                self.schedules.snapshot_request.run(&mut self.world);
            }
            EntityStage::PhysicsApply => {
                #[cfg(test)]
                {
                    self.physics_apply_stage_runs = self.physics_apply_stage_runs.saturating_add(1);
                }
                self.schedules.physics_apply.run(&mut self.world);
            }
            EntityStage::CombatLifecycle => {
                self.schedules.combat_lifecycle.run(&mut self.world);
            }
            EntityStage::PersistenceExtract => {
                self.schedules.persistence_extract.run(&mut self.world);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn input_ai_stage_runs(&self) -> usize {
        self.input_ai_stage_runs
    }

    #[cfg(test)]
    pub(crate) fn physics_apply_stage_runs(&self) -> usize {
        self.physics_apply_stage_runs
    }

    pub fn take_snapshot_output(&mut self) -> Vec<EntitySnapshot> {
        std::mem::take(&mut self.world.resource_mut::<SnapshotOutput>().0)
    }

    pub fn take_persistence_output(&mut self) -> Vec<EntitySnapshot> {
        std::mem::take(&mut self.world.resource_mut::<PersistenceOutput>().0)
    }

    #[cfg(test)]
    fn has_projectile(&self, id: EntityId) -> bool {
        self.component_exists::<ProjectileState>(id)
    }

    #[cfg(test)]
    fn has_persistent_state(&self, id: EntityId) -> bool {
        self.component_exists::<PersistentState>(id)
    }

    #[cfg(test)]
    fn has_visibility_state(&self, id: EntityId) -> bool {
        self.component_exists::<VisibilityState>(id)
    }

    #[cfg(test)]
    fn component_exists<T: Component>(&self, id: EntityId) -> bool {
        self.world
            .resource::<RuntimeEntityIndex>()
            .0
            .get(&id)
            .and_then(|&entity| self.world.get_entity(entity).ok())
            .is_some_and(|entity| entity.contains::<T>())
    }
}

fn active_set_is_sparse(world: &World, active_ids: &HashSet<EntityId>) -> bool {
    active_ids.len().saturating_mul(2) < world.resource::<RuntimeEntityIndex>().0.len()
}

fn active_set_covers_world(world: &World, active_ids: &HashSet<EntityId>) -> bool {
    let index = world.resource::<RuntimeEntityIndex>();
    active_ids.len() == index.0.len() && index.0.keys().all(|id| active_ids.contains(id))
}

#[expect(
    clippy::too_many_arguments,
    reason = "Builds a request from existing ECS component borrows and tick context"
)]
fn goal_pathing_request(
    identity: &StableIdentity,
    transform: &TransformState,
    motion: &MotionState,
    goal: &GoalState,
    path: &AiPathState,
    tick: u64,
    panic_since: Option<u64>,
    type_name: &str,
) -> Option<GoalPathingRequest> {
    let (target, target_epoch, speed) = match goal {
        GoalState::AquaticWander {
            speed,
            vertical_speed,
            period_ticks,
        } => {
            if crate::aquatic_motion::Swimmer::for_type(type_name)
                == crate::aquatic_motion::Swimmer::Other
            {
                return None;
            }
            let (target, epoch) = crate::aquatic_motion::wander_target(
                identity.id,
                transform.position,
                path.0,
                tick,
                *period_ticks,
                *vertical_speed,
                crate::aquatic_motion::Swimmer::for_type(type_name),
            );
            (target, Some(epoch), *speed)
        }
        GoalState::FollowPosition { target, speed } if *speed != 0.0 => (*target, None, *speed),
        GoalState::Wander {
            speed,
            period_ticks,
        } => {
            if let Some(damage_tick) = panic_since {
                let epoch = damage_tick + tick.saturating_sub(damage_tick) / 20;
                let target = if path.0.has_target && path.0.target_epoch == Some(epoch) {
                    path.0.target
                } else {
                    let horizontal = motion.velocity.x.hypot(motion.velocity.z);
                    if horizontal > f64::EPSILON {
                        Vec3::new(
                            transform.position.x + motion.velocity.x / horizontal * 8.0,
                            transform.position.y,
                            transform.position.z + motion.velocity.z / horizontal * 8.0,
                        )
                    } else {
                        crate::wander_pathing_target(
                            identity.id,
                            transform.position,
                            path.0,
                            tick,
                            *period_ticks,
                        )
                        .0
                    }
                };
                (
                    target,
                    Some(epoch),
                    *speed * crate::natural_spawn_26_1_2::panic_speed_multiplier_26_1_2(type_name),
                )
            } else {
                let (target, epoch) = crate::wander_pathing_target(
                    identity.id,
                    transform.position,
                    path.0,
                    tick,
                    *period_ticks,
                );
                (target, Some(epoch), *speed)
            }
        }
        _ => return None,
    };
    Some(GoalPathingRequest {
        id: identity.id,
        expected_position: transform.position,
        expected_rotation: transform.rotation,
        expected_velocity: motion.velocity,
        expected_on_ground: motion.on_ground,
        expected_goal: goal.clone(),
        expected_path: path.0,
        target,
        target_epoch,
        speed,
        aquatic: matches!(goal, GoalState::AquaticWander { .. })
            .then(|| crate::aquatic_motion::Swimmer::for_type(type_name)),
    })
}

impl EntitySchedules {
    fn new() -> Self {
        let mut input_ai = Schedule::default();
        input_ai.set_executor_kind(ExecutorKind::SingleThreaded);
        input_ai.add_systems(apply_input_commands);

        let mut snapshot_request = Schedule::default();
        snapshot_request.add_systems(capture_snapshot_request);

        let mut physics_apply = Schedule::default();
        physics_apply.set_executor_kind(ExecutorKind::SingleThreaded);
        physics_apply.add_systems((apply_physics_results, integrate_positions).chain());

        let mut combat_lifecycle = Schedule::default();
        combat_lifecycle.set_executor_kind(ExecutorKind::SingleThreaded);
        combat_lifecycle.add_systems(apply_combat_commands);

        let mut persistence_extract = Schedule::default();
        persistence_extract.set_executor_kind(ExecutorKind::SingleThreaded);
        persistence_extract.add_systems(extract_persistence_snapshots);

        Self {
            input_ai,
            snapshot_request,
            physics_apply,
            combat_lifecycle,
            persistence_extract,
        }
    }
}

fn living_state_from_snapshot(
    health: f32,
    lifecycle: EntityLifecycle,
    retained: EntityLivingRetainedState,
) -> crate::living_26_1_2::LivingState {
    crate::living_26_1_2::LivingState {
        health,
        absorption: retained.absorption,
        invulnerable_time: retained.invulnerable_time,
        hurt_time: retained.hurt_time,
        last_hurt: retained.last_hurt,
        lifecycle: match lifecycle {
            EntityLifecycle::Alive => LivingLifecycle::Alive,
            EntityLifecycle::Despawning => LivingLifecycle::Dying,
        },
        death_time: retained.death_time,
    }
}

fn active_effects_state_from_snapshot(
    snapshot: Option<EntityActiveEffectsState>,
) -> Option<ActiveEffectsState> {
    let snapshot = snapshot?;
    if snapshot.action_order.len() != snapshot.effects.chains.len()
        || snapshot.action_order.iter().enumerate().any(|(index, id)| {
            snapshot.action_order[..index].contains(id)
                || !snapshot
                    .effects
                    .chains
                    .iter()
                    .any(|chain| chain.current.id == *id)
        })
    {
        return None;
    }
    let limits =
        EffectLimits::new(ENTITY_EFFECT_ACTIVE_CAPACITY, ENTITY_EFFECT_HIDDEN_CAPACITY).ok()?;
    let active = ActiveEffects::try_from_snapshot(limits, &snapshot.effects).ok()?;
    Some(ActiveEffectsState {
        active,
        action_order: snapshot.action_order,
    })
}

fn active_effects_snapshot(state: Option<&ActiveEffectsState>) -> Option<EntityActiveEffectsState> {
    state.map(|state| EntityActiveEffectsState {
        effects: state.active.snapshot(),
        action_order: state.action_order.clone(),
    })
}

fn new_active_effects_state() -> Result<ActiveEffectsState, EntityEffectRejection> {
    let limits = EffectLimits::new(ENTITY_EFFECT_ACTIVE_CAPACITY, ENTITY_EFFECT_HIDDEN_CAPACITY)
        .map_err(|_: EffectLimitError| EntityEffectRejection::EffectCapacity)?;
    let active = ActiveEffects::try_new(limits)
        .map_err(|_: EffectStoreError| EntityEffectRejection::EffectCapacity)?;
    Ok(ActiveEffectsState {
        active,
        action_order: Vec::with_capacity(ENTITY_EFFECT_ACTIVE_CAPACITY),
    })
}

fn effect_action_amount_is_valid(action: EffectAction) -> bool {
    match action {
        EffectAction::HealIfBelowMax { amount }
        | EffectAction::Heal { amount }
        | EffectAction::Damage { amount, .. } => amount.is_finite() && amount > 0.0,
        EffectAction::MagicDamageIfHealthAbove {
            amount,
            minimum_health,
        } => amount.is_finite() && amount > 0.0 && minimum_health.is_finite(),
        EffectAction::ExhaustPlayer { amount } => amount.is_finite() && amount >= 0.0,
        EffectAction::FeedPlayer {
            saturation_modifier,
            ..
        } => saturation_modifier.is_finite() && saturation_modifier >= 0.0,
    }
}

fn insert_snapshot_into_world(world: &mut World, snapshot: EntitySnapshot) -> bool {
    if world
        .resource::<RuntimeEntityIndex>()
        .0
        .contains_key(&snapshot.id)
        || world
            .resource::<RuntimeEntityUuids>()
            .0
            .contains(&snapshot.uuid)
    {
        return false;
    }

    let EntitySnapshot {
        id,
        uuid,
        type_id,
        type_name,
        position,
        rotation,
        velocity,
        on_ground,
        item_stack,
        experience_value,
        block_state,
        lifecycle,
        health,
        attributes,
        goal,
        vehicle,
        animal,
        retained,
    } = snapshot;
    let EntityRetainedState {
        path,
        living: retained_living,
        fall_distance,
        remaining_fire_ticks,
        active_effects,
        arrow_state,
        hurting_projectile_state,
        throwable_projectile_state,
        last_damage_tick,
        death_remove_tick,
        sheep_grazing_ticks,
        spawn_tick,
        item_pickup_ready_tick,
        item_pickup_owner_block,
        item_pickup_claim,
        villager_food_recipient,
        primed_tnt,
        pending_explosion,
        crossbow_attack,
        bow_attack,
        blaze_attack,
        ghast_attack,
        breeze_attack,
        witch_attack,
        witch_potion,
        dragon_air,
        dragon_breath_cloud,
        guardian_beam,
        warden_sonic_boom,
        shulker_attack,
        shulker_bullet,
        evoker_attack,
        evoker_fangs,
        villager,
        villager_brain,
        villager_gossip,
        villager_merchant,
        villager_population,
        zombie_villager_conversion,
    } = retained;
    let living_state = living_state_from_snapshot(health, lifecycle, retained_living);
    if living_state.validate().is_err() {
        return false;
    }
    let active_effects = match active_effects {
        Some(snapshot) => match active_effects_state_from_snapshot(Some(snapshot)) {
            Some(state) => Some(state),
            None => return false,
        },
        None => None,
    };
    let needs_breeding_tick = lifecycle == EntityLifecycle::Alive
        && animal.is_some_and(AnimalBreedingState::needs_breeding_tick);
    let is_projectile = type_name == "minecraft:arrow"
        || hurting_projectile_state.is_some()
        || throwable_projectile_state.is_some();
    let is_sheep = lifecycle == EntityLifecycle::Alive
        && type_name == "minecraft:sheep"
        && animal.is_some_and(|animal| animal.sheep_wool.is_some());
    let mut entity = world.spawn((
        StableIdentity { id, uuid },
        entity_type_state(type_id, type_name),
        TransformState { position, rotation },
        MotionState {
            velocity,
            on_ground,
            fall_distance,
        },
        LifecycleState(lifecycle),
        LivingState {
            state: living_state,
            attributes,
        },
        AiGoalState(goal),
        AiPathState(path),
        GameplayDecisionState {
            arrow_state,
            hurting_projectile_state,
            throwable_projectile_state,
            remaining_fire_ticks,
            last_damage_tick,
            death_remove_tick,
            sheep_grazing_ticks,
            spawn_tick,
            item_pickup_ready_tick,
            item_pickup_owner_block,
            item_pickup_claim,
            villager_food_recipient,
            primed_tnt,
            pending_explosion,
            crossbow_attack,
            bow_attack,
            blaze_attack,
            ghast_attack,
            breeze_attack,
            witch_attack,
            witch_potion,
            dragon_air,
            dragon_breath_cloud,
            guardian_beam,
            warden_sonic_boom,
            shulker_attack,
            shulker_bullet,
            evoker_attack,
            evoker_fangs,
            villager,
            villager_brain,
            villager_gossip,
            villager_merchant,
            villager_population,
            zombie_villager_conversion,
        },
        PersistentState,
        VisibilityState,
    ));
    if let Some(active_effects) = active_effects {
        entity.insert(active_effects);
    }
    if let Some(item_stack) = item_stack {
        entity.insert(ItemStackState(item_stack));
    }
    if let Some(experience_value) = experience_value {
        entity.insert(ExperienceState(experience_value));
    }
    if let Some(block_state) = block_state {
        entity.insert(FallingBlockState(block_state));
    }
    if is_projectile {
        entity.insert(ProjectileState);
    }
    if let Some(vehicle) = vehicle {
        entity.insert(VehicleKindState(vehicle.kind));
        if let Some(passenger) = vehicle.passenger {
            entity.insert(PassengerState(passenger));
        }
    }
    if let Some(animal) = animal {
        entity.insert(AnimalState(animal));
    }
    let ecs_entity = entity.id();
    world
        .resource_mut::<RuntimeEntityIndex>()
        .0
        .insert(id, ecs_entity);
    world.resource_mut::<RuntimeEntityUuids>().0.insert(uuid);
    if needs_breeding_tick {
        world.resource_mut::<BreedingTickEntities>().0.insert(id);
    }
    if is_sheep {
        world.resource_mut::<SheepEntities>().0.insert(id);
    }
    true
}

fn restore_snapshot_in_world(
    world: &mut World,
    snapshot: EntitySnapshot,
    allow_type_change: bool,
) -> bool {
    let Some(&ecs_entity) = world.resource::<RuntimeEntityIndex>().0.get(&snapshot.id) else {
        return false;
    };
    let Ok(current) = world.get_entity(ecs_entity) else {
        return false;
    };
    let identity_matches = current
        .get::<StableIdentity>()
        .is_some_and(|identity| identity.id == snapshot.id && identity.uuid == snapshot.uuid);
    let type_matches = current.get::<EntityTypeState>().is_some_and(|entity_type| {
        entity_type.protocol_id == snapshot.type_id && *entity_type.name == *snapshot.type_name
    });
    if !identity_matches || (!allow_type_change && !type_matches) {
        return false;
    }

    let needs_breeding_tick = snapshot.lifecycle == EntityLifecycle::Alive
        && snapshot
            .animal
            .is_some_and(AnimalBreedingState::needs_breeding_tick);
    let is_sheep = snapshot.lifecycle == EntityLifecycle::Alive
        && snapshot.type_name == "minecraft:sheep"
        && snapshot
            .animal
            .is_some_and(|animal| animal.sheep_wool.is_some());
    let is_projectile = snapshot.type_name == "minecraft:arrow"
        || snapshot.retained.hurting_projectile_state.is_some()
        || snapshot.retained.throwable_projectile_state.is_some();
    let EntitySnapshot {
        id,
        uuid: _,
        type_id,
        type_name,
        position,
        rotation,
        velocity,
        on_ground,
        item_stack,
        experience_value,
        block_state,
        lifecycle,
        health,
        attributes,
        goal,
        vehicle,
        animal,
        retained,
    } = snapshot;
    let EntityRetainedState {
        path,
        living: retained_living,
        fall_distance,
        remaining_fire_ticks,
        active_effects,
        arrow_state,
        hurting_projectile_state,
        throwable_projectile_state,
        last_damage_tick,
        death_remove_tick,
        sheep_grazing_ticks,
        spawn_tick,
        item_pickup_ready_tick,
        item_pickup_owner_block,
        item_pickup_claim,
        villager_food_recipient,
        primed_tnt,
        pending_explosion,
        crossbow_attack,
        bow_attack,
        blaze_attack,
        ghast_attack,
        breeze_attack,
        witch_attack,
        witch_potion,
        dragon_air,
        dragon_breath_cloud,
        guardian_beam,
        warden_sonic_boom,
        shulker_attack,
        shulker_bullet,
        evoker_attack,
        evoker_fangs,
        villager,
        villager_brain,
        villager_gossip,
        villager_merchant,
        villager_population,
        zombie_villager_conversion,
    } = retained;
    let living_state = living_state_from_snapshot(health, lifecycle, retained_living);
    if living_state.validate().is_err() {
        return false;
    }
    let active_effects = match active_effects {
        Some(snapshot) => match active_effects_state_from_snapshot(Some(snapshot)) {
            Some(state) => Some(state),
            None => return false,
        },
        None => None,
    };

    {
        let Ok(mut entity) = world.get_entity_mut(ecs_entity) else {
            return false;
        };
        entity.insert((
            entity_type_state(type_id, type_name),
            TransformState { position, rotation },
            MotionState {
                velocity,
                on_ground,
                fall_distance,
            },
            LifecycleState(lifecycle),
            LivingState {
                state: living_state,
                attributes,
            },
            AiGoalState(goal),
            AiPathState(path),
            GameplayDecisionState {
                arrow_state,
                hurting_projectile_state,
                throwable_projectile_state,
                remaining_fire_ticks,
                last_damage_tick,
                death_remove_tick,
                sheep_grazing_ticks,
                spawn_tick,
                item_pickup_ready_tick,
                item_pickup_owner_block,
                item_pickup_claim,
                villager_food_recipient,
                primed_tnt,
                pending_explosion,
                crossbow_attack,
                bow_attack,
                blaze_attack,
                ghast_attack,
                breeze_attack,
                witch_attack,
                witch_potion,
                dragon_air,
                dragon_breath_cloud,
                guardian_beam,
                warden_sonic_boom,
                shulker_attack,
                shulker_bullet,
                evoker_attack,
                evoker_fangs,
                villager,
                villager_brain,
                villager_gossip,
                villager_merchant,
                villager_population,
                zombie_villager_conversion,
            },
        ));
        replace_optional_component(&mut entity, active_effects);
        replace_optional_component(&mut entity, item_stack.map(ItemStackState));
        replace_optional_component(&mut entity, experience_value.map(ExperienceState));
        replace_optional_component(&mut entity, block_state.map(FallingBlockState));
        replace_optional_component(&mut entity, is_projectile.then_some(ProjectileState));
        replace_optional_component(
            &mut entity,
            vehicle
                .as_ref()
                .map(|vehicle| VehicleKindState(vehicle.kind)),
        );
        replace_optional_component(
            &mut entity,
            vehicle
                .and_then(|vehicle| vehicle.passenger)
                .map(PassengerState),
        );
        replace_optional_component(&mut entity, animal.map(AnimalState));
    }

    if needs_breeding_tick {
        world.resource_mut::<BreedingTickEntities>().0.insert(id);
    } else {
        world.resource_mut::<BreedingTickEntities>().0.remove(&id);
    }
    if is_sheep {
        world.resource_mut::<SheepEntities>().0.insert(id);
    } else {
        world.resource_mut::<SheepEntities>().0.remove(&id);
    }
    true
}

fn replace_optional_component<T: Component>(
    entity: &mut bevy_ecs::world::EntityWorldMut<'_>,
    component: Option<T>,
) {
    if let Some(component) = component {
        entity.insert(component);
    } else {
        entity.remove::<T>();
    }
}

fn snapshot_from_world(world: &World, id: EntityId) -> Option<EntitySnapshot> {
    let ecs_entity = *world.resource::<RuntimeEntityIndex>().0.get(&id)?;
    let entity = world.get_entity(ecs_entity).ok()?;
    snapshot_from_entity(&entity)
}

fn snapshot_from_entity(entity: &EntityRef<'_>) -> Option<EntitySnapshot> {
    let identity = entity.get::<StableIdentity>()?;
    let entity_type = entity.get::<EntityTypeState>()?;
    let transform = entity.get::<TransformState>()?;
    let motion = entity.get::<MotionState>()?;
    let lifecycle = entity.get::<LifecycleState>()?;
    let living = entity.get::<LivingState>()?;
    let goal = entity.get::<AiGoalState>()?;
    let path = entity.get::<AiPathState>()?;
    let gameplay = entity.get::<GameplayDecisionState>()?;
    let vehicle = entity.get::<VehicleKindState>().map(|kind| VehicleState {
        kind: kind.0,
        passenger: entity.get::<PassengerState>().map(|passenger| passenger.0),
    });

    Some(EntitySnapshot {
        id: identity.id,
        uuid: identity.uuid,
        type_id: entity_type.protocol_id,
        type_name: entity_type.name.to_string(),
        position: transform.position,
        rotation: transform.rotation,
        velocity: motion.velocity,
        on_ground: motion.on_ground,
        item_stack: entity.get::<ItemStackState>().map(|stack| stack.0.clone()),
        experience_value: entity
            .get::<ExperienceState>()
            .map(|experience| experience.0),
        block_state: entity.get::<FallingBlockState>().map(|block| block.0),
        lifecycle: lifecycle.0,
        health: living.state.health,
        attributes: living.attributes.clone(),
        goal: goal.0.clone(),
        vehicle,
        animal: entity.get::<AnimalState>().map(|state| state.0),
        retained: EntityRetainedState {
            path: path.0,
            living: EntityLivingRetainedState {
                absorption: living.state.absorption,
                invulnerable_time: living.state.invulnerable_time,
                hurt_time: living.state.hurt_time,
                last_hurt: living.state.last_hurt,
                death_time: living.state.death_time,
            },
            fall_distance: motion.fall_distance,
            remaining_fire_ticks: gameplay.remaining_fire_ticks,
            active_effects: active_effects_snapshot(entity.get::<ActiveEffectsState>()),
            arrow_state: gameplay.arrow_state,
            hurting_projectile_state: gameplay.hurting_projectile_state,
            throwable_projectile_state: gameplay.throwable_projectile_state,
            last_damage_tick: gameplay.last_damage_tick,
            death_remove_tick: gameplay.death_remove_tick,
            sheep_grazing_ticks: gameplay.sheep_grazing_ticks,
            spawn_tick: gameplay.spawn_tick,
            item_pickup_ready_tick: gameplay.item_pickup_ready_tick,
            item_pickup_owner_block: gameplay.item_pickup_owner_block,
            item_pickup_claim: gameplay.item_pickup_claim,
            villager_food_recipient: gameplay.villager_food_recipient,
            primed_tnt: gameplay.primed_tnt,
            pending_explosion: gameplay.pending_explosion,
            crossbow_attack: gameplay.crossbow_attack,
            bow_attack: gameplay.bow_attack,

            blaze_attack: gameplay.blaze_attack,
            ghast_attack: gameplay.ghast_attack,
            breeze_attack: gameplay.breeze_attack,
            witch_attack: gameplay.witch_attack,
            witch_potion: gameplay.witch_potion,
            dragon_air: gameplay.dragon_air,
            dragon_breath_cloud: gameplay.dragon_breath_cloud.clone(),
            guardian_beam: gameplay.guardian_beam,
            warden_sonic_boom: gameplay.warden_sonic_boom,
            shulker_attack: gameplay.shulker_attack,
            shulker_bullet: gameplay.shulker_bullet,
            evoker_attack: gameplay.evoker_attack,
            evoker_fangs: gameplay.evoker_fangs,
            villager: gameplay.villager,
            villager_brain: gameplay.villager_brain.clone(),
            villager_gossip: gameplay.villager_gossip.clone(),
            villager_merchant: gameplay.villager_merchant.clone(),
            villager_population: gameplay.villager_population.clone(),
            zombie_villager_conversion: gameplay.zombie_villager_conversion,
        },
    })
}

fn entity_goal_checkpoint_from_world(world: &World, id: EntityId) -> Option<EntityGoalCheckpoint> {
    let ecs_entity = *world.resource::<RuntimeEntityIndex>().0.get(&id)?;
    let entity = world.get_entity(ecs_entity).ok()?;
    entity_goal_checkpoint_from_entity(&entity)
}

fn entity_goal_checkpoint_from_entity(entity: &EntityRef<'_>) -> Option<EntityGoalCheckpoint> {
    let identity = entity.get::<StableIdentity>()?;
    let transform = entity.get::<TransformState>()?;
    let motion = entity.get::<MotionState>()?;
    let lifecycle = entity.get::<LifecycleState>()?;
    let goal = entity.get::<AiGoalState>()?;
    let path = entity.get::<AiPathState>()?;
    Some(EntityGoalCheckpoint {
        id: identity.id,
        position: transform.position,
        rotation: transform.rotation,
        velocity: motion.velocity,
        on_ground: motion.on_ground,
        lifecycle: lifecycle.0,
        goal: goal.0.clone(),
        path: path.0,
    })
}

fn villager_job_site(gameplay: &GameplayDecisionState, position: Vec3) -> Option<Vec3> {
    match (gameplay.villager_brain.as_ref(), gameplay.villager) {
        (Some(brain), _) => brain.pois.job_site,
        (None, Some(villager)) => {
            crate::villager_26_1_2::default_villager_pois(position, villager.profession).job_site
        }
        (None, None) => None,
    }
}

fn entity_simulation_result_from_world(
    world: &World,
    id: EntityId,
) -> Option<EntitySimulationResult> {
    let ecs_entity = *world.resource::<RuntimeEntityIndex>().0.get(&id)?;
    let entity = world.get_entity(ecs_entity).ok()?;
    entity_simulation_result_from_entity(&entity)
}

fn villager_population_active(
    entity_type: &EntityTypeState,
    gameplay: &GameplayDecisionState,
) -> bool {
    entity_type.villager
        && gameplay
            .villager_population
            .as_ref()
            .is_some_and(|population| {
                population.age_ticks != 0
                    || population.food_level != 0
                    || !population.inventory.is_empty()
                    || population.pending_birth.is_some()
            })
}

fn entity_simulation_result_from_entity(entity: &EntityRef<'_>) -> Option<EntitySimulationResult> {
    let identity = entity.get::<StableIdentity>()?;
    let entity_type = entity.get::<EntityTypeState>()?;
    let transform = entity.get::<TransformState>()?;
    let motion = entity.get::<MotionState>()?;
    let lifecycle = entity.get::<LifecycleState>()?;
    let goal = entity.get::<AiGoalState>()?;
    let gameplay = entity.get::<GameplayDecisionState>()?;
    entity_simulation_result_from_components(
        identity,
        entity_type,
        transform,
        motion,
        lifecycle,
        goal,
        gameplay,
        entity.get::<AnimalState>().map(|state| state.0),
        entity.get::<ItemStackState>().is_none()
            && entity.get::<ExperienceState>().is_none()
            && entity.get::<FallingBlockState>().is_none()
            && entity.get::<VehicleKindState>().is_none(),
    )
}

#[allow(clippy::too_many_arguments)]
fn entity_simulation_result_from_components(
    identity: &StableIdentity,
    entity_type: &EntityTypeState,
    transform: &TransformState,
    motion: &MotionState,
    lifecycle: &LifecycleState,
    goal: &AiGoalState,
    gameplay: &GameplayDecisionState,
    animal: Option<AnimalBreedingState>,
    ordinary_living: bool,
) -> Option<EntitySimulationResult> {
    if lifecycle.0 != EntityLifecycle::Alive {
        return None;
    }
    let motion =
        motion_state_from_components(identity, entity_type, transform, motion, goal, gameplay);
    Some(entity_simulation_result_from_motion(
        entity_type,
        gameplay,
        animal,
        ordinary_living,
        motion,
    ))
}

fn entity_physics_kind(
    entity_type: &EntityTypeState,
    gameplay: &GameplayDecisionState,
    ordinary_living: bool,
) -> EntityPhysicsKind {
    let type_name = &*entity_type.name;
    let arrow_state = gameplay.arrow_state;
    let hurting_projectile_state = gameplay.hurting_projectile_state;
    let throwable_projectile_state = gameplay.throwable_projectile_state;
    if type_name == "minecraft:arrow" {
        EntityPhysicsKind::ArrowProjectile {
            revision: arrow_state.map(|state| state.projectile.revision),
            embedded_block: arrow_state
                .filter(|state| state.in_ground)
                .and_then(|state| state.last_block_position),
        }
    } else if type_name == "minecraft:ender_dragon" {
        EntityPhysicsKind::ExternalFlight
    } else if matches!(
        type_name,
        "minecraft:evoker_fangs" | "minecraft:area_effect_cloud"
    ) {
        EntityPhysicsKind::Immobile
    } else if type_name == "minecraft:shulker_bullet" {
        EntityPhysicsKind::ShulkerBullet {
            revision: hurting_projectile_state.map(|state| state.projectile.revision),
        }
    } else if let Some(state) = hurting_projectile_state {
        EntityPhysicsKind::HurtingProjectile {
            revision: Some(state.projectile.revision),
            acceleration_power_bits: state.acceleration_power.to_bits(),
        }
    } else if let Some(state) = throwable_projectile_state {
        EntityPhysicsKind::ThrowableProjectile {
            revision: Some(state.projectile.revision),
            gravity_bits: 0.05_f64.to_bits(),
        }
    } else if crate::aquatic_motion::Swimmer::for_type(type_name)
        == crate::aquatic_motion::Swimmer::Fish
    {
        EntityPhysicsKind::FishLiving
    } else if crate::aquatic_motion::Swimmer::for_type(type_name)
        == crate::aquatic_motion::Swimmer::Squid
    {
        EntityPhysicsKind::SquidLiving
    } else if entity_type.aquatic_physics {
        EntityPhysicsKind::AquaticLiving
    } else if type_name == "minecraft:falling_block" {
        EntityPhysicsKind::FallingBlock
    } else if ordinary_living {
        if entity_type.powder_snow_walkable {
            EntityPhysicsKind::PowderSnowWalkableLiving
        } else {
            EntityPhysicsKind::Living
        }
    } else {
        EntityPhysicsKind::Default
    }
}

fn entity_simulation_result_from_motion(
    entity_type: &EntityTypeState,
    gameplay: &GameplayDecisionState,
    animal: Option<AnimalBreedingState>,
    ordinary_living: bool,
    motion: EntityMotionState,
) -> EntitySimulationResult {
    let type_name = &*entity_type.name;
    let kind = entity_physics_kind(entity_type, gameplay, ordinary_living);

    EntitySimulationResult {
        hostile: entity_type.hostile,
        rotation: motion.rotation,
        villager_population_active: villager_population_active(entity_type, gameplay),
        villager: entity_type.villager,
        item: entity_type.item,
        physics: EntityPhysicsQuery {
            id: motion.id,
            position: motion.position,
            velocity: motion.velocity,
            aabb: if animal.is_some_and(AnimalBreedingState::is_baby) {
                crate::natural_spawn_26_1_2::entity_geometry(type_name, animal).aabb
            } else {
                entity_type.geometry.aabb
            },
            on_ground: motion.on_ground,
            fall_distance: motion.fall_distance,
            goal_fence: motion.goal_fence,
            kind,
        },
    }
}

fn retarget_shulker_bullet_snapshot(
    expected: &EntitySnapshot,
    targets: &HashMap<i32, Vec3>,
) -> Option<EntitySnapshot> {
    const TARGET_SPEED: f64 = 0.15;
    const STEERING: f64 = 0.2;

    let target_entity_id = expected.retained.shulker_bullet?.target_entity_id;
    let target = targets.get(&target_entity_id).copied()?;
    let state = expected.retained.hurting_projectile_state?;
    let delta = Vec3::new(
        target.x - expected.position.x,
        target.y - expected.position.y,
        target.z - expected.position.z,
    );
    let length_squared = delta.x * delta.x + delta.y * delta.y + delta.z * delta.z;
    if !length_squared.is_finite() || length_squared <= 1.0e-14 {
        return None;
    }
    let scale = TARGET_SPEED / length_squared.sqrt();
    let desired = Vec3::new(delta.x * scale, delta.y * scale, delta.z * scale);
    let velocity = Vec3::new(
        expected.velocity.x + (desired.x - expected.velocity.x) * STEERING,
        expected.velocity.y + (desired.y - expected.velocity.y) * STEERING,
        expected.velocity.z + (desired.z - expected.velocity.z) * STEERING,
    );
    let projectile_velocity =
        crate::projectile_26_1_2::Vec3::new(velocity.x, velocity.y, velocity.z);
    let next_state = state.retarget_velocity(projectile_velocity).ok()?;
    let mut next = expected.clone();
    next.velocity = velocity;
    next.rotation = Rotation {
        yaw: next_state.projectile.rotation.yaw,
        pitch: next_state.projectile.rotation.pitch,
        head_yaw: next_state.projectile.rotation.yaw,
    };
    next.retained.hurting_projectile_state = Some(next_state);
    Some(next)
}

const VILLAGER_BRAIN_TICK_INTERVAL: u64 = 20;
const VILLAGER_RESTOCK_REACH_SQUARED: f64 = 4.0;
const VILLAGER_GOSSIP_REACH_SQUARED: f64 = 5.0;
const VILLAGER_GOSSIP_COOLDOWN_TICKS: u64 = 1_200;
const VILLAGER_GOSSIP_CELL_SIZE: f64 = 3.0;

#[derive(Clone)]
struct PlannedVillager {
    ecs_entity: EcsEntity,
    id: EntityId,
    uuid: Uuid,
    position: Vec3,
    activity: crate::villager_26_1_2::VillagerActivity,
    interaction_target: Option<EntityId>,
    last_gossip_time: u64,
    update: Option<PlannedVillagerUpdate>,
}

#[derive(Clone)]
struct PlannedVillagerUpdate {
    goal: GoalState,
    population_pending_birth: bool,
    villager: crate::VillagerData,
    brain: crate::villager_26_1_2::VillagerBrainState,
    gossip: Option<crate::villager_gossip_26_1_2::VillagerGossipState>,
    merchant: Option<crate::villager_merchant_26_1_2::VillagerMerchantState>,
}

fn plan_region_villager_updates(
    world: &World,
    region: crate::RegionKey,
    ordered_entities: &[(EntityId, EcsEntity)],
    active_chunks: Option<&HashSet<(i32, i32)>>,
    tick: u64,
    inputs: Option<&crate::regional::RegionalVillagerGoalTickInputs>,
) -> (
    HashMap<EntityId, GoalState>,
    Vec<crate::EntityVillagerGoalUpdate>,
    Vec<EntityId>,
) {
    let Some(inputs) = inputs else {
        return (HashMap::new(), Vec::new(), Vec::new());
    };
    let Some(profile) = inputs.profile.validated().ok() else {
        return (HashMap::new(), Vec::new(), Vec::new());
    };
    let mut planned = HashMap::<EntityId, PlannedVillager>::new();
    let mut cells = HashMap::<(i32, i32, i32), Vec<EntityId>>::new();
    let mut due_ids = Vec::new();
    let mut goal_overrides = HashMap::new();
    let mut cross_region_villager_candidates = Vec::new();
    for &(id, ecs_entity) in ordered_entities {
        let Ok(entity) = world.get_entity(ecs_entity) else {
            continue;
        };
        if entity
            .get::<EntityTypeState>()
            .is_none_or(|entity_type| entity_type.name.as_ref() != "minecraft:villager")
            || entity
                .get::<LifecycleState>()
                .is_none_or(|lifecycle| lifecycle.0 != EntityLifecycle::Alive)
        {
            continue;
        }
        let Some(transform) = entity.get::<TransformState>() else {
            continue;
        };
        let position = transform.position;
        let chunk = (
            (position.x.floor() as i32).div_euclid(16),
            (position.z.floor() as i32).div_euclid(16),
        );
        if active_chunks.is_some_and(|chunks| !chunks.contains(&chunk)) {
            continue;
        }
        let Ok((identity, goal, gameplay)) =
            entity.get_components::<(&StableIdentity, &AiGoalState, &GameplayDecisionState)>()
        else {
            continue;
        };
        let Some(villager) = gameplay.villager else {
            continue;
        };
        let brain = current_villager_brain(position, villager, gameplay.villager_brain.as_ref());
        let base_brain_state = (
            brain.activity,
            brain.interaction_target,
            brain.last_gossip_time,
        );
        let due = villager_brain_due_for_tick(
            id,
            brain.schedule,
            brain.override_expires_tick,
            tick,
            inputs.day_time,
            &inputs.profile,
        );
        let update = if due {
            due_ids.push(id);
            let mut update = PlannedVillagerUpdate {
                goal: goal.0.clone(),
                population_pending_birth: gameplay
                    .villager_population
                    .as_ref()
                    .is_some_and(|population| population.pending_birth.is_some()),
                brain,
                gossip: gameplay.villager_gossip.clone(),
                merchant: gameplay.villager_merchant.clone(),
                villager,
            };
            if let Ok(plan) = profile.plan(&update.brain, tick, inputs.day_time) {
                if !update.population_pending_birth && update.goal != plan.goal {
                    goal_overrides.insert(id, plan.goal.clone());
                }
                update.brain = plan.state;
                if let Some(mut gossip) = update.gossip.clone()
                    && gossip.decay(inputs.day_time).ok() == Some(true)
                {
                    update.gossip = Some(gossip);
                }
                if villager_can_restock_at_job_site(position, &update.brain)
                    && let Some(mut merchant) = update.merchant.clone()
                    && merchant.restock(inputs.day_time).ok() == Some(true)
                {
                    update.merchant = Some(merchant);
                }
                if let Some(offer) = inputs.profession_offers.get(&id)
                    && update.villager.profession == crate::VillagerProfession::None
                    && update.villager.level == 1
                    && update.merchant.is_none()
                    && update.brain.schedule == crate::villager_26_1_2::VillagerScheduleKind::Adult
                {
                    update.villager.profession = offer.profession;
                    update.merchant = Some(offer.merchant.clone());
                }
            }
            Some(update)
        } else {
            None
        };
        let (activity, interaction_target, last_gossip_time) =
            update.as_ref().map_or(base_brain_state, |planned_update| {
                (
                    planned_update.brain.activity,
                    planned_update.brain.interaction_target,
                    planned_update.brain.last_gossip_time,
                )
            });
        planned.insert(
            id,
            PlannedVillager {
                ecs_entity,
                id,
                uuid: identity.uuid,
                position,
                activity,
                interaction_target,
                last_gossip_time,
                update,
            },
        );
        cells
            .entry(villager_gossip_cell(position))
            .or_default()
            .push(id);
        if villager_is_near_region_boundary(region, position) {
            cross_region_villager_candidates.push(id);
        }
    }

    let mut reserved = HashSet::new();
    for receiver_id in due_ids {
        if reserved.contains(&receiver_id) {
            continue;
        }
        let Some(receiver) = planned.get(&receiver_id) else {
            continue;
        };
        if !villager_gossip_activity_allows_transfer(receiver.activity)
            || !villager_gossip_cooldown_ready(tick, receiver.last_gossip_time)
        {
            continue;
        }
        let Some(source_id) =
            select_local_villager_gossip_target(receiver, &planned, &cells, &reserved, tick)
        else {
            continue;
        };
        let receiver_uuid = receiver.uuid;
        if planned
            .get(&source_id)
            .is_none_or(|source| source.update.is_none())
        {
            let Some(source) = planned.get(&source_id) else {
                continue;
            };
            let Ok(entity) = world.get_entity(source.ecs_entity) else {
                continue;
            };
            let Ok((goal, gameplay)) =
                entity.get_components::<(&AiGoalState, &GameplayDecisionState)>()
            else {
                continue;
            };
            let Some(villager) = gameplay.villager else {
                continue;
            };
            let source_update = PlannedVillagerUpdate {
                goal: goal.0.clone(),
                population_pending_birth: gameplay
                    .villager_population
                    .as_ref()
                    .is_some_and(|population| population.pending_birth.is_some()),
                brain: current_villager_brain(
                    source.position,
                    villager,
                    gameplay.villager_brain.as_ref(),
                ),
                gossip: gameplay.villager_gossip.clone(),
                merchant: gameplay.villager_merchant.clone(),
                villager,
            };
            planned
                .get_mut(&source_id)
                .expect("selected gossip source remains planned")
                .update = Some(source_update);
        }
        let Some(source) = planned.get(&source_id) else {
            continue;
        };
        let Some(source_update) = source.update.as_ref() else {
            continue;
        };
        let source_uuid = source.uuid;
        let source_gossip = source_update.gossip.clone().unwrap_or_default();
        let Some(receiver_update) = planned
            .get(&receiver_id)
            .and_then(|receiver| receiver.update.as_ref())
        else {
            continue;
        };
        let mut receiver_gossip = receiver_update.gossip.clone().unwrap_or_default();
        if receiver_gossip
            .transfer_from_seeded(
                &source_gossip,
                villager_gossip_seed(receiver_uuid, source_uuid, tick),
                crate::villager_gossip_26_1_2::MAX_TRANSFER_COUNT,
            )
            .is_err()
        {
            continue;
        }
        let Some(receiver_update) = planned
            .get_mut(&receiver_id)
            .and_then(|receiver| receiver.update.as_mut())
        else {
            continue;
        };
        receiver_update.brain.interaction_target = Some(source_id);
        receiver_update.brain.last_gossip_time = tick;
        receiver_update.gossip = Some(receiver_gossip);
        let Some(source_update) = planned
            .get_mut(&source_id)
            .and_then(|source| source.update.as_mut())
        else {
            continue;
        };
        source_update.brain.last_gossip_time = tick;
        reserved.insert(receiver_id);
        reserved.insert(source_id);
    }

    let mut updates = planned
        .into_values()
        .filter_map(|planned| {
            let update = planned.update?;
            let entity = world.get_entity(planned.ecs_entity).ok()?;
            let gameplay = entity.get::<GameplayDecisionState>()?;
            (gameplay.villager != Some(update.villager)
                || gameplay.villager_brain.as_ref() != Some(&update.brain)
                || gameplay.villager_gossip != update.gossip
                || gameplay.villager_merchant != update.merchant)
                .then(|| {
                    snapshot_from_entity(&entity).map(|expected| crate::EntityVillagerGoalUpdate {
                        expected,
                        villager: update.villager,
                        brain: update.brain,
                        gossip: update.gossip,
                        merchant: update.merchant,
                    })
                })
                .flatten()
        })
        .collect::<Vec<_>>();
    updates.sort_unstable_by_key(|update| update.expected.id);
    (goal_overrides, updates, cross_region_villager_candidates)
}

fn villager_is_near_region_boundary(region: crate::RegionKey, position: Vec3) -> bool {
    const GOSSIP_REACH: f64 = 2.236_067_977_499_79;
    const REGION_SIZE_BLOCKS: f64 = crate::REGION_SIZE_CHUNKS as f64 * 16.0;
    let min_x = f64::from(region.x) * REGION_SIZE_BLOCKS;
    let min_z = f64::from(region.z) * REGION_SIZE_BLOCKS;
    let max_x = min_x + REGION_SIZE_BLOCKS;
    let max_z = min_z + REGION_SIZE_BLOCKS;
    position.x - min_x <= GOSSIP_REACH
        || max_x - position.x <= GOSSIP_REACH
        || position.z - min_z <= GOSSIP_REACH
        || max_z - position.z <= GOSSIP_REACH
}

fn current_villager_brain(
    position: Vec3,
    villager: crate::VillagerData,
    brain: Option<&crate::villager_26_1_2::VillagerBrainState>,
) -> crate::villager_26_1_2::VillagerBrainState {
    brain.cloned().unwrap_or_else(|| {
        crate::villager_26_1_2::VillagerBrainState::adult(
            crate::villager_26_1_2::default_villager_pois(position, villager.profession),
        )
    })
}

fn villager_schedule_boundary(
    profile: &crate::villager_26_1_2::VillagerBrainProfile,
    schedule: crate::villager_26_1_2::VillagerScheduleKind,
    day_time: i64,
) -> bool {
    let entries = match schedule {
        crate::villager_26_1_2::VillagerScheduleKind::Adult => &profile.adult_schedule,
        crate::villager_26_1_2::VillagerScheduleKind::Baby => &profile.baby_schedule,
    };
    let normalized = day_time.rem_euclid(24_000);
    entries.iter().any(|entry| entry.day_time == normalized)
}

fn villager_brain_due_for_tick(
    entity: EntityId,
    schedule: crate::villager_26_1_2::VillagerScheduleKind,
    override_expires_tick: Option<u64>,
    lifecycle_tick: u64,
    day_time: i64,
    profile: &crate::villager_26_1_2::VillagerBrainProfile,
) -> bool {
    override_expires_tick.is_some_and(|expires| lifecycle_tick >= expires)
        || villager_schedule_boundary(profile, schedule, day_time)
        || lifecycle_tick
            .wrapping_add(u64::from(entity.0.unsigned_abs()))
            .is_multiple_of(VILLAGER_BRAIN_TICK_INTERVAL)
}

fn villager_can_restock_at_job_site(
    position: Vec3,
    brain: &crate::villager_26_1_2::VillagerBrainState,
) -> bool {
    if brain.activity != crate::villager_26_1_2::VillagerActivity::Work {
        return false;
    }
    let Some(job_site) = brain.pois.job_site else {
        return false;
    };
    let dx = position.x - job_site.x;
    let dy = position.y - job_site.y;
    let dz = position.z - job_site.z;
    dx * dx + dy * dy + dz * dz <= VILLAGER_RESTOCK_REACH_SQUARED
}

fn villager_gossip_activity_allows_transfer(
    activity: crate::villager_26_1_2::VillagerActivity,
) -> bool {
    matches!(
        activity,
        crate::villager_26_1_2::VillagerActivity::Idle
            | crate::villager_26_1_2::VillagerActivity::Meet
    )
}

fn villager_gossip_cooldown_ready(timestamp: u64, last_gossip_time: u64) -> bool {
    timestamp < last_gossip_time
        || timestamp >= last_gossip_time.saturating_add(VILLAGER_GOSSIP_COOLDOWN_TICKS)
}

fn villager_gossip_cell(position: Vec3) -> (i32, i32, i32) {
    (
        (position.x / VILLAGER_GOSSIP_CELL_SIZE).floor() as i32,
        (position.y / VILLAGER_GOSSIP_CELL_SIZE).floor() as i32,
        (position.z / VILLAGER_GOSSIP_CELL_SIZE).floor() as i32,
    )
}

fn select_local_villager_gossip_target(
    receiver: &PlannedVillager,
    candidates: &HashMap<EntityId, PlannedVillager>,
    cells: &HashMap<(i32, i32, i32), Vec<EntityId>>,
    reserved: &HashSet<EntityId>,
    timestamp: u64,
) -> Option<EntityId> {
    let eligible = |target: EntityId| {
        if target == receiver.id || reserved.contains(&target) {
            return None;
        }
        let candidate = candidates.get(&target)?;
        if !villager_gossip_cooldown_ready(timestamp, candidate.last_gossip_time) {
            return None;
        }
        let delta = Vec3::new(
            receiver.position.x - candidate.position.x,
            receiver.position.y - candidate.position.y,
            receiver.position.z - candidate.position.z,
        );
        let distance = delta.x * delta.x + delta.y * delta.y + delta.z * delta.z;
        (distance <= VILLAGER_GOSSIP_REACH_SQUARED).then_some((target, distance))
    };
    if let Some(target) = receiver.interaction_target
        && eligible(target).is_some()
    {
        return Some(target);
    }
    let (cell_x, cell_y, cell_z) = villager_gossip_cell(receiver.position);
    let mut best = None::<(EntityId, f64)>;
    for x in (cell_x - 1)..=(cell_x + 1) {
        for y in (cell_y - 1)..=(cell_y + 1) {
            for z in (cell_z - 1)..=(cell_z + 1) {
                let Some(ids) = cells.get(&(x, y, z)) else {
                    continue;
                };
                for &target in ids {
                    let Some((target, distance)) = eligible(target) else {
                        continue;
                    };
                    if best.is_none_or(|(best_id, best_distance)| {
                        distance < best_distance || distance == best_distance && target < best_id
                    }) {
                        best = Some((target, distance));
                    }
                }
            }
        }
    }
    best.map(|(target, _)| target)
}

fn villager_gossip_seed(receiver: Uuid, source: Uuid, timestamp: u64) -> u64 {
    let receiver = receiver.as_u128();
    let source = source.as_u128();
    splitmix64_villager(
        (receiver as u64)
            ^ ((receiver >> 64) as u64).rotate_left(11)
            ^ (source as u64).rotate_left(23)
            ^ ((source >> 64) as u64).rotate_left(37)
            ^ timestamp.wrapping_mul(0x9E37_79B9_7F4A_7C15),
    )
}

fn splitmix64_villager(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut mixed = value;
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    mixed ^ (mixed >> 31)
}
fn simulation_fence_result_from_entity(
    entity: &EntityRef<'_>,
) -> Option<(EntityKinematicsFenceState, Option<EntitySimulationResult>)> {
    let (identity, entity_type, transform, motion, lifecycle, _, goal, _, gameplay) = entity
        .get_components::<(
            &StableIdentity,
            &EntityTypeState,
            &TransformState,
            &MotionState,
            &LifecycleState,
            &LivingState,
            &AiGoalState,
            &AiPathState,
            &GameplayDecisionState,
        )>()
        .ok()?;
    let vehicle_attached = entity.get::<VehicleKindState>().is_some();
    let state = kinematics_fence_state_from_components(
        identity,
        entity_type,
        transform,
        motion,
        lifecycle,
        goal,
        gameplay,
        vehicle_attached,
    );
    let ordinary_living = entity.get::<ItemStackState>().is_none()
        && entity.get::<ExperienceState>().is_none()
        && entity.get::<FallingBlockState>().is_none()
        && !state.vehicle_attached;
    let result =
        (state.lifecycle == EntityLifecycle::Alive && !state.vehicle_attached).then(|| {
            entity_simulation_result_from_motion(
                entity_type,
                gameplay,
                entity.get::<AnimalState>().map(|animal| animal.0),
                ordinary_living,
                state.motion,
            )
        });
    Some((state, result))
}

#[allow(clippy::too_many_arguments)]
fn kinematics_fence_state_from_components(
    identity: &StableIdentity,
    entity_type: &EntityTypeState,
    transform: &TransformState,
    motion: &MotionState,
    lifecycle: &LifecycleState,
    goal: &AiGoalState,
    gameplay: &GameplayDecisionState,
    vehicle_attached: bool,
) -> EntityKinematicsFenceState {
    EntityKinematicsFenceState {
        uuid: identity.uuid,
        lifecycle: lifecycle.0,
        motion: motion_state_from_components(
            identity,
            entity_type,
            transform,
            motion,
            goal,
            gameplay,
        ),
        pickup_claimed: gameplay.item_pickup_claim.is_some(),
        vehicle_attached,
    }
}

fn motion_state_from_components(
    identity: &StableIdentity,
    entity_type: &EntityTypeState,
    transform: &TransformState,
    motion: &MotionState,
    goal: &AiGoalState,
    gameplay: &GameplayDecisionState,
) -> EntityMotionState {
    let arrow_state = gameplay.arrow_state;
    let hurting_projectile_state = gameplay.hurting_projectile_state;
    let throwable_projectile_state = gameplay.throwable_projectile_state;
    EntityMotionState {
        id: identity.id,
        position: transform.position,
        rotation: transform.rotation,
        velocity: motion.velocity,
        on_ground: motion.on_ground,
        fall_distance: motion.fall_distance,
        goal_fence: crate::EntityGoalFence::from_goal(&goal.0),
        is_item: &*entity_type.name == "minecraft:item",
        is_experience: &*entity_type.name == "minecraft:experience_orb",
        is_arrow: &*entity_type.name == "minecraft:arrow",
        arrow_revision: arrow_state.map(|state| state.projectile.revision),
        arrow_embedded_block: arrow_state
            .filter(|state| state.in_ground)
            .and_then(|state| state.last_block_position),
        is_hurting_projectile: hurting_projectile_state.is_some(),
        hurting_projectile_revision: hurting_projectile_state
            .map(|state| state.projectile.revision),
        is_throwable_projectile: throwable_projectile_state.is_some(),
        throwable_projectile_revision: throwable_projectile_state
            .map(|state| state.projectile.revision),
        sends_velocity: !matches!(
            &*entity_type.name,
            "minecraft:item" | "minecraft:experience_orb"
        ),
    }
}

fn entity_view_from_world(world: &World, id: EntityId) -> Option<EntityView<'_>> {
    let ecs_entity = *world.resource::<RuntimeEntityIndex>().0.get(&id)?;
    let entity = world.get_entity(ecs_entity).ok()?;
    let identity = entity.get::<StableIdentity>()?;
    let entity_type = entity.get::<EntityTypeState>()?;
    let transform = entity.get::<TransformState>()?;
    let motion = entity.get::<MotionState>()?;
    let lifecycle = entity.get::<LifecycleState>()?;
    let living = entity.get::<LivingState>()?;
    let goal = entity.get::<AiGoalState>()?;
    let path = entity.get::<AiPathState>()?;
    let gameplay = entity.get::<GameplayDecisionState>()?;
    let vehicle = entity.get::<VehicleKindState>().map(|kind| VehicleState {
        kind: kind.0,
        passenger: entity.get::<PassengerState>().map(|passenger| passenger.0),
    });

    Some(EntityView {
        id: identity.id,
        uuid: identity.uuid,
        type_id: entity_type.protocol_id,
        type_name: &entity_type.name,
        position: transform.position,
        rotation: transform.rotation,
        velocity: motion.velocity,
        on_ground: motion.on_ground,
        item_stack: entity.get::<ItemStackState>().map(|stack| stack.0.clone()),
        experience_value: entity
            .get::<ExperienceState>()
            .map(|experience| experience.0),
        block_state: entity.get::<FallingBlockState>().map(|block| block.0),
        lifecycle: lifecycle.0,
        health: living.state.health,
        attributes: &living.attributes,
        goal: &goal.0,
        vehicle,
        animal: entity.get::<AnimalState>().map(|state| state.0),
        retained: EntityRetainedState {
            path: path.0,
            living: EntityLivingRetainedState {
                absorption: living.state.absorption,
                invulnerable_time: living.state.invulnerable_time,
                hurt_time: living.state.hurt_time,
                last_hurt: living.state.last_hurt,
                death_time: living.state.death_time,
            },
            fall_distance: motion.fall_distance,
            remaining_fire_ticks: gameplay.remaining_fire_ticks,
            active_effects: active_effects_snapshot(entity.get::<ActiveEffectsState>()),
            arrow_state: gameplay.arrow_state,
            hurting_projectile_state: gameplay.hurting_projectile_state,
            throwable_projectile_state: gameplay.throwable_projectile_state,
            last_damage_tick: gameplay.last_damage_tick,
            death_remove_tick: gameplay.death_remove_tick,
            sheep_grazing_ticks: gameplay.sheep_grazing_ticks,
            spawn_tick: gameplay.spawn_tick,
            item_pickup_ready_tick: gameplay.item_pickup_ready_tick,
            item_pickup_owner_block: gameplay.item_pickup_owner_block,
            item_pickup_claim: gameplay.item_pickup_claim,
            villager_food_recipient: gameplay.villager_food_recipient,
            primed_tnt: gameplay.primed_tnt,
            pending_explosion: gameplay.pending_explosion,
            crossbow_attack: gameplay.crossbow_attack,
            bow_attack: gameplay.bow_attack,
            blaze_attack: gameplay.blaze_attack,
            ghast_attack: gameplay.ghast_attack,
            breeze_attack: gameplay.breeze_attack,
            witch_attack: gameplay.witch_attack,
            witch_potion: gameplay.witch_potion,
            dragon_air: gameplay.dragon_air,
            dragon_breath_cloud: gameplay.dragon_breath_cloud.clone(),
            guardian_beam: gameplay.guardian_beam,
            warden_sonic_boom: gameplay.warden_sonic_boom,
            shulker_attack: gameplay.shulker_attack,
            shulker_bullet: gameplay.shulker_bullet,
            evoker_attack: gameplay.evoker_attack,
            evoker_fangs: gameplay.evoker_fangs,
            villager: gameplay.villager,
            villager_brain: gameplay.villager_brain.clone(),
            villager_gossip: gameplay.villager_gossip.clone(),
            villager_merchant: gameplay.villager_merchant.clone(),
            villager_population: gameplay.villager_population.clone(),
            zombie_villager_conversion: gameplay.zombie_villager_conversion,
        },
    })
}

fn normalized_snapshots_from_world(world: &World) -> Vec<EntitySnapshot> {
    world
        .resource::<RuntimeEntityIndex>()
        .0
        .keys()
        .map(|&id| {
            snapshot_from_world(world, id)
                .expect("runtime id index must reference a complete ECS entity")
        })
        .collect()
}

fn remove_from_world(world: &mut World, id: EntityId) -> Option<EntitySnapshot> {
    let snapshot = snapshot_from_world(world, id)?;
    let ecs_entity = world.resource_mut::<RuntimeEntityIndex>().0.remove(&id)?;
    world
        .resource_mut::<RuntimeEntityUuids>()
        .0
        .remove(&snapshot.uuid);
    world.resource_mut::<BreedingTickEntities>().0.remove(&id);
    world.resource_mut::<SheepEntities>().0.remove(&id);
    let removed = world.despawn(ecs_entity);
    debug_assert!(removed, "runtime id index referenced a missing entity");
    let indexed_entities = world
        .resource::<RuntimeEntityIndex>()
        .0
        .values()
        .copied()
        .collect::<Vec<_>>();
    for entity in indexed_entities {
        let Ok(mut entity) = world.get_entity_mut(entity) else {
            continue;
        };
        if entity
            .get::<PassengerState>()
            .is_some_and(|passenger| passenger.0 == id)
        {
            entity.remove::<PassengerState>();
        }
    }
    Some(snapshot)
}

fn apply_input_commands(world: &mut World) {
    let commands = std::mem::take(&mut world.resource_mut::<PendingInputCommands>().0);
    for command in commands {
        match command {
            EntityInputCommand::Insert(snapshot) => {
                let _ = insert_snapshot_into_world(world, *snapshot);
            }
            EntityInputCommand::SetGoal { id, goal } => {
                let Some(entity) = ecs_entity_for(world, id) else {
                    continue;
                };
                {
                    let Ok(mut entity) = world.get_entity_mut(entity) else {
                        continue;
                    };
                    let Some(mut current) = entity.get_mut::<AiGoalState>() else {
                        continue;
                    };
                    current.0 = goal;
                    let Some(mut path) = entity.get_mut::<AiPathState>() else {
                        continue;
                    };
                    path.0 = RetainedPathState::default();
                };
            }
            EntityInputCommand::ResetPath { id } => {
                let Some(entity) = ecs_entity_for(world, id) else {
                    continue;
                };
                let Ok(mut entity) = world.get_entity_mut(entity) else {
                    continue;
                };
                let Some(mut path) = entity.get_mut::<AiPathState>() else {
                    continue;
                };
                path.0 = RetainedPathState::default();
            }
            EntityInputCommand::SetItemStack { id, stack } => {
                let Some(entity) = ecs_entity_for(world, id) else {
                    continue;
                };
                {
                    let Ok(mut entity) = world.get_entity_mut(entity) else {
                        continue;
                    };
                    match stack {
                        Some(stack) => {
                            entity.insert(ItemStackState(stack));
                        }
                        None => {
                            entity.remove::<ItemStackState>();
                        }
                    }
                };
            }
            EntityInputCommand::SetVehicle { id, vehicle } => {
                let Some(entity) = ecs_entity_for(world, id) else {
                    continue;
                };
                {
                    let Ok(mut entity) = world.get_entity_mut(entity) else {
                        continue;
                    };
                    match vehicle {
                        Some(vehicle) => {
                            entity.insert(VehicleKindState(vehicle.kind));
                            match vehicle.passenger {
                                Some(passenger) => {
                                    entity.insert(PassengerState(passenger));
                                }
                                None => {
                                    entity.remove::<PassengerState>();
                                }
                            }
                        }
                        None => {
                            entity.remove::<VehicleKindState>();
                            entity.remove::<PassengerState>();
                        }
                    }
                };
            }
            EntityInputCommand::SetAnimalState { id, animal } => {
                let Some(entity) = ecs_entity_for(world, id) else {
                    continue;
                };
                let (needs_breeding_tick, is_sheep) = {
                    let Ok(mut entity) = world.get_entity_mut(entity) else {
                        continue;
                    };
                    if !entity.contains::<AnimalState>() {
                        continue;
                    }
                    let is_alive = entity
                        .get::<LifecycleState>()
                        .is_some_and(|state| state.0 == EntityLifecycle::Alive);
                    let is_sheep = entity
                        .get::<EntityTypeState>()
                        .is_some_and(|entity_type| &*entity_type.name == "minecraft:sheep");
                    entity.insert(AnimalState(animal));
                    (
                        is_alive && animal.needs_breeding_tick(),
                        is_alive && is_sheep && animal.sheep_wool.is_some(),
                    )
                };
                {
                    let mut active = world.resource_mut::<BreedingTickEntities>();
                    if needs_breeding_tick {
                        active.0.insert(id);
                    } else {
                        active.0.remove(&id);
                    }
                }
                let mut sheep = world.resource_mut::<SheepEntities>();
                if is_sheep {
                    sheep.0.insert(id);
                } else {
                    sheep.0.remove(&id);
                }
            }
        }
    }
}

fn apply_goal_tick(
    world: &mut World,
    request: GoalTickRequest,
    mut owner_output: Option<&mut OwnerGoalTickOutput>,
) -> (GoalTickStats, Vec<GoalSimulationCandidate>) {
    #[cfg(feature = "load-bench")]
    let commit_split_tick = request.tick;
    #[cfg(feature = "load-bench")]
    let commit_split_started = std::time::Instant::now();
    let active_filter = request
        .active_ids
        .as_ref()
        .filter(|active_ids| !active_set_covers_world(world, active_ids));
    let active_entities = request
        .simulation_capture_ids
        .is_none()
        .then(|| {
            active_filter
                .filter(|active_ids| active_set_is_sparse(world, active_ids))
                .map(|active_ids| indexed_entities_for_ids(world, active_ids))
        })
        .flatten();
    let captures_world = request.simulation_capture_ids.as_ref().is_some_and(|ids| {
        let index = world.resource::<RuntimeEntityIndex>();
        ids.len() == index.0.len() && ids.iter().eq(index.0.keys())
    });
    #[cfg(feature = "load-bench")]
    let commit_pre_us =
        u64::try_from(commit_split_started.elapsed().as_micros()).unwrap_or(u64::MAX);
    #[cfg(feature = "load-bench")]
    let commit_follow_started = std::time::Instant::now();
    let mut positions = BTreeMap::new();
    if !request.external_follow_targets_complete {
        let mut target_ids = Vec::new();
        if let Some(active_entities) = active_entities.as_ref() {
            for &entity in active_entities {
                let Ok(entity) = world.get_entity(entity) else {
                    continue;
                };
                if let Some(AiGoalState(GoalState::FollowTarget { target, .. })) =
                    entity.get::<AiGoalState>()
                {
                    target_ids.push(*target);
                }
            }
        } else {
            let mut identity_query = world.query::<(&StableIdentity, &AiGoalState)>();
            target_ids.extend(identity_query.iter(world).filter_map(|(identity, goal)| {
                if active_filter.is_some_and(|active_ids| !active_ids.contains(&identity.id)) {
                    return None;
                }
                match goal.0 {
                    GoalState::FollowTarget { target, .. } => Some(target),
                    _ => None,
                }
            }));
        }
        target_ids.sort_unstable();
        target_ids.dedup();
        positions = indexed_positions(world, &target_ids);
    }
    positions.extend(
        request
            .external_follow_targets
            .iter()
            .map(|(&id, &position)| (id, position)),
    );
    #[cfg(feature = "load-bench")]
    let commit_follow_us =
        u64::try_from(commit_follow_started.elapsed().as_micros()).unwrap_or(u64::MAX);
    #[cfg(feature = "load-bench")]
    let commit_loop_started = std::time::Instant::now();
    let mut stats = GoalTickStats {
        alive_entities: request.passive_decisions,
        decisions_applied: request.passive_decisions,
        ..GoalTickStats::default()
    };
    let mut simulation_results = if owner_output.is_some() {
        Vec::new()
    } else {
        Vec::with_capacity(request.simulation_capture_ids.as_ref().map_or(0, Vec::len))
    };
    let mut query = world.query_filtered::<(
        &StableIdentity,
        &EntityTypeState,
        &mut TransformState,
        &mut MotionState,
        &LifecycleState,
        Option<&LivingState>,
        &AiGoalState,
        &mut AiPathState,
        &GameplayDecisionState,
        Option<&AnimalState>,
    ), (
        Without<ItemStackState>,
        Without<ExperienceState>,
        Without<FallingBlockState>,
        Without<ProjectileState>,
        Without<VehicleKindState>,
    )>();
    if let Some(active_entities) = active_entities {
        for entity in active_entities {
            let Ok((
                identity,
                entity_type,
                mut transform,
                mut motion,
                lifecycle,
                _,
                goal,
                mut path,
                _,
                _,
            )) = query.get_mut(world, entity)
            else {
                continue;
            };
            apply_goal_to_entity(
                &request,
                &positions,
                identity,
                entity_type,
                &mut transform,
                &mut motion,
                lifecycle,
                goal,
                &mut path,
                &mut stats,
            );
        }
    } else {
        for (
            identity,
            entity_type,
            mut transform,
            mut motion,
            lifecycle,
            living,
            goal,
            mut path,
            gameplay,
            animal,
        ) in query.iter_mut(world)
        {
            let captures_entity = captures_world
                || request
                    .simulation_capture_ids
                    .as_ref()
                    .is_some_and(|ids| ids.binary_search(&identity.id).is_ok());
            let previous_motion =
                (captures_entity && owner_output.is_some()).then_some(EntityTrackingMotion {
                    id: identity.id,
                    position: transform.position,
                    rotation: transform.rotation,
                    velocity: motion.velocity,
                    on_ground: motion.on_ground,
                    is_item: false,
                    is_experience: false,
                    is_arrow: false,
                    sends_velocity: true,
                });
            let applies_goal =
                active_filter.is_none_or(|active_ids| active_ids.contains(&identity.id));
            if applies_goal {
                apply_goal_to_entity(
                    &request,
                    &positions,
                    identity,
                    entity_type,
                    &mut transform,
                    &mut motion,
                    lifecycle,
                    goal,
                    &mut path,
                    &mut stats,
                );
            }
            if captures_entity {
                let candidate = goal_simulation_candidate(
                    identity,
                    entity_type,
                    &transform,
                    &motion,
                    lifecycle,
                    living,
                    goal,
                    gameplay,
                    animal,
                );
                if let Some(output) = owner_output.as_deref_mut() {
                    capture_owner_goal_simulation_result(
                        output,
                        previous_motion.expect("owner goal capture records previous motion"),
                        candidate,
                    );
                } else if let Some(candidate) = candidate {
                    simulation_results.push(candidate);
                }
            }
        }
    }
    #[cfg(feature = "load-bench")]
    let commit_loop_us =
        u64::try_from(commit_loop_started.elapsed().as_micros()).unwrap_or(u64::MAX);
    #[cfg(feature = "load-bench")]
    let commit_sort_started = std::time::Instant::now();
    simulation_results.sort_unstable_by_key(|candidate| candidate.id);
    #[cfg(feature = "load-bench")]
    if commit_split_tick.is_multiple_of(10) {
        eprintln!(
            "COMMIT_SPLIT tick={} pre_us={} follow_us={} loop_us={} sort_us={}",
            commit_split_tick,
            commit_pre_us,
            commit_follow_us,
            commit_loop_us,
            u64::try_from(commit_sort_started.elapsed().as_micros()).unwrap_or(u64::MAX),
        );
    }
    (stats, simulation_results)
}

#[allow(clippy::too_many_arguments)]
fn goal_simulation_candidate(
    identity: &StableIdentity,
    entity_type: &EntityTypeState,
    transform: &TransformState,
    motion: &MotionState,
    lifecycle: &LifecycleState,
    living: Option<&LivingState>,
    goal: &AiGoalState,
    gameplay: &GameplayDecisionState,
    animal: Option<&AnimalState>,
) -> Option<GoalSimulationCandidate> {
    if living.is_none() || lifecycle.0 != EntityLifecycle::Alive {
        return None;
    }
    let state =
        motion_state_from_components(identity, entity_type, transform, motion, goal, gameplay);
    let result = entity_simulation_result_from_motion(
        entity_type,
        gameplay,
        animal.map(|state| state.0),
        true,
        state,
    );
    Some(GoalSimulationCandidate {
        id: identity.id,
        uuid: identity.uuid,
        lifecycle: lifecycle.0,
        pickup_claimed: gameplay.item_pickup_claim.is_some(),
        vehicle_attached: false,
        result: Some(result),
    })
}

fn capture_owner_goal_simulation_result(
    output: &mut OwnerGoalTickOutput,
    previous: EntityTrackingMotion,
    candidate: Option<GoalSimulationCandidate>,
) {
    output.captured_count = output.captured_count.saturating_add(1);
    let Some(candidate) = candidate else {
        output.invalid_count = output.invalid_count.saturating_add(1);
        return;
    };
    let Some(result) = candidate.result else {
        output.invalid_count = output.invalid_count.saturating_add(1);
        return;
    };
    if candidate.id != previous.id
        || candidate.lifecycle != EntityLifecycle::Alive
        || candidate.pickup_claimed
        || candidate.vehicle_attached
        || result.physics.id != candidate.id
    {
        output.invalid_count = output.invalid_count.saturating_add(1);
        return;
    }
    let entity = candidate.id;
    let position = result.physics.position;
    if result.hostile {
        output.active_hostile_ids.push((entity, position));
    }
    if result.villager_population_active {
        output
            .villager_population_candidates
            .push((entity, position));
    }
    if result.villager {
        output.villager_ids.push((entity, position));
    }
    if result.villager_population_active || result.item {
        output.villager_proximity_seeds.push((entity, position));
    }
    if previous.position != result.physics.position
        || previous.rotation != result.rotation
        || previous.velocity != result.physics.velocity
        || previous.on_ground != result.physics.on_ground
    {
        output.goal_committed_motion.push(previous);
    }
}

fn indexed_entities_for_ids(world: &World, ids: &HashSet<EntityId>) -> Vec<EcsEntity> {
    let mut ordered_ids = ids.iter().copied().collect::<Vec<_>>();
    ordered_ids.sort_unstable();
    let index = world.resource::<RuntimeEntityIndex>();
    ordered_ids
        .into_iter()
        .filter_map(|id| index.0.get(&id).copied())
        .collect()
}

fn indexed_positions(world: &World, ids: &[EntityId]) -> BTreeMap<EntityId, Vec3> {
    let index = world.resource::<RuntimeEntityIndex>();
    let entities = ids
        .iter()
        .filter_map(|id| index.0.get(id).copied().map(|entity| (*id, entity)))
        .collect::<Vec<_>>();
    entities
        .into_iter()
        .filter_map(|(id, entity)| {
            world
                .get_entity(entity)
                .ok()?
                .get::<TransformState>()
                .map(|transform| (id, transform.position))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn apply_goal_to_entity(
    request: &GoalTickRequest,
    positions: &BTreeMap<EntityId, Vec3>,
    identity: &StableIdentity,
    entity_type: &EntityTypeState,
    transform: &mut TransformState,
    motion: &mut MotionState,
    lifecycle: &LifecycleState,
    goal: &AiGoalState,
    path: &mut AiPathState,
    stats: &mut GoalTickStats,
) {
    if lifecycle.0 != EntityLifecycle::Alive {
        stats.skipped_non_alive += 1;
        return;
    }
    let goal_uses_pathing = match &goal.0 {
        GoalState::Wander { .. } => true,
        GoalState::AquaticWander { .. } => {
            crate::aquatic_motion::Swimmer::for_type(&entity_type.name)
                != crate::aquatic_motion::Swimmer::Other
        }
        GoalState::FollowPosition { speed, .. } => *speed != 0.0,
        _ => false,
    };
    let pathing_result = if request.pathing_enabled && goal_uses_pathing {
        let Some(result) = request.pathing.get(&identity.id) else {
            return;
        };
        if !result.matches(
            transform.position,
            transform.rotation,
            motion.velocity,
            motion.on_ground,
            &goal.0,
            &path.0,
        ) {
            return;
        }
        Some(result)
    } else {
        None
    };
    if let Some(result) = pathing_result {
        path.0 = result.next_path;
    }
    stats.alive_entities += 1;
    match &goal.0 {
        GoalState::Idle => {
            motion.velocity.x = 0.0;
            motion.velocity.z = 0.0;
        }
        GoalState::Wander {
            speed,
            period_ticks,
        } => {
            let direction = if request.pathing_enabled {
                if let Some(result) = pathing_result {
                    match result.decision.kind {
                        PathingDecisionKind::Move => stats.pathing_moves += 1,
                        PathingDecisionKind::Blocked => stats.pathing_blocked += 1,
                        PathingDecisionKind::Unloaded => stats.pathing_unloaded += 1,
                    }
                    result.decision.velocity
                } else {
                    stats.pathing_blocked += 1;
                    Vec3::ZERO
                }
            } else {
                let period = u64::from((*period_ticks).max(1));
                let angle = crate::deterministic_angle(identity.id, request.tick / period);
                Vec3 {
                    x: angle.cos(),
                    y: 0.0,
                    z: angle.sin(),
                }
            };
            let speed = pathing_result
                .and_then(|result| result.request.as_ref())
                .map_or(*speed, |planned| planned.speed);
            motion.velocity.x = direction.x * speed;
            motion.velocity.z = direction.z * speed;
            face_horizontal_motion(&mut transform.rotation, motion.velocity);
        }
        GoalState::AquaticWander { .. } => {
            motion.velocity = crate::aquatic_motion::apply_goal(
                identity.id,
                &entity_type.name,
                request.tick,
                transform.position,
                &mut transform.rotation,
                motion.velocity,
                &mut path.0,
                &goal.0,
                pathing_result,
            );
        }
        GoalState::FollowTarget { target, speed } => {
            let direction = if let Some(target_position) = positions.get(target) {
                Vec3 {
                    x: target_position.x - transform.position.x,
                    y: 0.0,
                    z: target_position.z - transform.position.z,
                }
                .horizontal_normalized()
            } else {
                stats.missing_follow_targets += 1;
                Vec3::ZERO
            };
            motion.velocity.x = direction.x * speed;
            motion.velocity.z = direction.z * speed;
            face_horizontal_motion(&mut transform.rotation, motion.velocity);
        }
        GoalState::FollowPosition { target, speed } => {
            let vertical_velocity = motion.velocity.y;
            let direction = if let Some(result) = pathing_result {
                match result.decision.kind {
                    PathingDecisionKind::Move => stats.pathing_moves += 1,
                    PathingDecisionKind::Blocked => stats.pathing_blocked += 1,
                    PathingDecisionKind::Unloaded => stats.pathing_unloaded += 1,
                }
                result.decision.velocity
            } else {
                Vec3 {
                    x: target.x - transform.position.x,
                    y: 0.0,
                    z: target.z - transform.position.z,
                }
                .horizontal_normalized()
            };
            motion.velocity.x = direction.x * speed;
            motion.velocity.y = if direction.y != 0.0 {
                direction.y * speed
            } else {
                vertical_velocity
            };
            motion.velocity.z = direction.z * speed;
            let facing = if *speed == 0.0 {
                direction
            } else {
                motion.velocity
            };
            if *speed == 0.0 && facing.horizontal_len() > f64::EPSILON {
                let yaw = crate::yaw_from_velocity(facing);
                transform.rotation.yaw = yaw;
                transform.rotation.head_yaw = yaw;
            } else {
                face_horizontal_motion(&mut transform.rotation, facing);
            }
        }
    }
    stats.decisions_applied += 1;
}

fn face_horizontal_motion(rotation: &mut Rotation, velocity: Vec3) {
    if velocity.horizontal_len() <= f64::EPSILON {
        return;
    }
    let target = crate::yaw_from_velocity(velocity);
    rotation.yaw =
        crate::mob_control_26_1_2::rotate_towards(rotation.yaw, target, MOB_BODY_YAW_TURN_PER_TICK);
    rotation.head_yaw = crate::mob_control_26_1_2::rotate_towards(
        rotation.head_yaw,
        target,
        MOB_HEAD_YAW_TURN_PER_TICK,
    );
}

fn integrate_positions(world: &mut World) {
    let Some((delta_seconds, active_ids)) = world.resource_mut::<PendingPositionTick>().0.take()
    else {
        return;
    };
    let mut query = world.query::<(
        &StableIdentity,
        &mut TransformState,
        &MotionState,
        &LifecycleState,
    )>();
    for (identity, mut transform, motion, lifecycle) in query.iter_mut(world) {
        if lifecycle.0 != EntityLifecycle::Alive
            || active_ids
                .as_ref()
                .is_some_and(|active_ids| !active_ids.contains(&identity.id))
        {
            continue;
        }
        transform.position.x += motion.velocity.x * delta_seconds;
        transform.position.y += motion.velocity.y * delta_seconds;
        transform.position.z += motion.velocity.z * delta_seconds;
    }
}

fn capture_snapshot_request(world: &mut World) {
    if !world.resource::<SnapshotRequest>().0 {
        return;
    }
    let snapshots = normalized_snapshots_from_world(world);
    world.resource_mut::<SnapshotRequest>().0 = false;
    world.resource_mut::<SnapshotOutput>().0 = snapshots;
}

fn apply_physics_results(world: &mut World) {
    let results = std::mem::take(&mut world.resource_mut::<PendingPhysicsResults>().0);
    if results.len() >= 257 {
        let mut result_indices = HashMap::with_capacity(results.len());
        if results
            .iter()
            .enumerate()
            .all(|(index, result)| result_indices.insert(result.id, index).is_none())
        {
            apply_unique_physics_results(world, &results, &result_indices);
            return;
        }
    }
    for result in results {
        let Some(entity) = ecs_entity_for(world, result.id) else {
            continue;
        };

        {
            let Ok(mut entity) = world.get_entity_mut(entity) else {
                continue;
            };

            let old_y = {
                let Some(mut transform) = entity.get_mut::<TransformState>() else {
                    continue;
                };
                let old_y = transform.position.y;
                transform.position = result.position;
                transform.rotation = result.rotation;
                old_y
            };
            let Some(mut motion) = entity.get_mut::<MotionState>() else {
                continue;
            };
            motion.fall_distance = if result.on_ground {
                0.0
            } else {
                motion.fall_distance + (old_y - result.position.y).max(0.0)
            };
            motion.velocity = result.velocity;
            motion.on_ground = result.on_ground;
        }
    }
}

fn apply_unique_physics_results(
    world: &mut World,
    results: &[EntityPhysicsResult],
    result_indices: &HashMap<EntityId, usize>,
) {
    let mut query = world.query::<(&StableIdentity, &mut TransformState, &mut MotionState)>();
    for (identity, mut transform, mut motion) in query.iter_mut(world) {
        let Some(index) = result_indices.get(&identity.id) else {
            continue;
        };
        let result = &results[*index];
        let old_y = transform.position.y;
        transform.position = result.position;
        transform.rotation = result.rotation;
        motion.fall_distance = if result.on_ground {
            0.0
        } else {
            motion.fall_distance + (old_y - result.position.y).max(0.0)
        };
        motion.velocity = result.velocity;
        motion.on_ground = result.on_ground;
    }
}
fn apply_combat_commands(world: &mut World) {
    let commands = std::mem::take(&mut world.resource_mut::<PendingCombatCommands>().0);
    for command in commands {
        match command {
            EntityCombatCommand::Damage { id, request } => {
                let Some(entity) = ecs_entity_for(world, id) else {
                    continue;
                };
                let killed = {
                    let Ok(mut entity) = world.get_entity_mut(entity) else {
                        continue;
                    };
                    if entity
                        .get::<LifecycleState>()
                        .is_none_or(|state| state.0 != EntityLifecycle::Alive)
                    {
                        continue;
                    }
                    let killed = {
                        let Some(mut living) = entity.get_mut::<LivingState>() else {
                            continue;
                        };
                        living.state.health =
                            (living.state.health - request.amount.max(0.0)).max(0.0);
                        if living.state.health <= 0.0 {
                            living.state.lifecycle = LivingLifecycle::Dying;
                            living.state.death_time = 0;
                        }
                        living.state.health <= 0.0
                    };
                    let Some(mut gameplay) = entity.get_mut::<GameplayDecisionState>() else {
                        continue;
                    };
                    gameplay.last_damage_tick = Some(request.tick);
                    if gameplay.villager.is_some()
                        && let Some(event) = request.villager_gossip_event
                    {
                        gameplay
                            .villager_gossip
                            .get_or_insert_with(Default::default)
                            .record_event(event);
                    }
                    if killed {
                        gameplay.death_remove_tick = Some(request.death_remove_tick);
                        gameplay.sheep_grazing_ticks = None;
                    }
                    if killed && let Some(mut lifecycle) = entity.get_mut::<LifecycleState>() {
                        lifecycle.0 = EntityLifecycle::Despawning;
                    }
                    killed
                };
                if killed {
                    world.resource_mut::<BreedingTickEntities>().0.remove(&id);
                    world.resource_mut::<SheepEntities>().0.remove(&id);
                }
            }
            EntityCombatCommand::MarkDespawning { id } => {
                let Some(entity) = ecs_entity_for(world, id) else {
                    continue;
                };
                {
                    let Ok(mut entity) = world.get_entity_mut(entity) else {
                        continue;
                    };
                    let Some(mut lifecycle) = entity.get_mut::<LifecycleState>() else {
                        continue;
                    };
                    lifecycle.0 = EntityLifecycle::Despawning;
                }
                world.resource_mut::<BreedingTickEntities>().0.remove(&id);
                world.resource_mut::<SheepEntities>().0.remove(&id);
            }
            EntityCombatCommand::Remove { id } => {
                let _ = remove_from_world(world, id);
            }
        }
    }
}

fn extract_persistence_snapshots(world: &mut World) {
    if !world.resource::<PersistenceExtractRequest>().0 {
        return;
    }
    let snapshots = normalized_snapshots_from_world(world)
        .into_iter()
        .filter(|snapshot| {
            ecs_entity_for(world, snapshot.id)
                .and_then(|entity| world.get_entity(entity).ok())
                .is_some_and(|entity| entity.contains::<PersistentState>())
        })
        .collect();
    world.resource_mut::<PersistenceExtractRequest>().0 = false;
    world.resource_mut::<PersistenceOutput>().0 = snapshots;
}

fn ecs_entity_for(world: &World, id: EntityId) -> Option<EcsEntity> {
    world.resource::<RuntimeEntityIndex>().0.get(&id).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects_26_1_2::{EffectFlags, EffectKind};
    use crate::{
        AttributeKind, AttributeSet, EntityCrossbowAttackPhase, EntityCrossbowAttackState,
        EntityGuardianBeamPhase, EntityGuardianBeamState, EntityId, EntityItemStack,
        EntityLifecycle, EntitySnapshot, GoalState, Rotation, Vec3, VehicleKind, VehicleState,
    };
    use uuid::Uuid;

    fn snapshot(id: i32, type_id: i32, type_name: &str) -> EntitySnapshot {
        let mut attributes = AttributeSet::vanilla_mob_defaults();
        attributes.set_base(AttributeKind::AttackDamage, f64::from(id));
        EntitySnapshot {
            id: EntityId(id),
            uuid: Uuid::from_u128(id as u128),
            type_id,
            type_name: type_name.to_owned(),
            position: Vec3::new(f64::from(id), 64.0 + f64::from(id), -f64::from(id)),
            rotation: Rotation {
                yaw: id as f32,
                pitch: id as f32 / 2.0,
                head_yaw: id as f32 + 1.0,
            },
            velocity: Vec3::new(0.1 * f64::from(id), -0.02, 0.03),
            on_ground: id % 2 == 0,
            item_stack: None,
            experience_value: None,
            block_state: None,
            lifecycle: EntityLifecycle::Alive,
            health: 20.0 - id as f32,
            attributes,
            goal: GoalState::Wander {
                speed: 0.2,
                period_ticks: 40,
            },
            vehicle: None,
            animal: None,
            retained: EntityRetainedState::default(),
        }
    }

    #[test]
    fn all_supported_components_round_trip_exactly() {
        let mut item = snapshot(1, 1, "minecraft:item");
        item.item_stack = Some(EntityItemStack::new(42, 3).with_damage(2));

        let mut xp = snapshot(2, 2, "minecraft:experience_orb");
        xp.experience_value = Some(7);

        let mut hostile = snapshot(3, 3, "minecraft:pillager");
        hostile.goal = GoalState::FollowTarget {
            target: EntityId(7),
            speed: 0.23,
        };
        hostile.retained.crossbow_attack = Some(EntityCrossbowAttackState::new(
            EntityCrossbowAttackPhase::Charging,
            125,
        ));
        hostile.retained.guardian_beam = Some(EntityGuardianBeamState::new(
            EntityGuardianBeamPhase::Beam,
            17,
            99,
            205,
        ));

        let arrow = snapshot(4, 4, "minecraft:arrow");

        let mut falling_block = snapshot(5, 5, "minecraft:falling_block");
        falling_block.block_state = Some(91);

        let mut boat = snapshot(6, 6, "minecraft:oak_boat");
        boat.vehicle = Some(VehicleState {
            kind: VehicleKind::Boat,
            passenger: Some(EntityId(7)),
        });

        let passenger = snapshot(7, 7, "minecraft:cow");

        let mut minecart = snapshot(8, 8, "minecraft:minecart");
        minecart.lifecycle = EntityLifecycle::Despawning;
        minecart.health = 0.0;
        minecart.vehicle = Some(VehicleState::new(VehicleKind::Minecart));

        let mut villager = snapshot(9, 9, "minecraft:villager");
        villager.retained.villager = Some(crate::VillagerData::new(
            crate::VillagerKind::Plains,
            crate::VillagerProfession::None,
            1,
        ));
        let mut villager_brain = crate::villager_26_1_2::VillagerBrainState::adult(
            crate::villager_26_1_2::VillagerPoiSet::default(),
        );
        villager_brain.last_slept_tick = Some(100);
        villager_brain.golem_detected_until_tick = Some(699);
        villager.retained.villager_brain = Some(villager_brain);

        let expected = vec![
            item,
            xp,
            hostile,
            arrow,
            falling_block,
            boat,
            passenger,
            minecart,
            villager,
        ];
        let mut runtime = EntityRuntime::new();
        for entity in expected.iter().rev().cloned() {
            assert!(runtime.insert_snapshot(entity));
        }

        assert_eq!(runtime.normalized_snapshots(), expected);
        assert!(runtime.has_projectile(EntityId(4)));
        assert!(runtime.has_persistent_state(EntityId(1)));
        assert!(runtime.has_visibility_state(EntityId(7)));
        assert_eq!(runtime.remove(EntityId(6)), Some(expected[5].clone()));
        assert_eq!(runtime.snapshot(EntityId(6)), None);
    }

    #[test]
    fn full_population_visitors_follow_entity_id_order() {
        let mut runtime = EntityRuntime::new();
        for id in [3, 2, 1] {
            assert!(runtime.insert_snapshot(snapshot(id, id, "minecraft:cow")));
        }

        let mut goal_ids = Vec::new();
        runtime.visit_goal_tick_candidates(|motion, _, _, _, _| goal_ids.push(motion.id));
        let mut physics_ids = Vec::new();
        runtime.visit_simulation_fence_results(|state, _| physics_ids.push(state.motion.id));

        let expected = vec![EntityId(1), EntityId(2), EntityId(3)];
        assert_eq!(goal_ids, expected);
        assert_eq!(physics_ids, expected);
    }

    #[test]
    fn profession_projection_does_not_invent_missing_explicit_job_site() {
        let mut explicit_brain = snapshot(10, 10, "minecraft:villager");
        explicit_brain.retained.villager = Some(crate::VillagerData::new(
            crate::VillagerKind::Plains,
            crate::VillagerProfession::Toolsmith,
            1,
        ));
        explicit_brain.retained.villager_brain =
            Some(crate::villager_26_1_2::VillagerBrainState::adult(
                crate::villager_26_1_2::VillagerPoiSet::default(),
            ));
        let mut implicit_brain = explicit_brain.clone();
        implicit_brain.id = EntityId(11);
        implicit_brain.uuid = Uuid::from_u128(11);
        implicit_brain.retained.villager_brain = None;

        let mut runtime = EntityRuntime::new();
        assert!(runtime.insert_snapshot(explicit_brain));
        assert!(runtime.insert_snapshot(implicit_brain.clone()));

        assert_eq!(
            runtime
                .simulation_projection(EntityId(10))
                .expect("explicit-brain villager projection")
                .villager_job_site,
            None
        );
        assert_eq!(
            runtime
                .simulation_projection(EntityId(11))
                .expect("implicit-brain villager projection")
                .villager_job_site,
            Some(implicit_brain.position)
        );
    }

    #[test]
    fn duplicate_runtime_id_is_rejected_without_replacing_state() {
        let original = snapshot(9, 9, "minecraft:cow");
        let mut replacement = original.clone();
        replacement.position = Vec3::new(99.0, 99.0, 99.0);
        let mut runtime = EntityRuntime::new();

        assert!(runtime.insert_snapshot(original.clone()));
        assert!(!runtime.insert_snapshot(replacement));
        assert_eq!(runtime.snapshot(EntityId(9)), Some(original));
    }

    #[test]
    fn duplicate_uuid_is_rejected_and_remove_releases_uuid() {
        let original = snapshot(9, 9, "minecraft:cow");
        let mut duplicate = snapshot(10, 10, "minecraft:sheep");
        duplicate.uuid = original.uuid;
        let mut runtime = EntityRuntime::new();

        assert!(runtime.insert_snapshot(original.clone()));
        assert!(!runtime.insert_snapshot(duplicate.clone()));
        assert!(runtime.contains_uuid(original.uuid));
        assert_eq!(runtime.remove(original.id), Some(original));
        assert!(!runtime.contains_uuid(duplicate.uuid));
        assert!(runtime.insert_snapshot(duplicate));
    }

    #[test]
    fn physics_tracks_fall_distance_and_resets_it_on_ground() {
        let initial = snapshot(10, 10, "minecraft:zombie");
        let mut runtime = EntityRuntime::new();
        assert!(runtime.insert_snapshot(initial.clone()));

        runtime.queue_physics(EntityPhysicsResult {
            id: initial.id,
            position: Vec3::new(
                initial.position.x,
                initial.position.y - 3.0,
                initial.position.z,
            ),
            rotation: initial.rotation,
            velocity: Vec3::new(0.0, -0.5, 0.0),
            on_ground: false,
        });
        runtime.run_stage(EntityStage::PhysicsApply);
        assert_eq!(
            runtime.snapshot(initial.id).unwrap().retained.fall_distance,
            3.0
        );

        runtime.queue_physics(EntityPhysicsResult {
            id: initial.id,
            position: Vec3::new(
                initial.position.x,
                initial.position.y - 3.5,
                initial.position.z,
            ),
            rotation: initial.rotation,
            velocity: Vec3::ZERO,
            on_ground: true,
        });
        runtime.run_stage(EntityStage::PhysicsApply);
        assert_eq!(
            runtime.snapshot(initial.id).unwrap().retained.fall_distance,
            0.0
        );
    }

    #[test]
    fn schedules_apply_only_their_owned_operations() {
        let initial = snapshot(10, 10, "minecraft:zombie");
        let moved_position = Vec3::new(20.0, 65.0, 30.0);
        let moved_velocity = Vec3::new(0.5, -0.1, 0.25);
        let mut runtime = EntityRuntime::new();
        runtime.queue_input(EntityInputCommand::Insert(Box::new(initial.clone())));
        runtime.request_snapshots();

        runtime.run_stage(EntityStage::SnapshotRequest);
        assert_eq!(runtime.take_snapshot_output(), Vec::new());
        assert_eq!(runtime.snapshot(initial.id), None);

        runtime.run_stage(EntityStage::InputAi);
        assert_eq!(runtime.snapshot(initial.id), Some(initial.clone()));

        runtime.queue_physics(EntityPhysicsResult {
            id: initial.id,
            position: moved_position,
            rotation: initial.rotation,
            velocity: moved_velocity,
            on_ground: false,
        });
        runtime.queue_combat(EntityCombatCommand::Damage {
            id: initial.id,
            request: EntityDamageRequest {
                amount: 5.0,
                tick: 4,
                death_remove_tick: 24,
                villager_gossip_event: None,
            },
        });
        runtime.run_stage(EntityStage::CombatLifecycle);
        let damaged = runtime.snapshot(initial.id).unwrap();
        assert_eq!(damaged.health, initial.health - 5.0);
        assert_eq!(damaged.position, initial.position);

        runtime.run_stage(EntityStage::PhysicsApply);
        let moved = runtime.snapshot(initial.id).unwrap();
        assert_eq!(moved.position, moved_position);
        assert_eq!(moved.velocity, moved_velocity);
        assert!(!moved.on_ground);

        runtime.request_snapshots();
        runtime.run_stage(EntityStage::SnapshotRequest);
        assert_eq!(runtime.take_snapshot_output(), vec![moved.clone()]);

        runtime.request_persistence_extract();
        runtime.run_stage(EntityStage::PersistenceExtract);
        assert_eq!(runtime.take_persistence_output(), vec![moved]);
    }

    #[test]
    fn lethal_damage_then_remove_has_one_ordered_lifecycle_result() {
        let initial = snapshot(11, 11, "minecraft:cow");
        let mut runtime = EntityRuntime::new();
        assert!(runtime.insert_snapshot(initial.clone()));

        runtime.queue_combat(EntityCombatCommand::Damage {
            id: initial.id,
            request: EntityDamageRequest {
                amount: initial.health,
                tick: 5,
                death_remove_tick: 25,
                villager_gossip_event: None,
            },
        });
        runtime.queue_combat(EntityCombatCommand::Remove { id: initial.id });
        runtime.run_stage(EntityStage::CombatLifecycle);

        assert_eq!(runtime.snapshot(initial.id), None);
    }

    #[test]
    fn effect_transaction_rejects_non_finite_authoritative_health_explicitly() {
        let initial = snapshot(12, 12, "minecraft:cow");
        let mut runtime = EntityRuntime::new();
        assert!(runtime.insert_snapshot(initial.clone()));
        let entity = runtime.world.resource::<RuntimeEntityIndex>().0[&initial.id];
        runtime
            .world
            .get_mut::<LivingState>(entity)
            .unwrap()
            .state
            .health = f32::NAN;

        assert_eq!(
            runtime.apply_effect(
                initial.id,
                EntityEffectRequest {
                    operation: EntityEffectOperation::ApplyAction {
                        effect_id: EffectId::new(6),
                        action: EffectAction::Heal { amount: 1.0 },
                        damage_context: None,
                    },
                    target_kind: TargetKind::NonPlayer,
                    death_remove_tick: 20,
                },
            ),
            EntityEffectResult::Rejected(EntityEffectRejection::NonFiniteCurrentHealth)
        );
        assert!(runtime.snapshot(initial.id).unwrap().health.is_nan());
    }

    #[test]
    fn snapshot_restore_and_reinsert_preserve_living_clocks_and_hidden_effects_exactly() {
        let initial = snapshot(13, 13, "minecraft:cow");
        let mut runtime = EntityRuntime::new();
        assert!(runtime.insert_snapshot(initial.clone()));
        let entity = runtime.world.resource::<RuntimeEntityIndex>().0[&initial.id];
        {
            let mut living = runtime.world.get_mut::<LivingState>(entity).unwrap();
            living.state.absorption = 3.5;
            living.state.invulnerable_time = 17;
            living.state.hurt_time = 8;
            living.state.last_hurt = 4.25;
        }
        let effect_id = EffectId::new(10);
        for effect in [
            EffectInstance::new(
                effect_id,
                EffectKind::Regeneration,
                200,
                0,
                EffectFlags::default(),
            ),
            EffectInstance::new(
                effect_id,
                EffectKind::Regeneration,
                50,
                1,
                EffectFlags::default(),
            ),
        ] {
            assert!(matches!(
                runtime.apply_effect(
                    initial.id,
                    EntityEffectRequest {
                        operation: EntityEffectOperation::Add(effect),
                        target_kind: TargetKind::NonPlayer,
                        death_remove_tick: 30,
                    },
                ),
                EntityEffectResult::Applied(_)
            ));
        }
        let retained = runtime.snapshot(initial.id).unwrap();
        assert_eq!(
            retained
                .retained
                .active_effects
                .as_ref()
                .unwrap()
                .effects
                .chains[0]
                .hidden
                .len(),
            1
        );

        runtime
            .world
            .get_mut::<LivingState>(entity)
            .unwrap()
            .state
            .hurt_time = 1;
        assert!(runtime.restore_snapshot_in_place(retained.clone()));
        assert_eq!(runtime.snapshot(initial.id), Some(retained.clone()));

        assert_eq!(runtime.remove(initial.id), Some(retained.clone()));
        let mut restarted = EntityRuntime::new();
        assert!(restarted.insert_snapshot(retained.clone()));
        assert_eq!(restarted.snapshot(initial.id), Some(retained));
    }
}
