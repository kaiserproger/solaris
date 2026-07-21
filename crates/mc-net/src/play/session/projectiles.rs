use std::collections::{HashMap, HashSet};

use mc_entity::projectile_26_1_2::{
    Aabb as ProjectileAabb, ArrowBlockHit, ArrowDamageResolution, ArrowEntityHit,
    ArrowEntityResolution, ArrowState, ArrowTickInput, BlockStateId as ProjectileBlockStateId,
    EntityId as ProjectileEntityId, EntityIdentity, HitEligibility, InputStamp,
    MAX_PIERCED_ENTITIES, OwnerCollisionInput, OwnerVehicleMember, PickupMode, ProjectileLifecycle,
    ProjectilePublication, ProjectileState, ResolvedDeflection, Rotation as ProjectileRotation,
    Vec3 as ProjectileVec3, commit_arrow_tick, prepare_arrow_tick,
};
use mc_entity::{
    EntityDamage, EntityId, EntityLifecycle, EntityMotionState, EntitySnapshot, Rotation,
    SpawnEntity, Vec3,
};

use crate::play::combat::{PlayerDamageKind, PlayerDamageRequest};
use crate::play::spawn::chunk_pos_from_coords;
use crate::play::survival::{entity_item_stack, mob_xp_value};
use crate::play::{
    ARROW_ENTITY_HIT_DAMAGE, ARROW_ENTITY_HIT_KNOCKBACK, ArrowPhysicsFact, EntityPhysicsStep,
};

use super::entity_combat::{
    begin_server_entity_death_locked, publish_accepted_entity_health_locked,
};
use super::entity_lifecycle::{remove_server_entity_locked, track_entity_chunk_locked};
use super::interaction_geometry::{entity_aabb, entity_geometry};
use super::outbound::{OutboundCommand, ServerEntityMove, VisibilityDispatch};
use super::player_combat::{
    PreparedProjectilePlayerDamage, ProjectilePlayerDamagePreview,
    commit_projectile_player_damage_locked, prepare_projectile_player_damage_locked,
};
use super::visibility::{
    entity_event_dispatches_locked, initialize_entity_wire_state_locked, ordered_session_recipient,
    publish_server_entity_snapshot_locked, spawn_entity_visibility_locked,
    visible_entity_observers_locked,
};
use super::{
    ENTITY_DEATH_TICKS, ENTITY_HURT_INVULNERABLE_TICKS, EntityKillRewards, SessionEntityGuards,
    SessionId, apply_entity_facts, entity_kill_drop_stacks, player_aabb, player_collision_position,
};

const MAX_ARROW_TICK_CANDIDATES: usize = MAX_PIERCED_ENTITIES + 1;
const MAX_OWNER_VEHICLE_MEMBERS: usize = 8;

fn projectile_vec(value: Vec3) -> ProjectileVec3 {
    ProjectileVec3::new(value.x, value.y, value.z)
}

fn session_vec(value: ProjectileVec3) -> Vec3 {
    Vec3::new(value.x, value.y, value.z)
}

fn projectile_entity(value: EntityId) -> ProjectileEntityId {
    ProjectileEntityId::new(value.0)
}

fn session_entity(value: ProjectileEntityId) -> EntityId {
    EntityId(value.raw())
}

fn segment_location(start: Vec3, end: Vec3, fraction: f64) -> Vec3 {
    Vec3::new(
        start.x + (end.x - start.x) * fraction,
        start.y + (end.y - start.y) * fraction,
        start.z + (end.z - start.z) * fraction,
    )
}

pub(super) fn spawn_arrow_locked(
    inner: &mut SessionEntityGuards<'_>,
    owner_session: Option<SessionId>,
    entity_type_id: i32,
    position: Vec3,
    velocity: Vec3,
    rotation: Rotation,
) -> (EntityId, Vec<VisibilityDispatch>) {
    let owner = owner_session
        .and_then(|session_id| inner.sessions.get(&session_id))
        .map(|session| projectile_identity(EntityId(session.entity_id)));
    let mut entity = SpawnEntity::new(entity_type_id, "minecraft:arrow", position);
    entity.velocity = velocity;
    entity.rotation = rotation;
    entity.on_ground = false;
    apply_entity_facts(&mut entity);
    entity.retained.spawn_tick = inner.entity_lifecycle_tick;
    entity.retained.arrow_state = Some(
        initial_arrow_state(owner, position, velocity, rotation)
            .expect("finite spawned arrow must produce a valid kernel state"),
    );
    let aabb = entity_aabb(&entity.type_name);
    let id = inner.entities.spawn(entity);
    inner
        .entity_type_aabbs
        .entry(entity_type_id)
        .or_insert(aabb);
    track_entity_chunk_locked(inner, id, position);
    initialize_entity_wire_state_locked(inner, id);
    let dispatches = spawn_entity_visibility_locked(inner, id);
    (id, dispatches)
}

