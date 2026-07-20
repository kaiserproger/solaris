use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::ops::Range;

use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity as EcsEntity;
use bevy_ecs::query::Without;
use bevy_ecs::resource::Resource;
use bevy_ecs::schedule::{ExecutorKind, IntoScheduleConfigs, Schedule};
use bevy_ecs::world::World;
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
    AnimalBreedingState, AttributeSet, EntityActiveEffectsState, EntityDamageRequest, EntityId,
    EntityItemStack, EntityKinematics, EntityLifecycle, EntityLivingRetainedState,
    EntityMotionState, EntityRetainedState, EntitySnapshot, EntityView, GoalPathingRequest,
    GoalPathingResult, GoalState, GoalTickStats, PathingDecisionKind, RetainedPathState, Rotation,
    Vec3, VehicleKind, VehicleState,
};

#[derive(Component)]
struct StableIdentity {
    id: EntityId,
    uuid: Uuid,
}

#[derive(Component)]
struct EntityTypeState {
    protocol_id: i32,
    name: String,
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

#[derive(Component, Clone, Copy)]
struct GameplayDecisionState {
    arrow_state: Option<crate::projectile_26_1_2::ArrowState>,
    last_damage_tick: Option<u64>,
    death_remove_tick: Option<u64>,
    sheep_grazing_ticks: Option<u8>,
    spawn_tick: u64,
    item_pickup_ready_tick: Option<u64>,
    item_pickup_owner_block: Option<crate::EntityItemPickupOwnerBlock>,
    primed_tnt: Option<crate::EntityPrimedTntState>,
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

#[derive(Resource, Default)]
struct PendingGoalTick(Option<GoalTickRequest>);

struct GoalTickRequest {
    tick: u64,
    pathing_enabled: bool,
    pathing: BTreeMap<EntityId, GoalPathingResult>,
    active_ids: Option<HashSet<EntityId>>,
    external_follow_targets: HashMap<EntityId, Vec3>,
}

#[derive(Resource, Default)]
struct GoalTickOutput(Option<GoalTickStats>);

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
        world.init_resource::<PendingGoalTick>();
        world.init_resource::<GoalTickOutput>();
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
        }
    }

    pub fn insert_snapshot(&mut self, snapshot: EntitySnapshot) -> bool {
        insert_snapshot_into_world(&mut self.world, snapshot)
    }

    pub(crate) fn restore_snapshot_in_place(&mut self, snapshot: EntitySnapshot) -> bool {
        restore_snapshot_in_world(&mut self.world, snapshot)
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
            gameplay: *row.get::<GameplayDecisionState>()?,
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

    pub(crate) fn contains_uuid(&self, uuid: Uuid) -> bool {
        self.world
            .resource::<RuntimeEntityUuids>()
            .0
            .contains(&uuid)
    }

    pub(crate) fn motion_state(&self, id: EntityId) -> Option<EntityMotionState> {
        let entity = *self.world.resource::<RuntimeEntityIndex>().0.get(&id)?;
        let entity = self.world.get_entity(entity).ok()?;
        let identity = entity.get::<StableIdentity>()?;
        let entity_type = entity.get::<EntityTypeState>()?;
        let transform = entity.get::<TransformState>()?;
        let motion = entity.get::<MotionState>()?;
        let arrow_state = entity
            .get::<GameplayDecisionState>()
            .and_then(|gameplay| gameplay.arrow_state);
        Some(EntityMotionState {
            id: identity.id,
            position: transform.position,
            rotation: transform.rotation,
            velocity: motion.velocity,
            on_ground: motion.on_ground,
            is_item: entity_type.name == "minecraft:item",
            is_experience: entity_type.name == "minecraft:experience_orb",
            is_arrow: entity_type.name == "minecraft:arrow",
            arrow_revision: arrow_state.map(|state| state.projectile.revision),
            arrow_embedded_block: arrow_state
                .filter(|state| state.in_ground)
                .and_then(|state| state.last_block_position),
            sends_velocity: !matches!(
                entity_type.name.as_str(),
                "minecraft:item" | "minecraft:experience_orb"
            ),
        })
    }

    pub(crate) fn view(&self, id: EntityId) -> Option<EntityView<'_>> {
        entity_view_from_world(&self.world, id)
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

    pub(crate) fn pathing_requests(
        &mut self,
        tick: u64,
        active_ids: Option<&HashSet<EntityId>>,
    ) -> Vec<GoalPathingRequest> {
        if let Some(active_ids) = active_ids
            && active_set_is_sparse(&self.world, active_ids)
        {
            let mut ids = active_ids.iter().copied().collect::<Vec<_>>();
            ids.sort_unstable();
            return ids
                .into_iter()
                .filter_map(|id| self.pathing_request(id, tick))
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
        ), (
            Without<ItemStackState>,
            Without<ExperienceState>,
            Without<FallingBlockState>,
            Without<ProjectileState>,
            Without<VehicleKindState>,
        )>();
        let mut requests = query
            .iter(&self.world)
            .filter(|(_, _, _, lifecycle, _, _)| lifecycle.0 == EntityLifecycle::Alive)
            .filter(|(identity, _, _, _, _, _)| {
                active_filter.is_none_or(|active_ids| active_ids.contains(&identity.id))
            })
            .filter_map(|(identity, transform, motion, _, goal, path)| {
                goal_pathing_request(identity, transform, motion, goal, path, tick)
            })
            .collect::<Vec<_>>();
        requests.sort_unstable_by_key(|request| request.id);
        requests
    }

    fn pathing_request(&self, id: EntityId, tick: u64) -> Option<GoalPathingRequest> {
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
        goal_pathing_request(identity, transform, motion, goal, path, tick)
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

    pub(crate) fn queue_goal_tick(
        &mut self,
        tick: u64,
        pathing_enabled: bool,
        pathing: impl IntoIterator<Item = GoalPathingResult>,
        active_ids: Option<&HashSet<EntityId>>,
        external_follow_targets: Option<&HashMap<EntityId, Vec3>>,
    ) {
        let request = GoalTickRequest {
            tick,
            pathing_enabled,
            pathing: pathing
                .into_iter()
                .map(|result| (result.request.id, result))
                .collect(),
            active_ids: active_ids.cloned(),
            external_follow_targets: external_follow_targets.cloned().unwrap_or_default(),
        };
        let previous = self
            .world
            .resource_mut::<PendingGoalTick>()
            .0
            .replace(request);
        assert!(previous.is_none(), "goal tick already queued");
    }

    pub(crate) fn take_goal_tick_stats(&mut self) -> GoalTickStats {
        self.world
            .resource_mut::<GoalTickOutput>()
            .0
            .take()
            .expect("queued goal tick must publish stats")
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
            EntityStage::PhysicsApply => self.schedules.physics_apply.run(&mut self.world),
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
    active_ids.len() == index.0.len() && active_ids.iter().all(|id| index.0.contains_key(id))
}

fn goal_pathing_request(
    identity: &StableIdentity,
    transform: &TransformState,
    motion: &MotionState,
    goal: &AiGoalState,
    path: &AiPathState,
    tick: u64,
) -> Option<GoalPathingRequest> {
    let (target, target_epoch, speed) = match &goal.0 {
        GoalState::FollowPosition { target, speed } => (*target, None, *speed),
        GoalState::Wander {
            speed,
            period_ticks,
        } => {
            let (target, epoch) = crate::wander_pathing_target(
                identity.id,
                transform.position,
                path.0,
                tick,
                *period_ticks,
            );
            (target, Some(epoch), *speed)
        }
        _ => return None,
    };
    Some(GoalPathingRequest {
        id: identity.id,
        expected_position: transform.position,
        expected_rotation: transform.rotation,
        expected_velocity: motion.velocity,
        expected_on_ground: motion.on_ground,
        expected_goal: goal.0.clone(),
        expected_path: path.0,
        target,
        target_epoch,
        speed,
    })
}

impl EntitySchedules {
    fn new() -> Self {
        let mut input_ai = Schedule::default();
        input_ai.set_executor_kind(ExecutorKind::SingleThreaded);
        input_ai.add_systems((apply_input_commands, apply_goal_tick).chain());

        let mut snapshot_request = Schedule::default();
        snapshot_request.set_executor_kind(ExecutorKind::SingleThreaded);
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
        active_effects,
        arrow_state,
        last_damage_tick,
        death_remove_tick,
        sheep_grazing_ticks,
        spawn_tick,
        item_pickup_ready_tick,
        item_pickup_owner_block,
        primed_tnt,
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
    let is_projectile = type_name == "minecraft:arrow";
    let is_sheep = lifecycle == EntityLifecycle::Alive
        && type_name == "minecraft:sheep"
        && animal.is_some_and(|animal| animal.sheep_wool.is_some());
    let mut entity = world.spawn((
        StableIdentity { id, uuid },
        EntityTypeState {
            protocol_id: type_id,
            name: type_name,
        },
        TransformState { position, rotation },
        MotionState {
            velocity,
            on_ground,
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
            last_damage_tick,
            death_remove_tick,
            sheep_grazing_ticks,
            spawn_tick,
            item_pickup_ready_tick,
            item_pickup_owner_block,
            primed_tnt,
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

fn restore_snapshot_in_world(world: &mut World, snapshot: EntitySnapshot) -> bool {
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
        entity_type.protocol_id == snapshot.type_id && entity_type.name == snapshot.type_name
    });
    if !identity_matches || !type_matches {
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
    let EntitySnapshot {
        id,
        uuid: _,
        type_id: _,
        type_name: _,
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
        active_effects,
        arrow_state,
        last_damage_tick,
        death_remove_tick,
        sheep_grazing_ticks,
        spawn_tick,
        item_pickup_ready_tick,
        item_pickup_owner_block,
        primed_tnt,
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
            TransformState { position, rotation },
            MotionState {
                velocity,
                on_ground,
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
                last_damage_tick,
                death_remove_tick,
                sheep_grazing_ticks,
                spawn_tick,
                item_pickup_ready_tick,
                item_pickup_owner_block,
                primed_tnt,
            },
        ));
        replace_optional_component(&mut entity, active_effects);
        replace_optional_component(&mut entity, item_stack.map(ItemStackState));
        replace_optional_component(&mut entity, experience_value.map(ExperienceState));
        replace_optional_component(&mut entity, block_state.map(FallingBlockState));
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
        type_name: entity_type.name.clone(),
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
            active_effects: active_effects_snapshot(entity.get::<ActiveEffectsState>()),
            arrow_state: gameplay.arrow_state,
            last_damage_tick: gameplay.last_damage_tick,
            death_remove_tick: gameplay.death_remove_tick,
            sheep_grazing_ticks: gameplay.sheep_grazing_ticks,
            spawn_tick: gameplay.spawn_tick,
            item_pickup_ready_tick: gameplay.item_pickup_ready_tick,
            item_pickup_owner_block: gameplay.item_pickup_owner_block,
            primed_tnt: gameplay.primed_tnt,
        },
    })
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
            active_effects: active_effects_snapshot(entity.get::<ActiveEffectsState>()),
            arrow_state: gameplay.arrow_state,
            last_damage_tick: gameplay.last_damage_tick,
            death_remove_tick: gameplay.death_remove_tick,
            sheep_grazing_ticks: gameplay.sheep_grazing_ticks,
            spawn_tick: gameplay.spawn_tick,
            item_pickup_ready_tick: gameplay.item_pickup_ready_tick,
            item_pickup_owner_block: gameplay.item_pickup_owner_block,
            primed_tnt: gameplay.primed_tnt,
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
                        .is_some_and(|entity_type| entity_type.name == "minecraft:sheep");
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

fn apply_goal_tick(world: &mut World) {
    let Some(request) = world.resource_mut::<PendingGoalTick>().0.take() else {
        return;
    };
    let active_filter = request
        .active_ids
        .as_ref()
        .filter(|active_ids| !active_set_covers_world(world, active_ids));
    let active_entities = active_filter
        .filter(|active_ids| active_set_is_sparse(world, active_ids))
        .map(|active_ids| indexed_entities_for_ids(world, active_ids));
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
    let mut positions = indexed_positions(world, &target_ids);
    for (&id, &position) in &request.external_follow_targets {
        positions.entry(id).or_insert(position);
    }
    let mut stats = GoalTickStats::default();
    let mut query = world.query_filtered::<(
        &StableIdentity,
        &mut TransformState,
        &mut MotionState,
        &LifecycleState,
        &AiGoalState,
        &mut AiPathState,
    ), (
        Without<ItemStackState>,
        Without<ExperienceState>,
        Without<FallingBlockState>,
        Without<ProjectileState>,
        Without<VehicleKindState>,
    )>();
    if let Some(active_entities) = active_entities {
        for entity in active_entities {
            let Ok((identity, mut transform, mut motion, lifecycle, goal, mut path)) =
                query.get_mut(world, entity)
            else {
                continue;
            };
            apply_goal_to_entity(
                &request,
                &positions,
                identity,
                &mut transform,
                &mut motion,
                lifecycle,
                goal,
                &mut path,
                &mut stats,
            );
        }
    } else {
        for (identity, mut transform, mut motion, lifecycle, goal, mut path) in
            query.iter_mut(world)
        {
            if active_filter.is_some_and(|active_ids| !active_ids.contains(&identity.id)) {
                continue;
            }
            apply_goal_to_entity(
                &request,
                &positions,
                identity,
                &mut transform,
                &mut motion,
                lifecycle,
                goal,
                &mut path,
                &mut stats,
            );
        }
    }
    world.resource_mut::<GoalTickOutput>().0 = Some(stats);
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
    let pathing_result = if request.pathing_enabled
        && matches!(
            &goal.0,
            GoalState::Wander { .. } | GoalState::FollowPosition { .. }
        ) {
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
            motion.velocity.x = direction.x * speed;
            motion.velocity.z = direction.z * speed;
            if motion.velocity.horizontal_len() > 0.0 {
                transform.rotation.yaw = crate::yaw_from_velocity(motion.velocity);
                transform.rotation.head_yaw = transform.rotation.yaw;
            }
        }
        GoalState::AquaticWander {
            speed,
            vertical_speed,
            period_ticks,
        } => {
            let period = u64::from((*period_ticks).max(1));
            let phase = request.tick / period;
            let angle = crate::deterministic_angle(identity.id, phase);
            let vertical_wave = crate::deterministic_wave(identity.id, phase);
            motion.velocity.x = angle.cos() * speed;
            motion.velocity.z = angle.sin() * speed;
            motion.velocity.y = vertical_wave * vertical_speed;
            motion.on_ground = false;
            transform.rotation = crate::aquatic_rotation_from_velocity(motion.velocity);
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
            if motion.velocity.horizontal_len() > 0.0 {
                transform.rotation.yaw = crate::yaw_from_velocity(motion.velocity);
                transform.rotation.head_yaw = transform.rotation.yaw;
            }
        }
        GoalState::FollowPosition { target, speed } => {
            let vertical_velocity = motion.velocity.y;
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
            if motion.velocity.horizontal_len() > 0.0 {
                transform.rotation.yaw = crate::yaw_from_velocity(motion.velocity);
                transform.rotation.head_yaw = transform.rotation.yaw;
            }
        }
    }
    stats.decisions_applied += 1;
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
    for result in results {
        let Some(entity) = ecs_entity_for(world, result.id) else {
            continue;
        };

        {
            let Ok(mut entity) = world.get_entity_mut(entity) else {
                continue;
            };

            {
                let Some(mut transform) = entity.get_mut::<TransformState>() else {
                    continue;
                };
                transform.position = result.position;
                transform.rotation = result.rotation;
            }
            let Some(mut motion) = entity.get_mut::<MotionState>() else {
                continue;
            };
            motion.velocity = result.velocity;
            motion.on_ground = result.on_ground;
        }
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
        AttributeKind, AttributeSet, EntityId, EntityItemStack, EntityLifecycle, EntitySnapshot,
        GoalState, Rotation, Vec3, VehicleKind, VehicleState,
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

        let mut hostile = snapshot(3, 3, "minecraft:zombie");
        hostile.goal = GoalState::FollowTarget {
            target: EntityId(7),
            speed: 0.23,
        };

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

        let expected = vec![
            item,
            xp,
            hostile,
            arrow,
            falling_block,
            boat,
            passenger,
            minecart,
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
