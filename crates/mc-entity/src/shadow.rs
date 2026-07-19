use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::ops::Range;

use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity as EcsEntity;
use bevy_ecs::query::{With, Without};
use bevy_ecs::resource::Resource;
use bevy_ecs::schedule::{ExecutorKind, IntoScheduleConfigs, Schedule};
use bevy_ecs::world::World;
use uuid::Uuid;

use crate::{
    AnimalBreedingState, AttributeSet, EntityId, EntityItemStack, EntityKinematics,
    EntityLifecycle, EntityMotionState, EntitySnapshot, EntityView, GoalPathingRequest,
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
    health: f32,
    attributes: AttributeSet,
}

#[derive(Component)]
struct AiGoalState(GoalState);

#[derive(Component, Default)]
struct AiPathState(RetainedPathState);

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

#[derive(Component)]
struct AuthoritativeState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowStage {
    InputAi,
    SnapshotRequest,
    PhysicsApply,
    CombatLifecycle,
    PersistenceExtract,
    #[cfg(any(test, feature = "shadow-compare"))]
    OutputEvents,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ShadowInputCommand {
    #[cfg(any(test, feature = "shadow-compare"))]
    Insert(EntitySnapshot),
    InsertAuthoritative(EntitySnapshot),
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
pub struct ShadowPhysicsResult {
    pub id: EntityId,
    pub position: Vec3,
    pub rotation: Rotation,
    pub velocity: Vec3,
    pub on_ground: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShadowCombatCommand {
    Damage { id: EntityId, amount: f32 },
    MarkDespawning { id: EntityId },
    Remove { id: EntityId },
}

#[cfg(any(test, feature = "shadow-compare"))]
#[derive(Debug, Clone, PartialEq)]
pub enum ShadowSemanticEvent {
    Spawned {
        id: EntityId,
    },
    GoalChanged {
        id: EntityId,
    },
    ItemStackChanged {
        id: EntityId,
    },
    VehicleChanged {
        id: EntityId,
    },
    PhysicsApplied {
        id: EntityId,
    },
    Damaged {
        id: EntityId,
        health: f32,
        killed: bool,
    },
    LifecycleChanged {
        id: EntityId,
        lifecycle: EntityLifecycle,
    },
    Removed {
        id: EntityId,
    },
}

#[cfg(any(test, feature = "shadow-compare"))]
#[derive(Debug, Clone, PartialEq)]
pub struct ShadowComparison {
    pub tick: u64,
    pub stage: ShadowStage,
    pub compared_entities: usize,
    pub compared_events: usize,
}

#[cfg(any(test, feature = "shadow-compare"))]
#[derive(Debug, Clone, PartialEq)]
pub struct ShadowDivergence {
    pub tick: u64,
    pub stage: ShadowStage,
    pub compared_entities: usize,
    pub compared_events: usize,
    pub entity_id: Option<EntityId>,
    pub legacy: Option<EntitySnapshot>,
    pub shadow: Option<EntitySnapshot>,
    pub legacy_event: Option<ShadowSemanticEvent>,
    pub shadow_event: Option<ShadowSemanticEvent>,
}

#[cfg(any(test, feature = "shadow-compare"))]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShadowComparisonStats {
    pub comparisons: u64,
    pub compared_entities: u64,
    pub compared_events: u64,
    pub first_divergence: Option<ShadowDivergence>,
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
struct PendingInputCommands(Vec<ShadowInputCommand>);

#[derive(Resource, Default)]
struct PendingPhysicsResults(Vec<ShadowPhysicsResult>);

#[derive(Resource, Default)]
struct PendingGoalTick(Option<ShadowGoalTickRequest>);

struct ShadowGoalTickRequest {
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
struct PendingCombatCommands(Vec<ShadowCombatCommand>);

#[derive(Resource, Default)]
struct SnapshotRequest(bool);

#[derive(Resource, Default)]
struct SnapshotOutput(Vec<EntitySnapshot>);

#[derive(Resource, Default)]
struct PersistenceExtractRequest(bool);

#[derive(Resource, Default)]
struct PersistenceOutput(Vec<EntitySnapshot>);

#[cfg(any(test, feature = "shadow-compare"))]
#[derive(Resource, Default)]
struct PendingSemanticEvents(Vec<ShadowSemanticEvent>);

#[cfg(any(test, feature = "shadow-compare"))]
#[derive(Resource, Default)]
struct PublishedSemanticEvents(Vec<ShadowSemanticEvent>);

struct ShadowSchedules {
    input_ai: Schedule,
    snapshot_request: Schedule,
    physics_apply: Schedule,
    combat_lifecycle: Schedule,
    persistence_extract: Schedule,
    #[cfg(any(test, feature = "shadow-compare"))]
    output_events: Schedule,
}

/// ECS representation used for production authority and deterministic shadow checks.
pub struct ShadowEntityRuntime {
    world: World,
    schedules: ShadowSchedules,
    #[cfg(test)]
    input_ai_stage_runs: usize,
}

impl fmt::Debug for ShadowEntityRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ShadowEntityRuntime")
            .field(
                "entities",
                &self.world.resource::<RuntimeEntityIndex>().0.len(),
            )
            .finish_non_exhaustive()
    }
}

impl Default for ShadowEntityRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl ShadowEntityRuntime {
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
        #[cfg(any(test, feature = "shadow-compare"))]
        world.init_resource::<PendingSemanticEvents>();
        #[cfg(any(test, feature = "shadow-compare"))]
        world.init_resource::<PublishedSemanticEvents>();
        Self {
            world,
            schedules: ShadowSchedules::new(),
            #[cfg(test)]
            input_ai_stage_runs: 0,
        }
    }

    pub fn insert_snapshot(&mut self, snapshot: EntitySnapshot) -> bool {
        #[cfg(not(any(test, feature = "shadow-compare")))]
        {
            insert_snapshot_into_world(&mut self.world, snapshot, true)
        }
        #[cfg(any(test, feature = "shadow-compare"))]
        {
            insert_snapshot_into_world(&mut self.world, snapshot, false)
        }
    }

    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.world.resource::<RuntimeEntityIndex>().0.len()
    }