pub(super) fn resolve_arrow_entity_hits_locked<'a>(
    registry: &'a super::SessionRegistry,
    mut inner: SessionEntityGuards<'a>,
    steps: &[EntityPhysicsStep],
    old_motion: &HashMap<EntityId, EntityMotionState>,
    physics_facts: &[ArrowPhysicsFact],
    dispatches: &mut Vec<VisibilityDispatch>,
) -> SessionEntityGuards<'a> {
    let mut scratch = std::mem::take(&mut inner.arrow_tick_scratch);
    scratch.rejected.clear();
    scratch.processed.clear();
    scratch.grounded_transaction.clear();
    scratch.grounded_ids.clear();
    scratch.grounded_discards.clear();
    scratch.grounded_visibility.clear();
    let arrow_count = steps
        .iter()
        .filter(|step| {
            old_motion
                .get(&step.id)
                .is_some_and(|motion| motion.is_arrow)
        })
        .count();
    let batch_start = if arrow_count == 0 {
        0
    } else {
        scratch.next_arrow_batch_start % arrow_count
    };
    let batch_len = arrow_count.min(MAX_ARROW_TICK_CANDIDATES);
    let mut arrow_rank = 0;
    for step in steps {
        let Some(motion) = old_motion.get(&step.id) else {
            continue;
        };
        if !motion.is_arrow {
            continue;
        }
        let rank = arrow_rank;
        arrow_rank += 1;
        if !arrow_rank_is_in_batch(rank, batch_start, batch_len, arrow_count) {
            continue;
        }
        scratch.processed.push(step.id);
        let Some(physics_fact) = physics_facts
            .iter()
            .find(|fact| fact.arrow_id == step.id)
            .copied()
        else {
            scratch.rejected.push(step.id);
            continue;
        };
        if !inner.entities.contains(step.id) {
            scratch.rejected.push(step.id);
            continue;
        }
        let start = motion.position;
        let Some(expected) = inner.entities.snapshot(step.id) else {
            scratch.rejected.push(step.id);
            continue;
        };
        if expected.position != motion.position
            || expected.velocity != motion.velocity
            || expected.on_ground != motion.on_ground
        {
            scratch.rejected.push(step.id);
            continue;
        }
        let Some(mut state) = rebased_arrow_state(&expected, motion, step) else {
            scratch.rejected.push(step.id);
            continue;
        };
        let was_grounded = state.in_ground;
        if was_grounded {
            scratch.candidate_snapshots.clear();
            scratch.player_ids.clear();
            scratch.hits.clear();
            scratch.targets.clear();
            scratch.owner_members.clear();
            scratch.owner_vehicle_entities.clear();
        } else if !prepare_arrow_tick_candidates_locked(
            &inner,
            step.id,
            &state,
            start,
            step.position,
            &mut scratch,
        ) {
            scratch.rejected.push(step.id);
            continue;
        }
        let block_hit = physics_fact.block_hit.map(|hit| {
            // Deliberate vanilla bug fix: an entity exactly at the world collision
            // endpoint loses the tie to that block. The kernel enforces this with
            // its strict block-before-entity distance rule.
            ArrowBlockHit::block(
                ProjectileBlockStateId::new(hit.block_state.0),
                hit.block_position,
                projectile_vec(hit.location),
            )
        });
        let stamp = InputStamp {
            world_revision: inner.entity_lifecycle_tick,
            collision_revision: inner.entity_lifecycle_tick,
            resolution_revision: inner.entity_lifecycle_tick,
        };
        let input = ArrowTickInput {
            stamp,
            owner_collision: owner_collision_input(&state, &scratch.owner_members),
            embedded_in_block: physics_fact.embedded_in_block,
            current_block_state: ProjectileBlockStateId::new(physics_fact.current_block_state.0),
            should_fall: physics_fact.should_fall,
            fall_velocity_scale: physics_fact
                .should_fall
                .then_some(projectile_vec(physics_fact.fall_velocity_scale)),
            in_water: physics_fact.in_water,
            in_water_or_rain: physics_fact.in_water_or_rain,
            no_gravity: state.no_gravity,
            block_hit,
            entity_hits: &mut scratch.hits,
        };
        if was_grounded {
            let outcome = prepare_arrow_tick(&state, input)
                .ok()
                .and_then(|plan| commit_arrow_tick(&mut state, stamp, plan).ok());
            let Some(_outcome) = outcome else {
                scratch.rejected.push(step.id);
                continue;
            };
            let discard = state.projectile.lifecycle == ProjectileLifecycle::Discarded;
            if !state.in_ground && !discard {
                scratch.grounded_visibility.push(step.id);
            }
            if discard {
                scratch.grounded_discards.push(step.id);
            }
            scratch.grounded_ids.push(step.id);
            let next = arrow_snapshot_with_state(&expected, state);
            scratch.grounded_transaction.push((expected, next));
            continue;
        }
        drop(inner);
        #[cfg(test)]
        registry.pause_before_arrow_transaction_for_test();
        let outcome = prepare_arrow_tick(&state, input)
            .ok()
            .and_then(|plan| commit_arrow_tick(&mut state, stamp, plan).ok());
        inner = registry.lock_session_entities("commit prepared arrow tick");
        let Some(outcome) = outcome else {
            scratch.rejected.push(step.id);
            continue;
        };
        let next = arrow_snapshot_with_state(&expected, state);
        let Ok(discard_arrow) = commit_arrow_transaction_locked(
            &mut inner,
            expected,
            next,
            start,
            &outcome.publications,
            &mut scratch,
            dispatches,
        ) else {
            scratch.rejected.push(step.id);
            continue;
        };
        if discard_arrow {
            if let Some((_, arrow_dispatches)) = remove_server_entity_locked(&mut inner, step.id) {
                dispatches.extend(arrow_dispatches);
            }
        } else if inner.entities.contains(step.id) {
            synchronize_arrow_snapshot_locked(&mut inner, step.id);
        }
    }
    if !scratch.grounded_transaction.is_empty() {
        drop(inner);
        let mut entities = registry.lock_entities("commit grounded arrow batch");
        let committed =
            entities.replace_snapshots_if_current(scratch.grounded_transaction.drain(..));
        if committed {
            let grounded_ids = scratch.grounded_ids.iter().copied().collect::<HashSet<_>>();
            entities.prefetch(&grounded_ids);
        }
        let session_inner = registry.lock_inner("publish grounded arrow batch");
        inner = SessionEntityGuards {
            inner: session_inner,
            entities,
            entity_lifecycle_tick: registry.simulation_tick(),
        };
        if committed {
            for arrow_id in scratch.grounded_discards.drain(..) {
                if let Some((_, arrow_dispatches)) =
                    remove_server_entity_locked(&mut inner, arrow_id)
                {
                    dispatches.extend(arrow_dispatches);
                }
            }
            for arrow_id in scratch.grounded_visibility.drain(..) {
                if inner.entities.contains(arrow_id) {
                    synchronize_arrow_snapshot_locked(&mut inner, arrow_id);
                }
            }
        } else {
            scratch
                .rejected
                .extend(scratch.grounded_ids.iter().copied());
        }
    }
    scratch.next_arrow_batch_start = if arrow_count > batch_len {
        (batch_start + batch_len) % arrow_count
    } else {
        0
    };
    inner.arrow_tick_scratch = scratch;
    inner
}

fn arrow_rank_is_in_batch(rank: usize, start: usize, len: usize, count: usize) -> bool {
    if len == count {
        return true;
    }
    let offset = if rank >= start {
        rank - start
    } else {
        count - start + rank
    };
    offset < len
}

struct ArrowTickTarget {
    entity_id: EntityId,
    entity_location: Option<Vec3>,
    expected_entity: Option<EntitySnapshot>,
    committed_damage: Option<EntityDamage>,
    player_damage: Option<PreparedProjectilePlayerDamage>,
}

/// Reused owner-session storage for bounded projectile work. Candidate and
/// admitted-arrow buffers reject before their fixed capacities can grow; the
/// cursor rotates the admitted window across the stable entity-id step order.
pub(super) struct ArrowTickScratch {
    candidate_snapshots: Vec<EntitySnapshot>,
    player_ids: Vec<SessionId>,
    hits: Vec<ArrowEntityHit>,
    targets: Vec<ArrowTickTarget>,
    owner_members: Vec<OwnerVehicleMember>,
    owner_vehicle_entities: Vec<EntityId>,
    transaction: Vec<(EntitySnapshot, EntitySnapshot)>,
    grounded_transaction: Vec<(EntitySnapshot, EntitySnapshot)>,
    grounded_ids: Vec<EntityId>,
    grounded_discards: Vec<EntityId>,
    grounded_visibility: Vec<EntityId>,
    pub(super) processed: Vec<EntityId>,
    pub(super) rejected: Vec<EntityId>,
    next_arrow_batch_start: usize,
}

impl std::fmt::Debug for ArrowTickScratch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ArrowTickScratch")
            .field("candidate_snapshots", &self.candidate_snapshots.len())
            .field("player_ids", &self.player_ids.len())
            .field("hits", &self.hits.len())
            .field("targets", &self.targets.len())
            .field("owner_members", &self.owner_members.len())
            .field("owner_vehicle_entities", &self.owner_vehicle_entities.len())
            .field("transaction", &self.transaction.len())
            .field("grounded_transaction", &self.grounded_transaction.len())
            .field("processed", &self.processed.len())
            .field("rejected", &self.rejected.len())
            .field("next_arrow_batch_start", &self.next_arrow_batch_start)
            .finish()
    }
}

impl Default for ArrowTickScratch {
    fn default() -> Self {
        Self {
            candidate_snapshots: Vec::with_capacity(MAX_ARROW_TICK_CANDIDATES),
            player_ids: Vec::with_capacity(MAX_ARROW_TICK_CANDIDATES),
            hits: Vec::with_capacity(MAX_ARROW_TICK_CANDIDATES),
            targets: Vec::with_capacity(MAX_ARROW_TICK_CANDIDATES),
            owner_members: Vec::with_capacity(MAX_OWNER_VEHICLE_MEMBERS),
            owner_vehicle_entities: Vec::with_capacity(MAX_OWNER_VEHICLE_MEMBERS),
            transaction: Vec::with_capacity(MAX_ARROW_TICK_CANDIDATES + 1),
            grounded_transaction: Vec::with_capacity(MAX_ARROW_TICK_CANDIDATES),
            grounded_ids: Vec::with_capacity(MAX_ARROW_TICK_CANDIDATES),
            grounded_discards: Vec::with_capacity(MAX_ARROW_TICK_CANDIDATES),
            grounded_visibility: Vec::with_capacity(MAX_ARROW_TICK_CANDIDATES),
            processed: Vec::with_capacity(MAX_ARROW_TICK_CANDIDATES),
            rejected: Vec::with_capacity(MAX_ARROW_TICK_CANDIDATES),
            next_arrow_batch_start: 0,
        }
    }
}