    #[must_use]
    #[cfg(not(any(test, feature = "shadow-compare")))]
    pub(crate) fn is_empty(&self) -> bool {
        self.world.resource::<RuntimeEntityIndex>().0.is_empty()
    }

    #[must_use]
    pub fn snapshot(&self, id: EntityId) -> Option<EntitySnapshot> {
        snapshot_from_world(&self.world, id)
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
        Some(EntityMotionState {
            id: identity.id,
            position: transform.position,
            rotation: transform.rotation,
            velocity: motion.velocity,
            on_ground: motion.on_ground,
            is_item: entity_type.name == "minecraft:item",
            is_experience: entity_type.name == "minecraft:experience_orb",
            is_arrow: entity_type.name == "minecraft:arrow",
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

    #[cfg(not(any(test, feature = "shadow-compare")))]
    pub(crate) fn visit_entities(&self, visitor: &mut impl FnMut(EntityView<'_>)) {
        for view in self.views() {
            visitor(view);
        }
    }

    #[cfg(not(any(test, feature = "shadow-compare")))]
    pub(crate) fn visit_breeding_tick_entities(&self, visitor: &mut impl FnMut(EntityView<'_>)) {
        for &id in &self.world.resource::<BreedingTickEntities>().0 {
            if let Some(view) = entity_view_from_world(&self.world, id) {
                visitor(view);
            }
        }
    }

    #[cfg(not(any(test, feature = "shadow-compare")))]
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
            let mut query = self.world.query_filtered::<(
                &StableIdentity,
                &TransformState,
                &MotionState,
                &LifecycleState,
            ), With<AuthoritativeState>>();
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
                if !entity.contains::<AuthoritativeState>()
                    || entity.get::<LifecycleState>()?.0 != EntityLifecycle::Alive
                {
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

    #[cfg(any(test, feature = "shadow-compare"))]
    pub(crate) fn entity_ids(&self) -> Vec<EntityId> {
        self.world
            .resource::<RuntimeEntityIndex>()
            .0
            .keys()
            .copied()
            .collect()
    }

    #[cfg(any(test, feature = "shadow-compare"))]
    pub(crate) fn visit_authoritative_entities(
        &self,
        authoritative_ids: &HashSet<EntityId>,
        visitor: &mut impl FnMut(EntityView<'_>),
    ) {
        for &id in self.world.resource::<RuntimeEntityIndex>().0.keys() {
            if authoritative_ids.contains(&id)
                && let Some(view) = entity_view_from_world(&self.world, id)
            {
                visitor(view);
            }
        }
    }

    pub(crate) fn visit_authoritative_entity(
        &self,
        id: EntityId,
        visitor: &mut impl FnMut(EntityView<'_>),
    ) {
        if let Some(view) = entity_view_from_world(&self.world, id) {
            visitor(view);
        }
    }

    #[cfg(any(test, feature = "shadow-compare"))]
    pub(crate) fn visit_authoritative_breeding_tick_entities(
        &self,
        authoritative_ids: &HashSet<EntityId>,
        visitor: &mut impl FnMut(EntityView<'_>),
    ) {
        for &id in &self.world.resource::<BreedingTickEntities>().0 {
            if authoritative_ids.contains(&id)
                && let Some(view) = entity_view_from_world(&self.world, id)
            {
                visitor(view);
            }
        }
    }

    #[cfg(any(test, feature = "shadow-compare"))]
    pub(crate) fn visit_authoritative_sheep_entities_for_ids(
        &self,
        authoritative_ids: &HashSet<EntityId>,
        candidate_ids: &HashSet<EntityId>,
        visitor: &mut impl FnMut(EntityView<'_>),
    ) {
        let sheep_ids = &self.world.resource::<SheepEntities>().0;
        if candidate_ids.len() < sheep_ids.len() {
            let mut ordered_ids = candidate_ids.iter().copied().collect::<Vec<_>>();
            ordered_ids.sort_unstable();
            for id in ordered_ids {
                if sheep_ids.contains(&id)
                    && authoritative_ids.contains(&id)
                    && let Some(view) = entity_view_from_world(&self.world, id)
                {
                    visitor(view);
                }
            }
            return;
        }
        for &id in sheep_ids {
            if candidate_ids.contains(&id)
                && authoritative_ids.contains(&id)
                && let Some(view) = entity_view_from_world(&self.world, id)
            {
                visitor(view);
            }
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
            With<AuthoritativeState>,
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
        if !entity.contains::<AuthoritativeState>()
            || entity.contains::<ItemStackState>()
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

    #[cfg(any(test, feature = "shadow-compare"))]
    pub(crate) fn matches_view(
        &self,
        view: EntityView<'_>,
        expected_path: &RetainedPathState,
    ) -> bool {
        let Some(ecs_entity) = self
            .world
            .resource::<RuntimeEntityIndex>()
            .0
            .get(&view.id)
            .copied()
        else {
            return false;
        };
        let Ok(entity) = self.world.get_entity(ecs_entity) else {
            return false;
        };
        let vehicle = entity.get::<VehicleKindState>().map(|kind| VehicleState {
            kind: kind.0,
            passenger: entity.get::<PassengerState>().map(|passenger| passenger.0),
        });

        entity
            .get::<StableIdentity>()
            .is_some_and(|identity| identity.id == view.id && identity.uuid == view.uuid)
            && entity.get::<EntityTypeState>().is_some_and(|entity_type| {
                entity_type.protocol_id == view.type_id && entity_type.name == view.type_name
            })
            && entity.get::<TransformState>().is_some_and(|transform| {
                transform.position == view.position && transform.rotation == view.rotation
            })
            && entity.get::<MotionState>().is_some_and(|motion| {
                motion.velocity == view.velocity && motion.on_ground == view.on_ground
            })
            && entity
                .get::<LifecycleState>()
                .is_some_and(|lifecycle| lifecycle.0 == view.lifecycle)
            && entity.get::<LivingState>().is_some_and(|living| {
                living.health == view.health && &living.attributes == view.attributes
            })
            && entity
                .get::<AiGoalState>()
                .is_some_and(|goal| &goal.0 == view.goal)
            && entity
                .get::<AiPathState>()
                .is_some_and(|path| &path.0 == expected_path)
            && entity.get::<ItemStackState>().map(|stack| stack.0.clone()) == view.item_stack
            && entity
                .get::<ExperienceState>()
                .map(|experience| experience.0)
                == view.experience_value
            && entity.get::<FallingBlockState>().map(|block| block.0) == view.block_state
            && entity.contains::<ProjectileState>() == (view.type_name == "minecraft:arrow")
            && entity.contains::<PersistentState>()
            && entity.contains::<VisibilityState>()
            && vehicle == view.vehicle
            && entity.get::<AnimalState>().map(|state| state.0) == view.animal
    }

    pub fn remove(&mut self, id: EntityId) -> Option<EntitySnapshot> {
        remove_from_world(&mut self.world, id)
    }

    pub fn queue_input(&mut self, command: ShadowInputCommand) {
        self.world
            .resource_mut::<PendingInputCommands>()
            .0
            .push(command);
    }

    pub fn queue_physics(&mut self, result: ShadowPhysicsResult) {
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
        let request = ShadowGoalTickRequest {
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

    pub fn queue_combat(&mut self, command: ShadowCombatCommand) {
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

    pub fn run_stage(&mut self, stage: ShadowStage) {
        match stage {
            ShadowStage::InputAi => {
                #[cfg(test)]
                {
                    self.input_ai_stage_runs = self.input_ai_stage_runs.saturating_add(1);
                }
                self.schedules.input_ai.run(&mut self.world);
            }
            ShadowStage::SnapshotRequest => {
                self.schedules.snapshot_request.run(&mut self.world);
            }
            ShadowStage::PhysicsApply => self.schedules.physics_apply.run(&mut self.world),
            ShadowStage::CombatLifecycle => {
                self.schedules.combat_lifecycle.run(&mut self.world);
            }
            ShadowStage::PersistenceExtract => {
                self.schedules.persistence_extract.run(&mut self.world);
            }
            #[cfg(any(test, feature = "shadow-compare"))]
            ShadowStage::OutputEvents => self.schedules.output_events.run(&mut self.world),
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

    #[cfg(any(test, feature = "shadow-compare"))]
    pub(crate) fn take_output_events(&mut self) -> Vec<ShadowSemanticEvent> {
        std::mem::take(&mut self.world.resource_mut::<PublishedSemanticEvents>().0)
    }

    #[cfg(any(test, feature = "shadow-compare"))]
    pub(crate) fn semantic_event_checkpoint(&self) -> (usize, usize) {
        (
            self.world.resource::<PendingSemanticEvents>().0.len(),
            self.world.resource::<PublishedSemanticEvents>().0.len(),
        )
    }

    #[cfg(any(test, feature = "shadow-compare"))]
    pub(crate) fn restore_semantic_event_checkpoint(&mut self, checkpoint: (usize, usize)) {
        self.world
            .resource_mut::<PendingSemanticEvents>()
            .0
            .truncate(checkpoint.0);
        self.world
            .resource_mut::<PublishedSemanticEvents>()
            .0
            .truncate(checkpoint.1);
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

    #[cfg(test)]
    pub(crate) fn perturb_path_for_test(&mut self, id: EntityId, path: RetainedPathState) {
        let Some(entity) = self
            .world
            .resource::<RuntimeEntityIndex>()
            .0
            .get(&id)
            .copied()
        else {
            return;
        };
        let Ok(mut entity) = self.world.get_entity_mut(entity) else {
            return;
        };
        if let Some(mut current) = entity.get_mut::<AiPathState>() {
            current.0 = path;
        }
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

impl ShadowSchedules {
    fn new() -> Self {
        let mut input_ai = Schedule::default();
        input_ai.set_executor_kind(ExecutorKind::SingleThreaded);
        input_ai.add_systems((apply_input_commands, apply_authoritative_goal_tick).chain());

        let mut snapshot_request = Schedule::default();
        snapshot_request.set_executor_kind(ExecutorKind::SingleThreaded);
        snapshot_request.add_systems(capture_snapshot_request);

        let mut physics_apply = Schedule::default();
        physics_apply.set_executor_kind(ExecutorKind::SingleThreaded);
        physics_apply
            .add_systems((apply_physics_results, integrate_authoritative_positions).chain());

        let mut combat_lifecycle = Schedule::default();
        combat_lifecycle.set_executor_kind(ExecutorKind::SingleThreaded);
        combat_lifecycle.add_systems(apply_combat_commands);

        let mut persistence_extract = Schedule::default();
        persistence_extract.set_executor_kind(ExecutorKind::SingleThreaded);
        persistence_extract.add_systems(extract_persistence_snapshots);

        #[cfg(any(test, feature = "shadow-compare"))]
        let mut output_events = Schedule::default();
        #[cfg(any(test, feature = "shadow-compare"))]
        output_events.set_executor_kind(ExecutorKind::SingleThreaded);
        #[cfg(any(test, feature = "shadow-compare"))]
        output_events.add_systems(publish_semantic_events);

        Self {
            input_ai,
            snapshot_request,
            physics_apply,
            combat_lifecycle,
            persistence_extract,
            #[cfg(any(test, feature = "shadow-compare"))]
            output_events,
        }
    }
}

fn insert_snapshot_into_world(
    world: &mut World,
    snapshot: EntitySnapshot,
    authoritative: bool,
) -> bool {
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
    } = snapshot;
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
        LivingState { health, attributes },
        AiGoalState(goal),
        AiPathState::default(),
        PersistentState,
        VisibilityState,
    ));
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
    if authoritative {
        entity.insert(AuthoritativeState);
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
        health: living.health,
        attributes: living.attributes.clone(),
        goal: goal.0.clone(),
        vehicle,
        animal: entity.get::<AnimalState>().map(|state| state.0),
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
        health: living.health,
        attributes: &living.attributes,
        goal: &goal.0,
        vehicle,
        animal: entity.get::<AnimalState>().map(|state| state.0),
    })
}

fn normalized_snapshots_from_world(world: &World) -> Vec<EntitySnapshot> {
    world
        .resource::<RuntimeEntityIndex>()
        .0
        .keys()
        .map(|&id| {
            snapshot_from_world(world, id)
                .expect("shadow runtime id index must reference a complete ECS entity")
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
    debug_assert!(
        removed,
        "shadow runtime id index referenced a missing entity"
    );
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

#[cfg(any(test, feature = "shadow-compare"))]
fn push_pending_event(world: &mut World, event: ShadowSemanticEvent) {
    world.resource_mut::<PendingSemanticEvents>().0.push(event);
}

fn apply_input_commands(world: &mut World) {
    let commands = std::mem::take(&mut world.resource_mut::<PendingInputCommands>().0);
    for command in commands {
        match command {
            #[cfg(any(test, feature = "shadow-compare"))]
            ShadowInputCommand::Insert(snapshot) => {
                #[cfg(any(test, feature = "shadow-compare"))]
                let id = snapshot.id;
                if insert_snapshot_into_world(world, snapshot, false) {
                    #[cfg(any(test, feature = "shadow-compare"))]
                    push_pending_event(world, ShadowSemanticEvent::Spawned { id });
                }
            }
            ShadowInputCommand::InsertAuthoritative(snapshot) => {
                #[cfg(any(test, feature = "shadow-compare"))]
                let id = snapshot.id;
                if insert_snapshot_into_world(world, snapshot, true) {
                    #[cfg(any(test, feature = "shadow-compare"))]
                    push_pending_event(world, ShadowSemanticEvent::Spawned { id });
                }
            }
            ShadowInputCommand::SetGoal { id, goal } => {
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
                }
                #[cfg(any(test, feature = "shadow-compare"))]
                push_pending_event(world, ShadowSemanticEvent::GoalChanged { id });
            }
            ShadowInputCommand::ResetPath { id } => {
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
            ShadowInputCommand::SetItemStack { id, stack } => {
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
                }
                #[cfg(any(test, feature = "shadow-compare"))]
                push_pending_event(world, ShadowSemanticEvent::ItemStackChanged { id });
            }
            ShadowInputCommand::SetVehicle { id, vehicle } => {
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
                }
                #[cfg(any(test, feature = "shadow-compare"))]
                push_pending_event(world, ShadowSemanticEvent::VehicleChanged { id });
            }
            ShadowInputCommand::SetAnimalState { id, animal } => {
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

fn apply_authoritative_goal_tick(world: &mut World) {
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
            if !entity.contains::<AuthoritativeState>() {
                continue;
            }
            if let Some(AiGoalState(GoalState::FollowTarget { target, .. })) =
                entity.get::<AiGoalState>()
            {
                target_ids.push(*target);
            }
        }
    } else {
        let mut identity_query =
            world.query_filtered::<(&StableIdentity, &AiGoalState), With<AuthoritativeState>>();
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
        With<AuthoritativeState>,
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
    request: &ShadowGoalTickRequest,
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

fn integrate_authoritative_positions(world: &mut World) {
    let Some((delta_seconds, active_ids)) = world.resource_mut::<PendingPositionTick>().0.take()
    else {
        return;
    };
    let mut query = world.query_filtered::<(
        &StableIdentity,
        &mut TransformState,
        &MotionState,
        &LifecycleState,
    ), With<AuthoritativeState>>();
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
        #[cfg(any(test, feature = "shadow-compare"))]
        let authoritative;
        {
            let Ok(mut entity) = world.get_entity_mut(entity) else {
                continue;
            };
            #[cfg(any(test, feature = "shadow-compare"))]
            {
                authoritative = entity.contains::<AuthoritativeState>();
            }
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
        #[cfg(any(test, feature = "shadow-compare"))]
        if !authoritative {
            push_pending_event(world, ShadowSemanticEvent::PhysicsApplied { id: result.id });
        }
    }
}

fn apply_combat_commands(world: &mut World) {
    let commands = std::mem::take(&mut world.resource_mut::<PendingCombatCommands>().0);
    for command in commands {
        match command {
            ShadowCombatCommand::Damage { id, amount } => {
                let Some(entity) = ecs_entity_for(world, id) else {
                    continue;
                };
                let (health, killed) = {
                    let Ok(mut entity) = world.get_entity_mut(entity) else {
                        continue;
                    };
                    if entity
                        .get::<LifecycleState>()
                        .is_none_or(|state| state.0 != EntityLifecycle::Alive)
                    {
                        continue;
                    }
                    let (health, killed) = {
                        let Some(mut living) = entity.get_mut::<LivingState>() else {
                            continue;
                        };
                        living.health = (living.health - amount.max(0.0)).max(0.0);
                        (living.health, living.health <= 0.0)
                    };
                    if killed && let Some(mut lifecycle) = entity.get_mut::<LifecycleState>() {
                        lifecycle.0 = EntityLifecycle::Despawning;
                    }
                    (health, killed)
                };
                if killed {
                    world.resource_mut::<BreedingTickEntities>().0.remove(&id);
                    world.resource_mut::<SheepEntities>().0.remove(&id);
                }
                #[cfg(any(test, feature = "shadow-compare"))]
                push_pending_event(world, ShadowSemanticEvent::Damaged { id, health, killed });
                #[cfg(not(any(test, feature = "shadow-compare")))]
                let _ = health;
            }
            ShadowCombatCommand::MarkDespawning { id } => {
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
                #[cfg(any(test, feature = "shadow-compare"))]
                push_pending_event(
                    world,
                    ShadowSemanticEvent::LifecycleChanged {
                        id,
                        lifecycle: EntityLifecycle::Despawning,
                    },
                );
            }
            ShadowCombatCommand::Remove { id } => {
                if remove_from_world(world, id).is_some() {
                    #[cfg(any(test, feature = "shadow-compare"))]
                    push_pending_event(world, ShadowSemanticEvent::Removed { id });
                }
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

#[cfg(any(test, feature = "shadow-compare"))]
fn publish_semantic_events(world: &mut World) {
    let events = std::mem::take(&mut world.resource_mut::<PendingSemanticEvents>().0);
    world
        .resource_mut::<PublishedSemanticEvents>()
        .0
        .extend(events);
}

fn ecs_entity_for(world: &World, id: EntityId) -> Option<EcsEntity> {
    world.resource::<RuntimeEntityIndex>().0.get(&id).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let mut shadow = ShadowEntityRuntime::new();
        for entity in expected.iter().rev().cloned() {
            assert!(shadow.insert_snapshot(entity));
        }

        assert_eq!(shadow.normalized_snapshots(), expected);
        assert!(shadow.has_projectile(EntityId(4)));
        assert!(shadow.has_persistent_state(EntityId(1)));
        assert!(shadow.has_visibility_state(EntityId(7)));
        assert_eq!(shadow.remove(EntityId(6)), Some(expected[5].clone()));
        assert_eq!(shadow.snapshot(EntityId(6)), None);
    }

    #[test]
    fn duplicate_runtime_id_is_rejected_without_replacing_state() {
        let original = snapshot(9, 9, "minecraft:cow");
        let mut replacement = original.clone();
        replacement.position = Vec3::new(99.0, 99.0, 99.0);
        let mut shadow = ShadowEntityRuntime::new();

        assert!(shadow.insert_snapshot(original.clone()));
        assert!(!shadow.insert_snapshot(replacement));
        assert_eq!(shadow.snapshot(EntityId(9)), Some(original));
    }

    #[test]
    fn duplicate_uuid_is_rejected_and_remove_releases_uuid() {
        let original = snapshot(9, 9, "minecraft:cow");
        let mut duplicate = snapshot(10, 10, "minecraft:sheep");
        duplicate.uuid = original.uuid;
        let mut shadow = ShadowEntityRuntime::new();

        assert!(shadow.insert_snapshot(original.clone()));
        assert!(!shadow.insert_snapshot(duplicate.clone()));
        assert!(shadow.contains_uuid(original.uuid));
        assert_eq!(shadow.remove(original.id), Some(original));
        assert!(!shadow.contains_uuid(duplicate.uuid));
        assert!(shadow.insert_snapshot(duplicate));
    }

    #[test]
    fn schedules_apply_only_their_owned_operations() {
        let initial = snapshot(10, 10, "minecraft:zombie");
        let moved_position = Vec3::new(20.0, 65.0, 30.0);
        let moved_velocity = Vec3::new(0.5, -0.1, 0.25);
        let mut shadow = ShadowEntityRuntime::new();
        shadow.queue_input(ShadowInputCommand::Insert(initial.clone()));
        shadow.request_snapshots();

        shadow.run_stage(ShadowStage::SnapshotRequest);
        assert_eq!(shadow.take_snapshot_output(), Vec::new());
        assert_eq!(shadow.snapshot(initial.id), None);

        shadow.run_stage(ShadowStage::InputAi);
        assert_eq!(shadow.snapshot(initial.id), Some(initial.clone()));
        assert!(shadow.take_output_events().is_empty());

        shadow.queue_physics(ShadowPhysicsResult {
            id: initial.id,
            position: moved_position,
            rotation: initial.rotation,
            velocity: moved_velocity,
            on_ground: false,
        });
        shadow.queue_combat(ShadowCombatCommand::Damage {
            id: initial.id,
            amount: 5.0,
        });
        shadow.run_stage(ShadowStage::CombatLifecycle);
        let damaged = shadow.snapshot(initial.id).unwrap();
        assert_eq!(damaged.health, initial.health - 5.0);
        assert_eq!(damaged.position, initial.position);

        shadow.run_stage(ShadowStage::PhysicsApply);
        let moved = shadow.snapshot(initial.id).unwrap();
        assert_eq!(moved.position, moved_position);
        assert_eq!(moved.velocity, moved_velocity);
        assert!(!moved.on_ground);

        shadow.request_snapshots();
        shadow.run_stage(ShadowStage::SnapshotRequest);
        assert_eq!(shadow.take_snapshot_output(), vec![moved.clone()]);

        shadow.request_persistence_extract();
        shadow.run_stage(ShadowStage::PersistenceExtract);
        assert_eq!(shadow.take_persistence_output(), vec![moved]);
        assert!(shadow.take_output_events().is_empty());

        shadow.run_stage(ShadowStage::OutputEvents);
        assert_eq!(
            shadow.take_output_events(),
            vec![
                ShadowSemanticEvent::Spawned { id: initial.id },
                ShadowSemanticEvent::Damaged {
                    id: initial.id,
                    health: initial.health - 5.0,
                    killed: false,
                },
                ShadowSemanticEvent::PhysicsApplied { id: initial.id },
            ]
        );
    }

    #[test]
    fn lethal_damage_then_remove_has_one_ordered_lifecycle_result() {
        let initial = snapshot(11, 11, "minecraft:cow");
        let mut shadow = ShadowEntityRuntime::new();
        assert!(shadow.insert_snapshot(initial.clone()));

        shadow.queue_combat(ShadowCombatCommand::Damage {
            id: initial.id,
            amount: initial.health,
        });
        shadow.queue_combat(ShadowCombatCommand::Remove { id: initial.id });
        shadow.run_stage(ShadowStage::CombatLifecycle);
        shadow.run_stage(ShadowStage::OutputEvents);

        assert_eq!(shadow.snapshot(initial.id), None);
        assert_eq!(
            shadow.take_output_events(),
            vec![
                ShadowSemanticEvent::Damaged {
                    id: initial.id,
                    health: 0.0,
                    killed: true,
                },
                ShadowSemanticEvent::Removed { id: initial.id },
            ]
        );
    }
}