pub(super) fn initial_arrow_state(
    owner: Option<EntityIdentity>,
    position: Vec3,
    velocity: Vec3,
    rotation: Rotation,
) -> Option<ArrowState> {
    let geometry = entity_geometry("minecraft:arrow", None).aabb;
    let bounds = ProjectileAabb::new(
        position.x - geometry.half_width,
        position.y,
        position.z - geometry.half_width,
        position.x + geometry.half_width,
        position.y + geometry.height,
        position.z + geometry.half_width,
    )
    .ok()?;
    let projectile = ProjectileState::new(
        owner,
        projectile_vec(position),
        bounds,
        projectile_vec(velocity),
        ProjectileRotation::new(rotation.yaw, rotation.pitch),
    )
    .ok()?;
    Some(ArrowState::new(projectile, PickupMode::Allowed, 0))
}

fn rebased_arrow_state(
    snapshot: &EntitySnapshot,
    motion: &EntityMotionState,
    step: &EntityPhysicsStep,
) -> Option<ArrowState> {
    let movement = projectile_vec(step.position).subtract(projectile_vec(motion.position));
    if !movement.is_finite() {
        return None;
    }
    let mut state = snapshot.retained.arrow_state?;
    state.projectile.position = projectile_vec(motion.position);
    state.projectile.velocity = movement;
    state.projectile.rotation = ProjectileRotation::new(motion.rotation.yaw, motion.rotation.pitch);
    let geometry = entity_geometry("minecraft:arrow", None).aabb;
    state.projectile.bounds = ProjectileAabb::new(
        motion.position.x - geometry.half_width,
        motion.position.y,
        motion.position.z - geometry.half_width,
        motion.position.x + geometry.half_width,
        motion.position.y + geometry.height,
        motion.position.z + geometry.half_width,
    )
    .ok()?;
    Some(state)
}

fn prepare_arrow_tick_candidates_locked(
    inner: &SessionEntityGuards<'_>,
    arrow_id: EntityId,
    state: &ArrowState,
    start: Vec3,
    end: Vec3,
    scratch: &mut ArrowTickScratch,
) -> bool {
    scratch.candidate_snapshots.clear();
    scratch.player_ids.clear();
    scratch.hits.clear();
    scratch.targets.clear();
    scratch.owner_members.clear();
    scratch.owner_vehicle_entities.clear();
    let owner = state.projectile.owner;
    if !collect_arrow_entity_candidate_snapshots_locked(
        inner,
        start,
        end,
        &mut scratch.candidate_snapshots,
    ) {
        return false;
    }
    if !prepare_arrow_owner_members_locked(
        inner,
        owner,
        &scratch.candidate_snapshots,
        &mut scratch.owner_members,
        &mut scratch.owner_vehicle_entities,
    ) {
        return false;
    }
    while let Some(entity) = scratch.candidate_snapshots.pop() {
        if Some(projectile_identity(entity.id)) == owner
            || !arrow_entity_is_candidate(arrow_id, &entity)
            || arrow_damage_is_invulnerable(inner, entity.id)
        {
            continue;
        }
        let geometry = entity_geometry(&entity.type_name, entity.animal).aabb;
        let Some(hit_t) = segment_target_aabb_t(start, end, entity.position, geometry) else {
            continue;
        };
        let location = segment_location(start, end, hit_t);
        let entity_id = entity.id;
        let entity_position = entity.position;
        let enderman = entity.type_name == "minecraft:enderman";
        let killed = entity.health <= ARROW_ENTITY_HIT_DAMAGE;
        scratch.targets.push(ArrowTickTarget {
            entity_id,
            entity_location: Some(location),
            expected_entity: Some(entity),
            committed_damage: None,
            player_damage: None,
        });
        scratch.hits.push(ArrowEntityHit {
            entity: projectile_entity(entity_id),
            location: projectile_vec(location),
            entity_position: projectile_vec(entity_position),
            eligibility: projectile_hit_eligibility(
                scratch.owner_vehicle_entities.contains(&entity_id),
            ),
            resolution: ArrowEntityResolution::Damage(ArrowDamageResolution::Accepted {
                enderman,
                living: true,
                killed,
            }),
            input_order: 0,
        });
    }
    for session_id in inner.sessions.keys().copied() {
        if scratch.player_ids.len() == MAX_ARROW_TICK_CANDIDATES {
            return false;
        }
        scratch.player_ids.push(session_id);
    }
    scratch.player_ids.sort_unstable();
    for session_id in scratch.player_ids.iter().copied() {
        let Some(session) = inner.sessions.get(&session_id) else {
            continue;
        };
        if Some(projectile_identity(EntityId(session.entity_id))) == owner {
            continue;
        }
        let Some(hit_t) = segment_target_aabb_t(
            start,
            end,
            player_collision_position(session.pose),
            player_aabb(),
        ) else {
            continue;
        };
        let entity = EntityId(session.entity_id);
        let location = segment_location(start, end, hit_t);
        if scratch.targets.len() == MAX_ARROW_TICK_CANDIDATES {
            return false;
        }
        let preview = prepare_projectile_player_damage_locked(
            inner,
            session_id,
            inner.entity_lifecycle_tick,
            PlayerDamageRequest {
                kind: PlayerDamageKind::Projectile,
                amount: ARROW_ENTITY_HIT_DAMAGE,
                source_origin: Some(start),
            },
        );
        let (resolution, damage) = match preview {
            ProjectilePlayerDamagePreview::Accepted(damage) => {
                let killed = damage.kills_player();
                (
                    ArrowDamageResolution::Accepted {
                        enderman: false,
                        living: true,
                        killed,
                    },
                    Some(damage),
                )
            }
            ProjectilePlayerDamagePreview::Rejected(damage) => (
                ArrowDamageResolution::Rejected {
                    reverse: rejected_player_hit_deflection(start, end),
                },
                damage,
            ),
        };
        scratch.targets.push(ArrowTickTarget {
            entity_id: entity,
            entity_location: None,
            expected_entity: None,
            committed_damage: None,
            player_damage: damage,
        });
        scratch.hits.push(ArrowEntityHit {
            entity: projectile_entity(entity),
            location: projectile_vec(location),
            entity_position: projectile_vec(player_collision_position(session.pose)),
            eligibility: projectile_hit_eligibility(false),
            resolution: ArrowEntityResolution::Damage(resolution),
            input_order: 0,
        });
    }
    scratch
        .hits
        .sort_unstable_by_key(|candidate| candidate.entity);
    scratch
        .targets
        .sort_unstable_by_key(|target| target.entity_id);
    true
}

pub(super) fn projectile_identity(entity: EntityId) -> EntityIdentity {
    EntityIdentity::new(u128::from(entity.0 as u32))
}

fn owner_collision_input<'a>(
    state: &ArrowState,
    members: &'a [OwnerVehicleMember],
) -> OwnerCollisionInput<'a> {
    state
        .projectile
        .owner
        .map_or_else(OwnerCollisionInput::missing, |owner| {
            OwnerCollisionInput::resolved(owner, members)
        })
}

fn prepare_arrow_owner_members_locked(
    inner: &SessionEntityGuards<'_>,
    owner: Option<EntityIdentity>,
    candidate_entities: &[EntitySnapshot],
    members: &mut Vec<OwnerVehicleMember>,
    entities: &mut Vec<EntityId>,
) -> bool {
    let Some(owner) = owner else {
        return true;
    };
    if let Some(session) = inner
        .sessions
        .values()
        .find(|session| projectile_identity(EntityId(session.entity_id)) == owner)
    {
        if let Some(bounds) =
            projectile_bounds(player_collision_position(session.pose), player_aabb())
        {
            members.push(OwnerVehicleMember {
                pickable: true,
                bounds,
            });
        }
        return true;
    }
    let Ok(raw) = i32::try_from(owner.raw()) else {
        return true;
    };
    let owner_id = EntityId(raw);
    let fetched_owner;
    let owner_snapshot = if let Some(snapshot) = candidate_entities
        .iter()
        .find(|snapshot| snapshot.id == owner_id)
    {
        snapshot
    } else {
        let Some(snapshot) = inner.entities.snapshot(owner_id) else {
            return false;
        };
        fetched_owner = snapshot;
        &fetched_owner
    };
    if !push_owner_vehicle_member(owner_snapshot, members, entities) {
        return false;
    }

    let mut mounted = owner_id;
    loop {
        let parent = candidate_entities.iter().find(|candidate| {
            candidate.vehicle.and_then(|vehicle| vehicle.passenger) == Some(mounted)
        });
        let Some(parent) = parent else {
            break;
        };
        if entities.contains(&parent.id) {
            break;
        }
        if !push_owner_vehicle_member(parent, members, entities) {
            return false;
        }
        mounted = parent.id;
    }

    let mut passenger = owner_snapshot.vehicle.and_then(|vehicle| vehicle.passenger);
    loop {
        let Some(passenger_id) = passenger else {
            break;
        };
        if entities.contains(&passenger_id) {
            break;
        }
        let fetched_passenger;
        let passenger_snapshot = if let Some(snapshot) = candidate_entities
            .iter()
            .find(|snapshot| snapshot.id == passenger_id)
        {
            snapshot
        } else {
            let Some(snapshot) = inner.entities.snapshot(passenger_id) else {
                return false;
            };
            fetched_passenger = snapshot;
            &fetched_passenger
        };
        if !push_owner_vehicle_member(passenger_snapshot, members, entities) {
            return false;
        }
        passenger = passenger_snapshot
            .vehicle
            .and_then(|vehicle| vehicle.passenger);
    }
    true
}

fn push_owner_vehicle_member(
    snapshot: &EntitySnapshot,
    members: &mut Vec<OwnerVehicleMember>,
    entities: &mut Vec<EntityId>,
) -> bool {
    if entities.len() == MAX_OWNER_VEHICLE_MEMBERS {
        return false;
    }
    entities.push(snapshot.id);
    let geometry = entity_geometry(&snapshot.type_name, snapshot.animal).aabb;
    if let Some(bounds) = projectile_bounds(snapshot.position, geometry) {
        members.push(OwnerVehicleMember {
            pickable: true,
            bounds,
        });
    }
    true
}

fn projectile_bounds(position: Vec3, geometry: mc_physics::Aabb) -> Option<ProjectileAabb> {
    ProjectileAabb::new(
        position.x - geometry.half_width,
        position.y,
        position.z - geometry.half_width,
        position.x + geometry.half_width,
        position.y + geometry.height,
        position.z + geometry.half_width,
    )
    .ok()
}

fn rejected_player_hit_deflection(start: Vec3, end: Vec3) -> ResolvedDeflection {
    ResolvedDeflection {
        velocity: projectile_vec(Vec3::new(start.x - end.x, start.y - end.y, start.z - end.z)),
        yaw_delta: 180.0,
    }
}

fn projectile_hit_eligibility(shares_owner_vehicle: bool) -> HitEligibility {
    HitEligibility {
        can_be_hit_by_projectile: true,
        arrow_pvp_permitted: true,
        shares_owner_vehicle,
    }
}

fn arrow_damage_is_invulnerable(inner: &SessionEntityGuards<'_>, entity_id: EntityId) -> bool {
    inner
        .entities
        .snapshot(entity_id)
        .and_then(|snapshot| snapshot.retained.last_damage_tick)
        .is_some_and(|last| {
            inner.entity_lifecycle_tick.saturating_sub(last) < ENTITY_HURT_INVULNERABLE_TICKS
        })
}

fn commit_arrow_transaction_locked(
    inner: &mut SessionEntityGuards<'_>,
    expected_arrow: EntitySnapshot,
    next_arrow: EntitySnapshot,
    start: Vec3,
    publications: &mc_entity::projectile_26_1_2::PublicationBatch,
    scratch: &mut ArrowTickScratch,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> Result<bool, ()> {
    scratch.transaction.clear();
    scratch.transaction.push((expected_arrow, next_arrow));
    let mut discard_arrow = false;
    let mut player_damage = None;
    for publication in publications.iter() {
        match publication {
            ProjectilePublication::ArrowDamageAccepted { entity, .. } => {
                let entity = session_entity(entity);
                let target = target_for_entity_mut(&mut scratch.targets, entity).ok_or(())?;
                if let Some(location) = target.entity_location {
                    let expected = target.expected_entity.take().ok_or(())?;
                    let damage = prepare_arrow_entity_damage(
                        &expected,
                        start,
                        location,
                        inner.entity_lifecycle_tick,
                    )
                    .ok_or(())?;
                    scratch
                        .transaction
                        .push((expected, damage.snapshot.clone()));
                    target.committed_damage = Some(damage);
                } else {
                    if player_damage.is_some() {
                        return Err(());
                    }
                    player_damage = Some(target.player_damage.take().ok_or(())?);
                }
            }
            ProjectilePublication::ArrowDamageRejected { entity } => {
                let entity = session_entity(entity);
                if let Some(target) = target_for_entity_mut(&mut scratch.targets, entity)
                    && target.entity_location.is_none()
                    && let Some(prepared) = target.player_damage.take()
                    && player_damage.replace(prepared).is_some()
                {
                    return Err(());
                }
            }
            ProjectilePublication::Discarded { .. } => {
                discard_arrow = true;
            }
            _ => {}
        }
    }

    let committed = if let Some(player_damage) = player_damage {
        commit_projectile_player_damage_locked(
            inner,
            player_damage,
            |inner| {
                inner
                    .entities
                    .replace_snapshots_if_current(scratch.transaction.drain(..))
            },
            dispatches,
        )
    } else {
        inner
            .entities
            .replace_snapshots_if_current(scratch.transaction.drain(..))
    };
    if !committed {
        return Err(());
    }

    publish_committed_arrow_targets_locked(inner, publications, &mut scratch.targets, dispatches);
    Ok(discard_arrow)
}

fn prepare_arrow_entity_damage(
    expected: &EntitySnapshot,
    start: Vec3,
    location: Vec3,
    tick: u64,
) -> Option<EntityDamage> {
    if expected.lifecycle != EntityLifecycle::Alive
        || !expected.health.is_finite()
        || expected.health <= 0.0
    {
        return None;
    }
    let mut next = expected.clone();
    next.health = (next.health - ARROW_ENTITY_HIT_DAMAGE).max(0.0);
    next.retained.last_damage_tick = Some(tick);
    let killed = next.health <= 0.0;
    if killed {
        next.lifecycle = EntityLifecycle::Despawning;
        next.retained.death_remove_tick = Some(tick.saturating_add(ENTITY_DEATH_TICKS));
        next.retained.sheep_grazing_ticks = None;
    } else if let Some(knockback) = arrow_knockback(start, location) {
        next.velocity = Vec3::new(
            next.velocity.x + knockback.x,
            (next.velocity.y + knockback.y).max(knockback.y),
            next.velocity.z + knockback.z,
        );
    }
    Some(EntityDamage {
        snapshot: next,
        killed,
    })
}

fn publish_committed_arrow_targets_locked(
    inner: &mut SessionEntityGuards<'_>,
    publications: &mc_entity::projectile_26_1_2::PublicationBatch,
    targets: &mut [ArrowTickTarget],
    dispatches: &mut Vec<VisibilityDispatch>,
) {
    for publication in publications.iter() {
        let ProjectilePublication::ArrowDamageAccepted { entity, .. } = publication else {
            continue;
        };
        let target = target_for_entity_mut(targets, session_entity(entity))
            .expect("accepted projectile publication was validated before owner commit");
        let Some(damage) = target.committed_damage.take() else {
            continue;
        };
        dispatches.extend(publish_accepted_entity_health_locked(
            inner,
            &damage.snapshot,
        ));
        if damage.killed {
            let rewards =
                EntityKillRewards {
                    items: inner.arrow_kill_rewards.item_entity_type_id.map_or_else(
                        Vec::new,
                        |entity_type_id| {
                            entity_kill_drop_stacks(
                                &inner.arrow_kill_rewards,
                                &damage.snapshot.type_name,
                                damage.snapshot.animal,
                                damage.snapshot.id.0 as i64 as u64,
                            )
                            .into_iter()
                            .map(|drop| (entity_type_id, entity_item_stack(drop)))
                            .collect()
                        },
                    ),
                    experience: inner.arrow_kill_rewards.xp_orb_entity_type_id.map(
                        |entity_type_id| (entity_type_id, mob_xp_value(&damage.snapshot.type_name)),
                    ),
                };
            let (_, target_dispatches) = begin_server_entity_death_locked(inner, &damage, &rewards);
            dispatches.extend(target_dispatches);
        } else {
            publish_arrow_knockback_locked(inner, &damage.snapshot, dispatches);
            dispatches.extend(entity_event_dispatches_locked(inner, damage.snapshot.id, 2));
        }
    }
}

fn target_for_entity_mut(
    targets: &mut [ArrowTickTarget],
    entity: EntityId,
) -> Option<&mut ArrowTickTarget> {
    targets
        .binary_search_by_key(&entity, |target| target.entity_id)
        .ok()
        .map(|index| &mut targets[index])
}

fn arrow_snapshot_with_state(expected: &EntitySnapshot, state: ArrowState) -> EntitySnapshot {
    let velocity = if state.in_ground {
        Vec3::ZERO
    } else {
        session_vec(state.projectile.velocity)
    };
    let rotation = Rotation {
        yaw: state.projectile.rotation.yaw,
        pitch: state.projectile.rotation.pitch,
        head_yaw: state.projectile.rotation.yaw,
    };
    let mut next = expected.clone();
    next.position = session_vec(state.projectile.position);
    next.rotation = rotation;
    next.velocity = velocity;
    next.on_ground = state.in_ground;
    next.retained.arrow_state = Some(state);
    next
}

fn synchronize_arrow_snapshot_locked(inner: &mut SessionEntityGuards<'_>, arrow_id: EntityId) {
    let _ = publish_server_entity_snapshot_locked(inner, arrow_id);
}

fn publish_arrow_knockback_locked(
    inner: &mut SessionEntityGuards<'_>,
    accepted: &EntitySnapshot,
    dispatches: &mut Vec<VisibilityDispatch>,
) {
    if inner.entities.snapshot(accepted.id).as_ref() != Some(accepted) {
        return;
    }
    let Some(snapshot) = publish_server_entity_snapshot_locked(inner, accepted.id) else {
        return;
    };
    for observer_id in visible_entity_observers_locked(inner, accepted.id) {
        if let Some(observer) = inner.sessions.get(&observer_id) {
            dispatches.push(VisibilityDispatch {
                recipient: ordered_session_recipient(observer_id, observer),
                command: OutboundCommand::MoveEntityRelative(ServerEntityMove {
                    id: accepted.id,
                    position: snapshot.position,
                    wire_move: None,
                    velocity: snapshot.velocity,
                    rotation: snapshot.rotation,
                    on_ground: snapshot.on_ground,
                    send_velocity: true,
                    send_head_rotation: false,
                }),
            });
        }
    }
}

fn arrow_knockback(start: Vec3, end: Vec3) -> Option<Vec3> {
    let dx = end.x - start.x;
    let dz = end.z - start.z;
    let horizontal = dx.hypot(dz);
    if horizontal <= f64::EPSILON {
        return None;
    }
    Some(Vec3::new(
        dx / horizontal * ARROW_ENTITY_HIT_KNOCKBACK,
        0.1,
        dz / horizontal * ARROW_ENTITY_HIT_KNOCKBACK,
    ))
}

const ARROW_HIT_EXPANSION: f64 = 0.25;

#[cfg(test)]
pub(super) fn arrow_entity_candidate_snapshots_locked(
    inner: &SessionEntityGuards<'_>,
    start: Vec3,
    end: Vec3,
) -> Vec<EntitySnapshot> {
    let mut snapshots = Vec::with_capacity(MAX_ARROW_TICK_CANDIDATES);
    let _ = collect_arrow_entity_candidate_snapshots_locked(inner, start, end, &mut snapshots);
    snapshots
}

fn collect_arrow_entity_candidate_snapshots_locked(
    inner: &SessionEntityGuards<'_>,
    start: Vec3,
    end: Vec3,
    snapshots: &mut Vec<EntitySnapshot>,
) -> bool {
    snapshots.clear();
    if [start.x, start.z, end.x, end.z]
        .into_iter()
        .any(|coordinate| !coordinate.is_finite())
    {
        return true;
    }
    let max_half_width = inner
        .entity_type_aabbs
        .values()
        .map(|aabb| aabb.half_width)
        .filter(|half_width| half_width.is_finite() && *half_width >= 0.0)
        .fold(0.0_f64, f64::max);
    let padding = max_half_width + ARROW_HIT_EXPANSION;
    let (min_cx, min_cz) =
        chunk_pos_from_coords(start.x.min(end.x) - padding, start.z.min(end.z) - padding);
    let (max_cx, max_cz) =
        chunk_pos_from_coords(start.x.max(end.x) + padding, start.z.max(end.z) + padding);
    let chunk_count = u64::from(min_cx.abs_diff(max_cx))
        .saturating_add(1)
        .saturating_mul(u64::from(min_cz.abs_diff(max_cz)).saturating_add(1));
    if chunk_count >= inner.entities_by_chunk.len() as u64 {
        for id in inner.entity_chunks.keys().copied() {
            if !push_arrow_candidate_snapshot(inner, snapshots, id) {
                return false;
            }
        }
        snapshots.sort_unstable_by_key(|snapshot| snapshot.id);
        return true;
    }

    for cz in min_cz..=max_cz {
        for cx in min_cx..=max_cx {
            if let Some(chunk_ids) = inner.entities_by_chunk.get(&(cx, cz)) {
                for id in chunk_ids.iter().copied() {
                    if !push_arrow_candidate_snapshot(inner, snapshots, id) {
                        return false;
                    }
                }
            }
        }
    }
    snapshots.sort_unstable_by_key(|snapshot| snapshot.id);
    true
}

fn push_arrow_candidate_snapshot(
    inner: &SessionEntityGuards<'_>,
    snapshots: &mut Vec<EntitySnapshot>,
    id: EntityId,
) -> bool {
    if snapshots.iter().any(|snapshot| snapshot.id == id) {
        return true;
    }
    let Some(snapshot) = inner.entities.snapshot(id) else {
        return true;
    };
    if snapshots.len() == MAX_ARROW_TICK_CANDIDATES {
        return false;
    }
    snapshots.push(snapshot);
    true
}

fn arrow_entity_is_candidate(arrow_id: EntityId, entity: &EntitySnapshot) -> bool {
    entity.id != arrow_id
        && entity.lifecycle == EntityLifecycle::Alive
        && entity.type_name != "minecraft:arrow"
        && entity.item_stack.is_none()
        && entity.experience_value.is_none()
        && entity.block_state.is_none()
}

fn segment_target_aabb_t(
    start: Vec3,
    end: Vec3,
    target_position: Vec3,
    target_aabb: mc_physics::Aabb,
) -> Option<f64> {
    let min = Vec3::new(
        target_position.x - target_aabb.half_width - ARROW_HIT_EXPANSION,
        target_position.y - ARROW_HIT_EXPANSION,
        target_position.z - target_aabb.half_width - ARROW_HIT_EXPANSION,
    );
    let max = Vec3::new(
        target_position.x + target_aabb.half_width + ARROW_HIT_EXPANSION,
        target_position.y + target_aabb.height + ARROW_HIT_EXPANSION,
        target_position.z + target_aabb.half_width + ARROW_HIT_EXPANSION,
    );
    segment_aabb_intersection_t(start, end, min, max)
}

pub(super) fn segment_aabb_intersection_t(
    start: Vec3,
    end: Vec3,
    min: Vec3,
    max: Vec3,
) -> Option<f64> {
    if [
        start.x, start.y, start.z, end.x, end.y, end.z, min.x, min.y, min.z, max.x, max.y, max.z,
    ]
    .into_iter()
    .any(|coordinate| !coordinate.is_finite())
        || min.x > max.x
        || min.y > max.y
        || min.z > max.z
    {
        return None;
    }
    let delta = Vec3::new(end.x - start.x, end.y - start.y, end.z - start.z);
    if [delta.x, delta.y, delta.z]
        .into_iter()
        .any(|coordinate| !coordinate.is_finite())
    {
        return None;
    }
    let mut t_min: f64 = 0.0;
    let mut t_max: f64 = 1.0;
    for (origin, direction, min_axis, max_axis) in [
        (start.x, delta.x, min.x, max.x),
        (start.y, delta.y, min.y, max.y),
        (start.z, delta.z, min.z, max.z),
    ] {
        if direction.abs() <= f64::EPSILON {
            if origin < min_axis || origin > max_axis {
                return None;
            }
            continue;
        }
        let inv_direction = 1.0 / direction;
        let mut near = (min_axis - origin) * inv_direction;
        let mut far = (max_axis - origin) * inv_direction;
        if near > far {
            std::mem::swap(&mut near, &mut far);
        }
        t_min = t_min.max(near);
        t_max = t_max.min(far);
        if t_min > t_max {
            return None;
        }
    }
    Some(t_min.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use mc_entity::{Rotation, Vec3};

    use crate::play::SessionRegistry;

    use super::spawn_arrow_locked;

    #[test]
    fn arrow_spawn_installs_required_retained_state_in_one_ecs_transaction() {
        let registry = SessionRegistry::new();
        registry.reset_entity_owner_requests_for_test();

        let arrow_id = {
            let mut inner = registry.lock_session_entities("spawn atomic arrow test");
            spawn_arrow_locked(
                &mut inner,
                None,
                1,
                Vec3::new(0.5, 64.0, 0.5),
                Vec3::new(0.1, 0.2, 0.3),
                Rotation::ZERO,
            )
            .0
        };

        assert_eq!(registry.entity_owner_requests_for_test(), 2);
        let snapshot = registry
            .lock_entities("read atomic arrow test")
            .snapshot(arrow_id)
            .expect("spawned arrow remains authoritative");
        assert_eq!(snapshot.retained.spawn_tick, 0);
        assert!(snapshot.retained.arrow_state.is_some());
    }
}
