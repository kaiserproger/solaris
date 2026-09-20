use std::collections::{HashMap, HashSet};

use mc_entity::projectile_26_1_2::{
    Aabb as ProjectileAabb, ArrowBlockHit, ArrowDamageResolution, ArrowEntityHit,
    ArrowEntityResolution, ArrowState, ArrowTickInput, BlockHit as ProjectileBlockHit,
    BlockStateId as ProjectileBlockStateId, EntityHitResolution, EntityId as ProjectileEntityId,
    EntityIdentity, HURTING_PROJECTILE_DEFAULT_ACCELERATION_POWER, HitEligibility, HitTarget,
    HurtingProjectileState, HurtingProjectileTickInput, InputStamp, MAX_PIERCED_ENTITIES,
    OwnerCollisionInput, OwnerVehicleMember, PickupMode, ProjectileLifecycle,
    ProjectilePublication, ProjectileState, ResolvedDeflection, Rotation as ProjectileRotation,
    ThrowableEntityHit, ThrowableState, ThrowableTickInput, Vec3 as ProjectileVec3,
    commit_arrow_tick, commit_hurting_projectile_tick, commit_throwable_tick, prepare_arrow_tick,
    prepare_hurting_projectile_tick, prepare_throwable_tick,
};
use mc_entity::{
    EntityDamage, EntityDamageRequest, EntityId, EntityLifecycle, EntityMotionState,
    EntitySnapshot, EntityWitchPotionKind, Rotation, SpawnEntity, Vec3,
};

use crate::play::combat::{PlayerDamageKind, PlayerDamageRequest};
use crate::play::spawn::chunk_pos_from_coords;
use crate::play::survival::{entity_item_stack, mob_xp_value};
use crate::play::{
    ARROW_ENTITY_HIT_DAMAGE, ARROW_ENTITY_HIT_KNOCKBACK, ArrowPhysicsFact, EntityPhysicsStep,
    HurtingProjectilePhysicsFact, SessionRegistry,
};

use super::damage_precommit::{
    EntityDamagePrecommitCompletion, EntityDamagePrecommitResume, PlayerDamagePrecommitResume,
    begin_entity_damage, damage_kind_name,
};
use super::entity_combat::{
    begin_server_entity_death_locked, publish_accepted_entity_health_locked,
};
use super::entity_lifecycle::{remove_server_entity_locked, track_entity_chunk_locked};
use super::explosion_authority::schedule_primed_tnt_deadline_locked;
use super::interaction_geometry::{entity_aabb, entity_geometry};
use super::outbound::{OutboundCommand, ServerEntityMove, VisibilityDispatch};
use super::player_combat::{
    PreparedProjectilePlayerDamage, ProjectilePlayerDamagePreview,
    commit_projectile_player_damage_locked, prepare_projectile_player_damage_locked,
};
use super::player_effects::{
    SLOWNESS_EFFECT_ID, WEAKNESS_EFFECT_ID, apply_player_effect_locked, caller_owned_effect,
    poison_effect,
};
use super::visibility::{
    entity_hurt_dispatches_locked, initialize_entity_wire_state_locked, ordered_session_recipient,
    publish_server_entity_snapshot_locked, spawn_entity_visibility_locked,
    visible_entity_observers_locked,
};
use super::{
    ENTITY_DEATH_TICKS, ENTITY_HURT_INVULNERABLE_TICKS, EntityKillRewards, SessionEntityGuards,
    SessionId, apply_entity_facts, entity_kill_drop_stacks, player_aabb, player_collision_position,
};
use mc_script::ScriptPosition;
use mc_script::precommit::{
    Approval, DamageContext, DamageTarget, HookActor, HookContext, HookDecision, HookFailure,
    HookKind, HookPlayer,
};

const MAX_ARROW_TICK_CANDIDATES: usize = MAX_PIERCED_ENTITIES + 1;
const MAX_OWNER_VEHICLE_MEMBERS: usize = 8;
const SMALL_FIREBALL_ENTITY_DAMAGE: f32 = 5.0;
const LARGE_FIREBALL_ENTITY_DAMAGE: f32 = 6.0;
const SHULKER_BULLET_ENTITY_DAMAGE: f32 = 4.0;
const BREEZE_WIND_CHARGE_ENTITY_DAMAGE: f32 = 1.0;
const WITHER_SKULL_ENTITY_DAMAGE: f32 = 8.0;
const WITCH_HARMING_DAMAGE: f32 = 6.0;
const WITCH_SPLASH_POTION_GRAVITY: f64 = 0.05;

/// Per-type throwable gravity. The witch's splash potions fly on 0.05; every
/// other throwable (snowball today) uses the vanilla ThrowableProjectile
/// default of 0.03. Keep in lockstep with the physics lane's per-type gravity
/// in mc-entity/runtime.rs `entity_physics_kind`.
fn throwable_gravity(type_name: &str) -> f64 {
    if type_name == "minecraft:splash_potion" {
        WITCH_SPLASH_POTION_GRAVITY
    } else {
        mc_entity::projectile_26_1_2::THROWABLE_DEFAULT_GRAVITY
    }
}

const WITCH_SLOWNESS_DURATION_TICKS: i32 = 1_800;
const WITCH_POISON_DURATION_TICKS: i32 = 900;
const WITCH_WEAKNESS_DURATION_TICKS: i32 = 1_800;
const SHULKER_BULLET_LEVITATION_EFFECT_ID: u32 = 24;
const SHULKER_BULLET_LEVITATION_TICKS: i32 = 200;

#[derive(Debug, Clone, Copy)]
struct PlainProjectileDamage {
    kind: PlayerDamageKind,
    amount: f32,
}

struct ProjectileEntityHit {
    expected_projectile: EntitySnapshot,
    next_projectile: EntitySnapshot,
    expected_target: EntitySnapshot,
    amount: f32,
}

struct ProjectilePlayerHit {
    expected_projectile: EntitySnapshot,
    next_projectile: EntitySnapshot,
    target_session: SessionId,
    source_origin: Vec3,
}

/// Immutable projectile-side state that must be conditionally committed with a
/// deferred player-damage approval.
#[derive(Debug, Clone)]
pub(in crate::play) enum ProjectileDamageContinuation {
    Hit {
        expected_projectile: EntitySnapshot,
        next_projectile: EntitySnapshot,
    },
}

/// The immutable native impact that a before-damage approval must re-enter.
///
/// Both entity images are fences: an answer for an older projectile flight or
/// target state must not be allowed to publish a later impact.
#[derive(Debug, Clone)]
pub(in crate::play) struct ProjectileEntityDamageContinuation {
    expected_projectile: EntitySnapshot,
    next_projectile: EntitySnapshot,
    expected_target: EntitySnapshot,
    impact: ProjectileEntityDamageImpact,
}

#[derive(Debug, Clone, Copy)]
enum ProjectileEntityDamageImpact {
    Arrow {
        start: Vec3,
        location: Vec3,
        discard: bool,
    },
    Plain,
    SmallFireball,
    ShulkerBullet,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectileDamageAdmission {
    Direct,
    Pending,
    Refused,
}

fn pending_projectile_admission_locked(
    inner: &mut SessionEntityGuards<'_>,
    projectile: EntityId,
) -> Option<ProjectileDamageAdmission> {
    if let Some(deadline) = inner.pending_projectile_damage.get(&projectile) {
        if std::time::Instant::now() < *deadline {
            return Some(ProjectileDamageAdmission::Pending);
        }
        // A dropped resume must not leave this collision frozen indefinitely.
        inner.pending_projectile_damage.remove(&projectile);
        return Some(ProjectileDamageAdmission::Refused);
    }
    (inner.pending_projectile_damage.len() >= mc_script::precommit::MAX_PRECOMMIT_REQUESTS)
        .then_some(ProjectileDamageAdmission::Refused)
}

#[derive(Debug, Clone, Copy)]
pub(super) struct HurtingProjectileMotionProfile {
    pub acceleration_power: f64,
    pub air_inertia: f64,
    pub water_inertia: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HurtingProjectileKind {
    SmallFireball,
    LargeFireball,
    ShulkerBullet,
    BreezeWindCharge,
    WitherSkull,
    DragonFireball,
}

impl HurtingProjectileKind {
    fn from_type_name(type_name: &str) -> Option<Self> {
        match type_name {
            "minecraft:small_fireball" => Some(Self::SmallFireball),
            "minecraft:fireball" => Some(Self::LargeFireball),
            "minecraft:shulker_bullet" => Some(Self::ShulkerBullet),
            "minecraft:breeze_wind_charge" => Some(Self::BreezeWindCharge),
            "minecraft:wither_skull" => Some(Self::WitherSkull),
            "minecraft:dragon_fireball" => Some(Self::DragonFireball),
            _ => None,
        }
    }
}

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

/// Spawns a player-thrown throwable item projectile (snowball today) with the
/// same wire/visibility fanout as arrows.
pub(super) fn spawn_throwable_projectile_locked(
    inner: &mut SessionEntityGuards<'_>,
    owner_session: Option<SessionId>,
    entity_type_id: i32,
    type_name: &str,
    position: Vec3,
    velocity: Vec3,
    rotation: Rotation,
) -> (EntityId, Vec<VisibilityDispatch>) {
    let owner = owner_session
        .and_then(|session_id| inner.sessions.get(&session_id))
        .map(|session| projectile_identity(EntityId(session.entity_id)));
    let mut entity = SpawnEntity::new(entity_type_id, type_name, position);
    entity.velocity = velocity;
    entity.rotation = rotation;
    entity.on_ground = false;
    apply_entity_facts(&mut entity);
    entity.retained.spawn_tick = inner.entity_lifecycle_tick;
    entity.retained.throwable_projectile_state = Some(
        initial_throwable_projectile_state(owner, type_name, position, velocity, rotation)
            .expect("finite spawned throwable must produce a valid kernel state"),
    );
    let aabb = entity_aabb(type_name);
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

pub(super) fn resolve_hurting_projectile_hits_locked<'a>(
    registry: &'a super::SessionRegistry,
    mut inner: SessionEntityGuards<'a>,
    steps: &[EntityPhysicsStep],
    old_motion: &HashMap<EntityId, EntityMotionState>,
    physics_facts: &[HurtingProjectilePhysicsFact],
    dragon_cloud_entity_type_id: Option<i32>,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> (SessionEntityGuards<'a>, Vec<EntityPhysicsStep>) {
    let mut accepted_steps = Vec::new();
    let mut scratch = HurtingProjectileTickScratch::default();
    for step in steps {
        let Some(motion) = old_motion.get(&step.id) else {
            continue;
        };
        if !motion.is_hurting_projectile {
            continue;
        }
        let Some(fact) = physics_facts
            .iter()
            .find(|fact| fact.projectile_id == step.id)
        else {
            continue;
        };
        let Some(expected) = inner.entities.snapshot(step.id) else {
            continue;
        };
        if expected.position != motion.position
            || expected.velocity != motion.velocity
            || expected.on_ground != motion.on_ground
        {
            continue;
        }
        let Some(projectile_kind) = HurtingProjectileKind::from_type_name(&expected.type_name)
        else {
            continue;
        };
        let Some(state) = expected.retained.hurting_projectile_state else {
            continue;
        };
        if state.projectile.revision != motion.hurting_projectile_revision.unwrap_or(u64::MAX) {
            continue;
        }
        let start = motion.position;
        let end = step.position;
        if !prepare_common_projectile_candidates_locked(
            &inner,
            step.id,
            &state.projectile,
            start,
            end,
            &mut scratch,
        ) {
            continue;
        }
        let block_hit = fact.block_hit.map(|hit| ProjectileBlockHit {
            block_state: ProjectileBlockStateId::new(hit.block_state.0),
            location: projectile_vec(hit.location),
        });
        let stamp = InputStamp {
            world_revision: inner.entity_lifecycle_tick,
            collision_revision: inner.entity_lifecycle_tick,
            resolution_revision: inner.entity_lifecycle_tick,
        };
        let owner_collision = state
            .projectile
            .owner
            .map_or_else(OwnerCollisionInput::missing, |owner| {
                OwnerCollisionInput::resolved(owner, &scratch.owner_members)
            });
        let mut next_state = state;
        let outcome = prepare_hurting_projectile_tick(
            &next_state,
            HurtingProjectileTickInput {
                stamp,
                in_water: fact.in_water,
                owner_collision,
                block_hit,
                entity_hits: &mut scratch.hits,
            },
        )
        .ok()
        .and_then(|plan| commit_hurting_projectile_tick(&mut next_state, stamp, plan).ok());
        let Some(outcome) = outcome else {
            continue;
        };
        let selected_target = match outcome.hit {
            HitTarget::Entity { entity, .. } => scratch
                .targets
                .iter()
                .find(|target| projectile_entity(target.entity_id()) == entity)
                .cloned(),
            _ => None,
        };
        let hit = !matches!(outcome.hit, HitTarget::Miss);
        let mut next_projectile = hurting_projectile_snapshot_with_state(&expected, next_state);
        if hit && let Some(mut pending) = next_projectile.retained.pending_explosion {
            pending.expires_tick = inner.entity_lifecycle_tick;
            next_projectile.retained.pending_explosion = Some(pending);
        }
        let accepted_step = EntityPhysicsStep {
            id: step.id,
            position: next_projectile.position,
            velocity: next_projectile.velocity,
            on_ground: false,
            horizontal_collision: false,
        };
        let has_pending_explosion = next_projectile.retained.pending_explosion.is_some();
        let committed = match (outcome.hit, selected_target) {
            (
                HitTarget::Entity { entity, .. },
                Some(HurtingProjectileTarget::Entity(entity_id)),
            ) if entity == projectile_entity(entity_id) => {
                let Some(candidate_index) = scratch
                    .candidates
                    .iter()
                    .position(|snapshot| snapshot.id == entity_id)
                else {
                    continue;
                };
                let snapshot = scratch.candidates.swap_remove(candidate_index);
                match projectile_kind {
                    HurtingProjectileKind::SmallFireball => commit_projectile_entity_hit_locked(
                        registry,
                        &mut inner,
                        ProjectileEntityHit {
                            expected_projectile: expected,
                            next_projectile,
                            expected_target: snapshot,
                            amount: SMALL_FIREBALL_ENTITY_DAMAGE,
                        },
                        ProjectileEntityDamageImpact::SmallFireball,
                        dispatches,
                    ),
                    HurtingProjectileKind::ShulkerBullet => commit_projectile_entity_hit_locked(
                        registry,
                        &mut inner,
                        ProjectileEntityHit {
                            expected_projectile: expected,
                            next_projectile,
                            expected_target: snapshot,
                            amount: SHULKER_BULLET_ENTITY_DAMAGE,
                        },
                        ProjectileEntityDamageImpact::ShulkerBullet,
                        dispatches,
                    ),
                    HurtingProjectileKind::LargeFireball => commit_projectile_entity_hit_locked(
                        registry,
                        &mut inner,
                        ProjectileEntityHit {
                            expected_projectile: expected,
                            next_projectile,
                            expected_target: snapshot,
                            amount: LARGE_FIREBALL_ENTITY_DAMAGE,
                        },
                        ProjectileEntityDamageImpact::Plain,
                        dispatches,
                    ),
                    HurtingProjectileKind::BreezeWindCharge => commit_projectile_entity_hit_locked(
                        registry,
                        &mut inner,
                        ProjectileEntityHit {
                            expected_projectile: expected,
                            next_projectile,
                            expected_target: snapshot,
                            amount: BREEZE_WIND_CHARGE_ENTITY_DAMAGE,
                        },
                        ProjectileEntityDamageImpact::Plain,
                        dispatches,
                    ),
                    HurtingProjectileKind::WitherSkull => commit_projectile_entity_hit_locked(
                        registry,
                        &mut inner,
                        ProjectileEntityHit {
                            expected_projectile: expected,
                            next_projectile,
                            expected_target: snapshot,
                            amount: WITHER_SKULL_ENTITY_DAMAGE,
                        },
                        ProjectileEntityDamageImpact::Plain,
                        dispatches,
                    ),
                    HurtingProjectileKind::DragonFireball => commit_dragon_fireball_hit_locked(
                        &mut inner,
                        expected,
                        next_projectile,
                        dragon_cloud_entity_type_id,
                        dispatches,
                    ),
                }
            }
            (
                HitTarget::Entity { entity, .. },
                Some(HurtingProjectileTarget::Player {
                    session,
                    entity: player_entity,
                }),
            ) if entity == projectile_entity(player_entity) => match projectile_kind {
                HurtingProjectileKind::SmallFireball => commit_small_fireball_player_hit_locked(
                    registry,
                    &mut inner,
                    expected,
                    next_projectile,
                    session,
                    start,
                    dispatches,
                ),
                HurtingProjectileKind::ShulkerBullet => commit_shulker_bullet_player_hit_locked(
                    registry,
                    &mut inner,
                    expected,
                    next_projectile,
                    session,
                    start,
                    dispatches,
                ),
                HurtingProjectileKind::LargeFireball => {
                    commit_plain_hurting_projectile_player_hit_locked(
                        registry,
                        &mut inner,
                        ProjectilePlayerHit {
                            expected_projectile: expected,
                            next_projectile,
                            target_session: session,
                            source_origin: start,
                        },
                        PlainProjectileDamage {
                            kind: PlayerDamageKind::LargeFireball,
                            amount: LARGE_FIREBALL_ENTITY_DAMAGE,
                        },
                        dispatches,
                    )
                }
                HurtingProjectileKind::BreezeWindCharge => {
                    commit_plain_hurting_projectile_player_hit_locked(
                        registry,
                        &mut inner,
                        ProjectilePlayerHit {
                            expected_projectile: expected,
                            next_projectile,
                            target_session: session,
                            source_origin: start,
                        },
                        PlainProjectileDamage {
                            kind: PlayerDamageKind::WindCharge,
                            amount: BREEZE_WIND_CHARGE_ENTITY_DAMAGE,
                        },
                        dispatches,
                    )
                }
                HurtingProjectileKind::WitherSkull => {
                    commit_plain_hurting_projectile_player_hit_locked(
                        registry,
                        &mut inner,
                        ProjectilePlayerHit {
                            expected_projectile: expected,
                            next_projectile,
                            target_session: session,
                            source_origin: start,
                        },
                        PlainProjectileDamage {
                            kind: PlayerDamageKind::Projectile,
                            amount: WITHER_SKULL_ENTITY_DAMAGE,
                        },
                        dispatches,
                    )
                }
                HurtingProjectileKind::DragonFireball => commit_dragon_fireball_hit_locked(
                    &mut inner,
                    expected,
                    next_projectile,
                    dragon_cloud_entity_type_id,
                    dispatches,
                ),
            },
            (HitTarget::Block { .. }, _)
                if projectile_kind == HurtingProjectileKind::DragonFireball =>
            {
                commit_dragon_fireball_hit_locked(
                    &mut inner,
                    expected,
                    next_projectile,
                    dragon_cloud_entity_type_id,
                    dispatches,
                )
            }
            (HitTarget::Block { .. }, _) | (HitTarget::Miss, _) => inner
                .entities
                .replace_snapshot_if_current(expected, next_projectile),
            _ => false,
        };
        if !committed {
            continue;
        }
        if hit {
            if has_pending_explosion {
                let current_tick = inner.entity_lifecycle_tick;
                schedule_primed_tnt_deadline_locked(&mut inner, step.id, Some(current_tick));
            } else if let Some((_, removed)) = remove_server_entity_locked(&mut inner, step.id) {
                dispatches.extend(removed);
            }
            continue;
        }
        let _ = publish_server_entity_snapshot_locked(&mut inner, step.id);
        accepted_steps.push(accepted_step);
    }
    (inner, accepted_steps)
}

pub(super) fn resolve_throwable_projectile_hits_locked<'a>(
    registry: &'a super::SessionRegistry,
    mut inner: SessionEntityGuards<'a>,
    steps: &[EntityPhysicsStep],
    old_motion: &HashMap<EntityId, EntityMotionState>,
    physics_facts: &[HurtingProjectilePhysicsFact],
    dispatches: &mut Vec<VisibilityDispatch>,
) -> (SessionEntityGuards<'a>, Vec<EntityPhysicsStep>) {
    let mut accepted_steps = Vec::new();
    let mut scratch = HurtingProjectileTickScratch::default();
    for step in steps {
        let Some(motion) = old_motion.get(&step.id) else {
            continue;
        };
        if !motion.is_throwable_projectile {
            continue;
        }
        let Some(fact) = physics_facts
            .iter()
            .find(|fact| fact.projectile_id == step.id)
        else {
            continue;
        };
        let Some(expected) = inner.entities.snapshot(step.id) else {
            continue;
        };
        if expected.position != motion.position
            || expected.velocity != motion.velocity
            || expected.on_ground != motion.on_ground
        {
            continue;
        }
        // The witch is the only mob that throws splash potions; player-thrown
        // snowballs ride the same kernel with their own gravity and no potion
        // payload on impact.
        if !matches!(
            expected.type_name.as_str(),
            "minecraft:splash_potion" | "minecraft:snowball"
        ) {
            continue;
        }
        let Some(state) = expected.retained.throwable_projectile_state else {
            continue;
        };
        if state.projectile.revision != motion.throwable_projectile_revision.unwrap_or(u64::MAX) {
            continue;
        }
        let start = motion.position;
        let end = step.position;
        if !prepare_common_projectile_candidates_locked(
            &inner,
            step.id,
            &state.projectile,
            start,
            end,
            &mut scratch,
        ) {
            continue;
        }
        let block_hit = fact.block_hit.map(|hit| ProjectileBlockHit {
            block_state: ProjectileBlockStateId::new(hit.block_state.0),
            location: projectile_vec(hit.location),
        });
        let stamp = InputStamp {
            world_revision: inner.entity_lifecycle_tick,
            collision_revision: inner.entity_lifecycle_tick,
            resolution_revision: inner.entity_lifecycle_tick,
        };
        let owner_collision = state
            .projectile
            .owner
            .map_or_else(OwnerCollisionInput::missing, |owner| {
                OwnerCollisionInput::resolved(owner, &scratch.owner_members)
            });
        let mut next_state = state;
        let outcome = prepare_throwable_tick(
            &next_state,
            ThrowableTickInput {
                stamp,
                gravity: throwable_gravity(&expected.type_name),
                no_gravity: false,
                in_water: fact.in_water,
                owner_collision,
                block_hit,
                entity_hits: &mut scratch.hits,
            },
        )
        .ok()
        .and_then(|plan| commit_throwable_tick(&mut next_state, stamp, plan).ok());
        let Some(outcome) = outcome else {
            continue;
        };
        let mc_entity::projectile_26_1_2::ThrowableTickMutation::Flight { hit, .. } =
            outcome.mutation;
        let selected_target = match hit {
            HitTarget::Entity { entity, .. } => scratch
                .targets
                .iter()
                .find(|target| projectile_entity(target.entity_id()) == entity)
                .copied(),
            _ => None,
        };
        let next_projectile = throwable_projectile_snapshot_with_state(&expected, next_state);
        let potion_kind = expected
            .retained
            .witch_potion
            .unwrap_or(EntityWitchPotionKind::Harming);
        let hit_location = match hit {
            HitTarget::Entity { location, .. } | HitTarget::Block { location, .. } => {
                Some(session_vec(location))
            }
            HitTarget::Miss => None,
        };
        let did_hit = hit_location.is_some();
        let accepted_step = EntityPhysicsStep {
            id: step.id,
            position: next_projectile.position,
            velocity: next_projectile.velocity,
            on_ground: false,
            horizontal_collision: false,
        };
        let committed = if expected.type_name == "minecraft:snowball" {
            // Snowballs carry no potion payload: advancing the kernel state
            // is all a tick commits; the shared did-hit removal below
            // despawns the snowball on any impact.
            inner
                .entities
                .replace_snapshot_if_current(expected, next_projectile)
        } else {
            match (hit, selected_target) {
                (
                    HitTarget::Entity { entity, .. },
                    Some(HurtingProjectileTarget::Entity(entity_id)),
                ) if entity == projectile_entity(entity_id) => {
                    let Some(candidate_index) = scratch
                        .candidates
                        .iter()
                        .position(|snapshot| snapshot.id == entity_id)
                    else {
                        continue;
                    };
                    let target = scratch.candidates.swap_remove(candidate_index);
                    commit_witch_potion_entity_hit_locked(
                        registry,
                        &mut inner,
                        expected,
                        next_projectile,
                        target,
                        potion_kind,
                        dispatches,
                    )
                }
                (
                    HitTarget::Entity { entity, .. },
                    Some(HurtingProjectileTarget::Player {
                        session,
                        entity: player_entity,
                    }),
                ) if entity == projectile_entity(player_entity) => {
                    commit_witch_potion_player_hit_locked(
                        registry,
                        &mut inner,
                        ProjectilePlayerHit {
                            expected_projectile: expected,
                            next_projectile,
                            target_session: session,
                            source_origin: start,
                        },
                        potion_kind,
                        dispatches,
                    )
                }
                (HitTarget::Block { .. }, _) | (HitTarget::Miss, _) => inner
                    .entities
                    .replace_snapshot_if_current(expected, next_projectile),
                _ => false,
            }
        };
        if !committed {
            continue;
        }
        if did_hit {
            if potion_kind != EntityWitchPotionKind::Harming
                && let Some(hit_location) = hit_location
            {
                apply_witch_status_splash_players_locked(
                    &mut inner,
                    hit_location,
                    potion_kind,
                    dispatches,
                );
            }
            if let Some((_, removed)) = remove_server_entity_locked(&mut inner, step.id) {
                dispatches.extend(removed);
            }
            continue;
        }
        let _ = publish_server_entity_snapshot_locked(&mut inner, step.id);
        accepted_steps.push(accepted_step);
    }
    (inner, accepted_steps)
}

fn throwable_projectile_snapshot_with_state(
    expected: &EntitySnapshot,
    state: ThrowableState,
) -> EntitySnapshot {
    let mut next = expected.clone();
    next.position = session_vec(state.projectile.position);
    next.rotation = Rotation {
        yaw: state.projectile.rotation.yaw,
        pitch: state.projectile.rotation.pitch,
        head_yaw: state.projectile.rotation.yaw,
    };
    next.velocity = session_vec(state.projectile.velocity);
    next.on_ground = false;
    next.retained.throwable_projectile_state = Some(state);
    next
}

fn commit_witch_potion_entity_hit_locked(
    registry: &super::SessionRegistry,
    inner: &mut SessionEntityGuards<'_>,
    expected_projectile: EntitySnapshot,
    next_projectile: EntitySnapshot,
    expected_target: EntitySnapshot,
    potion: EntityWitchPotionKind,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> bool {
    if potion == EntityWitchPotionKind::Harming {
        return commit_projectile_entity_hit_locked(
            registry,
            inner,
            ProjectileEntityHit {
                expected_projectile,
                next_projectile,
                expected_target,
                amount: WITCH_HARMING_DAMAGE,
            },
            ProjectileEntityDamageImpact::Plain,
            dispatches,
        );
    }
    inner
        .entities
        .replace_snapshot_if_current(expected_projectile, next_projectile)
}

fn apply_witch_status_splash_players_locked(
    inner: &mut SessionEntityGuards<'_>,
    hit: Vec3,
    potion: EntityWitchPotionKind,
    dispatches: &mut Vec<VisibilityDispatch>,
) {
    let player_shape = player_aabb();
    let candidates = inner
        .sessions
        .iter()
        .filter_map(|(&session_id, session)| {
            if inner.dead_sessions.contains(&session_id)
                || inner.spectator_sessions.contains(&session_id)
                || inner.client_unloaded_sessions.contains(&session_id)
            {
                return None;
            }
            let feet = player_collision_position(session.pose);
            if (feet.x - hit.x).abs() > 4.5
                || (feet.z - hit.z).abs() > 4.5
                || feet.y > hit.y + 2.5
                || feet.y + player_shape.height < hit.y - 2.5
            {
                return None;
            }
            let axis_distance = |value: f64, min: f64, max: f64| {
                if value < min {
                    min - value
                } else if value > max {
                    value - max
                } else {
                    0.0
                }
            };
            let dx = axis_distance(
                hit.x,
                feet.x - player_shape.half_width,
                feet.x + player_shape.half_width,
            );
            let dy = axis_distance(hit.y, feet.y, feet.y + player_shape.height);
            let dz = axis_distance(
                hit.z,
                feet.z - player_shape.half_width,
                feet.z + player_shape.half_width,
            );
            let distance_sq = dx * dx + dy * dy + dz * dz;
            (distance_sq < 16.0).then_some((session_id, distance_sq))
        })
        .collect::<Vec<_>>();
    for (session_id, distance_sq) in candidates {
        let scale = (1.0 - distance_sq.sqrt() / 4.0).clamp(0.0, 1.0);
        let (effect_id, base_duration, poison) = match potion {
            EntityWitchPotionKind::Slowness => {
                (SLOWNESS_EFFECT_ID, WITCH_SLOWNESS_DURATION_TICKS, false)
            }
            EntityWitchPotionKind::Poison => (0, WITCH_POISON_DURATION_TICKS, true),
            EntityWitchPotionKind::Weakness => {
                (WEAKNESS_EFFECT_ID, WITCH_WEAKNESS_DURATION_TICKS, false)
            }
            EntityWitchPotionKind::Harming => continue,
        };
        let duration = (scale * f64::from(base_duration) + 0.5) as i32;
        if duration <= 20 {
            continue;
        }
        let effect = if poison {
            poison_effect(duration, 0)
        } else {
            caller_owned_effect(effect_id, duration, 0)
        };
        dispatches.extend(apply_player_effect_locked(inner, session_id, effect));
    }
}

fn commit_witch_potion_player_hit_locked(
    registry: &super::SessionRegistry,
    inner: &mut SessionEntityGuards<'_>,
    hit: ProjectilePlayerHit,
    potion: EntityWitchPotionKind,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> bool {
    if potion == EntityWitchPotionKind::Harming {
        return commit_plain_hurting_projectile_player_hit_locked(
            registry,
            inner,
            hit,
            PlainProjectileDamage {
                kind: PlayerDamageKind::IndirectMagic,
                amount: WITCH_HARMING_DAMAGE,
            },
            dispatches,
        );
    }
    let ProjectilePlayerHit {
        expected_projectile,
        next_projectile,
        ..
    } = hit;
    inner
        .entities
        .replace_snapshot_if_current(expected_projectile, next_projectile)
}

#[derive(Debug, Clone, Copy)]
enum HurtingProjectileTarget {
    Entity(EntityId),
    Player {
        session: SessionId,
        entity: EntityId,
    },
}

#[derive(Debug)]
struct HurtingProjectileTickScratch {
    candidates: Vec<EntitySnapshot>,
    owner_members: Vec<OwnerVehicleMember>,
    owner_vehicle_entities: Vec<EntityId>,
    hits: Vec<ThrowableEntityHit>,
    targets: Vec<HurtingProjectileTarget>,
}

impl Default for HurtingProjectileTickScratch {
    fn default() -> Self {
        Self {
            candidates: Vec::with_capacity(MAX_ARROW_TICK_CANDIDATES),
            owner_members: Vec::with_capacity(MAX_OWNER_VEHICLE_MEMBERS),
            owner_vehicle_entities: Vec::with_capacity(MAX_OWNER_VEHICLE_MEMBERS),
            hits: Vec::with_capacity(MAX_ARROW_TICK_CANDIDATES),
            targets: Vec::with_capacity(MAX_ARROW_TICK_CANDIDATES),
        }
    }
}

impl HurtingProjectileTarget {
    fn entity_id(self) -> EntityId {
        match self {
            Self::Entity(entity) => entity,
            Self::Player { entity, .. } => entity,
        }
    }
}

fn prepare_common_projectile_candidates_locked(
    inner: &SessionEntityGuards<'_>,
    projectile_id: EntityId,
    state: &ProjectileState,
    start: Vec3,
    end: Vec3,
    scratch: &mut HurtingProjectileTickScratch,
) -> bool {
    scratch.candidates.clear();
    scratch.owner_members.clear();
    scratch.owner_vehicle_entities.clear();
    scratch.hits.clear();
    scratch.targets.clear();
    if !collect_arrow_entity_candidate_snapshots_locked(inner, start, end, &mut scratch.candidates)
    {
        return false;
    }
    if !prepare_arrow_owner_members_locked(
        inner,
        state.owner,
        &scratch.candidates,
        &mut scratch.owner_members,
        &mut scratch.owner_vehicle_entities,
    ) {
        return false;
    }
    for snapshot in &scratch.candidates {
        if snapshot.id == projectile_id
            || snapshot.lifecycle != EntityLifecycle::Alive
            || snapshot.item_stack.is_some()
            || snapshot.experience_value.is_some()
            || snapshot.block_state.is_some()
            || snapshot.retained.hurting_projectile_state.is_some()
            || snapshot.retained.throwable_projectile_state.is_some()
            || snapshot.type_name == "minecraft:arrow"
        {
            continue;
        }
        let geometry = entity_geometry(&snapshot.type_name, snapshot.animal).aabb;
        let Some(hit_t) = segment_target_aabb_t(start, end, snapshot.position, geometry) else {
            continue;
        };
        let entity_id = snapshot.id;
        scratch.hits.push(ThrowableEntityHit {
            entity: projectile_entity(entity_id),
            location: projectile_vec(segment_location(start, end, hit_t)),
            eligibility: projectile_hit_eligibility(
                scratch.owner_vehicle_entities.contains(&entity_id),
            ),
            resolution: EntityHitResolution::Impact,
            input_order: 0,
        });
        scratch
            .targets
            .push(HurtingProjectileTarget::Entity(entity_id));
    }
    let mut player_ids = inner.sessions.keys().copied().collect::<Vec<_>>();
    if player_ids.len() > MAX_ARROW_TICK_CANDIDATES {
        return false;
    }
    player_ids.sort_unstable();
    for session_id in player_ids {
        let Some(session) = inner.sessions.get(&session_id) else {
            continue;
        };
        if inner.dead_sessions.contains(&session_id)
            || inner.spectator_sessions.contains(&session_id)
            || inner.client_unloaded_sessions.contains(&session_id)
        {
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
        let entity_id = EntityId(session.entity_id);
        let shares_owner_vehicle = Some(projectile_identity(entity_id)) == state.owner;
        scratch.hits.push(ThrowableEntityHit {
            entity: projectile_entity(entity_id),
            location: projectile_vec(segment_location(start, end, hit_t)),
            eligibility: projectile_hit_eligibility(shares_owner_vehicle),
            resolution: EntityHitResolution::Impact,
            input_order: 0,
        });
        scratch.targets.push(HurtingProjectileTarget::Player {
            session: session_id,
            entity: entity_id,
        });
    }
    scratch.hits.len() <= MAX_ARROW_TICK_CANDIDATES
}

fn hurting_projectile_snapshot_with_state(
    expected: &EntitySnapshot,
    state: mc_entity::projectile_26_1_2::HurtingProjectileState,
) -> EntitySnapshot {
    let mut next = expected.clone();
    next.position = session_vec(state.projectile.position);
    next.rotation = Rotation {
        yaw: state.projectile.rotation.yaw,
        pitch: state.projectile.rotation.pitch,
        head_yaw: state.projectile.rotation.yaw,
    };
    next.velocity = session_vec(state.projectile.velocity);
    next.on_ground = false;
    next.retained.hurting_projectile_state = Some(state);
    next
}

fn commit_dragon_fireball_hit_locked(
    inner: &mut SessionEntityGuards<'_>,
    expected_projectile: EntitySnapshot,
    next_projectile: EntitySnapshot,
    cloud_entity_type_id: Option<i32>,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> bool {
    let Some(cloud_entity_type_id) = cloud_entity_type_id else {
        return false;
    };
    let Some(owner) = expected_projectile
        .retained
        .hurting_projectile_state
        .and_then(|state| state.projectile.owner)
    else {
        return false;
    };
    let Ok(owner_entity_id) = u32::try_from(owner.raw()) else {
        return false;
    };
    let owner_entity_id = owner_entity_id as i32;
    let cloud_position = next_projectile.position;
    if !inner
        .entities
        .replace_snapshot_if_current(expected_projectile, next_projectile)
    {
        return false;
    }
    let mut cloud = SpawnEntity::new(
        cloud_entity_type_id,
        "minecraft:area_effect_cloud",
        cloud_position,
    );
    cloud.on_ground = false;
    apply_entity_facts(&mut cloud);
    cloud.retained.spawn_tick = inner.entity_lifecycle_tick;
    cloud.retained.dragon_breath_cloud = Some(
        mc_entity::EntityDragonBreathCloudState::dragon_fireball(owner_entity_id),
    );
    let aabb = entity_aabb(&cloud.type_name);
    let cloud_id = inner.entities.spawn(cloud);
    inner
        .entity_type_aabbs
        .entry(cloud_entity_type_id)
        .or_insert(aabb);
    track_entity_chunk_locked(inner, cloud_id, cloud_position);
    initialize_entity_wire_state_locked(inner, cloud_id);
    dispatches.extend(spawn_entity_visibility_locked(inner, cloud_id));
    true
}

fn commit_small_fireball_entity_hit_direct_locked(
    inner: &mut SessionEntityGuards<'_>,
    tick: u64,
    expected_projectile: EntitySnapshot,
    next_projectile: EntitySnapshot,
    expected_target: EntitySnapshot,
    amount: f32,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> bool {
    let damage = if projectile_entity_damage_is_admissible(&expected_target, tick, amount) {
        prepare_small_fireball_entity_damage(&expected_target, tick, amount)
    } else {
        None
    };
    commit_projectile_entity_damage_transaction_locked(
        inner,
        expected_projectile,
        next_projectile,
        expected_target,
        damage,
        false,
        tick,
        dispatches,
    )
}

fn commit_shulker_bullet_entity_hit_direct_locked(
    inner: &mut SessionEntityGuards<'_>,
    tick: u64,
    expected_projectile: EntitySnapshot,
    next_projectile: EntitySnapshot,
    expected_target: EntitySnapshot,
    amount: f32,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> bool {
    let damage = if projectile_entity_damage_is_admissible(&expected_target, tick, amount) {
        prepare_shulker_bullet_entity_damage(&expected_target, tick, amount)
    } else {
        None
    };
    commit_projectile_entity_damage_transaction_locked(
        inner,
        expected_projectile,
        next_projectile,
        expected_target,
        damage,
        true,
        tick,
        dispatches,
    )
}

fn commit_plain_hurting_projectile_entity_hit_direct_locked(
    inner: &mut SessionEntityGuards<'_>,
    tick: u64,
    expected_projectile: EntitySnapshot,
    next_projectile: EntitySnapshot,
    expected_target: EntitySnapshot,
    amount: f32,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> bool {
    let damage = if projectile_entity_damage_is_admissible(&expected_target, tick, amount) {
        prepare_plain_hurting_projectile_entity_damage(&expected_target, tick, amount)
    } else {
        None
    };
    commit_projectile_entity_damage_transaction_locked(
        inner,
        expected_projectile,
        next_projectile,
        expected_target,
        damage,
        false,
        tick,
        dispatches,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "the atomic entity-damage transaction must retain its independent CAS fences, damage result, effect flag, tick, and publication sink"
)]
fn commit_projectile_entity_damage_transaction_locked(
    inner: &mut SessionEntityGuards<'_>,
    expected_projectile: EntitySnapshot,
    next_projectile: EntitySnapshot,
    expected_target: EntitySnapshot,
    damage: Option<EntityDamage>,
    apply_levitation: bool,
    tick: u64,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> bool {
    let mut transaction = vec![(expected_projectile, next_projectile)];
    if let Some(damage) = &damage {
        transaction.push((expected_target, damage.snapshot.clone()));
    }
    if !inner.entities.replace_snapshots_if_current(transaction) {
        return false;
    }
    let Some(damage) = damage else {
        return true;
    };
    dispatches.extend(publish_accepted_entity_health_locked(
        inner,
        &damage.snapshot,
    ));
    if damage.killed {
        let rewards = projectile_entity_kill_rewards(inner, &damage.snapshot);
        let (_, death_dispatches) = begin_server_entity_death_locked(inner, &damage, &rewards);
        dispatches.extend(death_dispatches);
    } else {
        dispatches.extend(entity_hurt_dispatches_locked(inner, damage.snapshot.id));
        if apply_levitation {
            let _ = inner.entities.apply_effect_if_current(
                damage.snapshot.clone(),
                mc_entity::EntityEffectRequest {
                    operation: mc_entity::EntityEffectOperation::Add(
                        mc_entity::effects_26_1_2::EffectInstance::new(
                            mc_entity::effects_26_1_2::EffectId::new(
                                SHULKER_BULLET_LEVITATION_EFFECT_ID,
                            ),
                            mc_entity::effects_26_1_2::EffectKind::CallerOwned,
                            SHULKER_BULLET_LEVITATION_TICKS,
                            0,
                            mc_entity::effects_26_1_2::EffectFlags::default(),
                        ),
                    ),
                    target_kind: mc_entity::runtime_26_1_2::TargetKind::NonPlayer,
                    death_remove_tick: tick.saturating_add(ENTITY_DEATH_TICKS),
                },
            );
        }
    }
    true
}

fn commit_projectile_entity_hit_locked(
    registry: &super::SessionRegistry,
    inner: &mut SessionEntityGuards<'_>,
    hit: ProjectileEntityHit,
    impact: ProjectileEntityDamageImpact,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> bool {
    let ProjectileEntityHit {
        expected_projectile,
        next_projectile,
        expected_target,
        amount,
    } = hit;
    let tick = inner.entity_lifecycle_tick;
    if projectile_entity_damage_is_admissible(&expected_target, tick, amount) {
        match defer_projectile_entity_damage_locked(
            registry,
            inner,
            expected_projectile.clone(),
            next_projectile.clone(),
            expected_target.clone(),
            amount,
            impact,
        ) {
            ProjectileDamageAdmission::Pending => return false,
            ProjectileDamageAdmission::Refused => {
                return commit_projectile_entity_impact_without_damage_locked(
                    inner,
                    expected_projectile,
                    next_projectile,
                    impact,
                    tick,
                    dispatches,
                );
            }
            ProjectileDamageAdmission::Direct => {}
        }
    }
    commit_projectile_entity_impact_locked(
        inner,
        tick,
        expected_projectile,
        next_projectile,
        expected_target,
        amount,
        impact,
        dispatches,
    )
}

fn commit_projectile_entity_impact_after_precommit_locked(
    inner: &mut SessionEntityGuards<'_>,
    expected_projectile: EntitySnapshot,
    next_projectile: EntitySnapshot,
    expected_target: EntitySnapshot,
    request: EntityDamageRequest,
    impact: ProjectileEntityDamageImpact,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> bool {
    commit_projectile_entity_impact_locked(
        inner,
        request.tick,
        expected_projectile,
        next_projectile,
        expected_target,
        request.amount,
        impact,
        dispatches,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "the impact dispatcher keeps its independent projectile and target fences, impact protocol, tick, and publication sink explicit"
)]
fn commit_projectile_entity_impact_locked(
    inner: &mut SessionEntityGuards<'_>,
    tick: u64,
    expected_projectile: EntitySnapshot,
    next_projectile: EntitySnapshot,
    expected_target: EntitySnapshot,
    amount: f32,
    impact: ProjectileEntityDamageImpact,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> bool {
    match impact {
        ProjectileEntityDamageImpact::Plain => {
            commit_plain_hurting_projectile_entity_hit_direct_locked(
                inner,
                tick,
                expected_projectile,
                next_projectile,
                expected_target,
                amount,
                dispatches,
            )
        }
        ProjectileEntityDamageImpact::SmallFireball => {
            commit_small_fireball_entity_hit_direct_locked(
                inner,
                tick,
                expected_projectile,
                next_projectile,
                expected_target,
                amount,
                dispatches,
            )
        }
        ProjectileEntityDamageImpact::ShulkerBullet => {
            commit_shulker_bullet_entity_hit_direct_locked(
                inner,
                tick,
                expected_projectile,
                next_projectile,
                expected_target,
                amount,
                dispatches,
            )
        }
        ProjectileEntityDamageImpact::Arrow {
            start, location, ..
        } => commit_arrow_entity_hit_direct_locked(
            inner,
            tick,
            expected_projectile,
            next_projectile,
            expected_target,
            start,
            location,
            amount,
            dispatches,
        ),
    }
}

fn projectile_entity_damage_is_admissible(
    expected: &EntitySnapshot,
    tick: u64,
    amount: f32,
) -> bool {
    expected.lifecycle == EntityLifecycle::Alive
        && expected.health.is_finite()
        && expected.health > 0.0
        && amount.is_finite()
        && amount > 0.0
        && expected
            .retained
            .last_damage_tick
            .is_none_or(|last| tick.saturating_sub(last) >= ENTITY_HURT_INVULNERABLE_TICKS)
}

#[expect(
    clippy::too_many_arguments,
    reason = "the arrow transaction must retain its independent projectile and target fences, impact coordinates, tick, and publication sink"
)]
fn commit_arrow_entity_hit_direct_locked(
    inner: &mut SessionEntityGuards<'_>,
    tick: u64,
    expected_projectile: EntitySnapshot,
    next_projectile: EntitySnapshot,
    expected_target: EntitySnapshot,
    start: Vec3,
    location: Vec3,
    amount: f32,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> bool {
    let Some(damage) = prepare_arrow_entity_damage(&expected_target, start, location, tick, amount)
    else {
        return false;
    };
    if !inner.entities.replace_snapshots_if_current([
        (expected_projectile, next_projectile),
        (expected_target, damage.snapshot.clone()),
    ]) {
        return false;
    }
    dispatches.extend(publish_accepted_entity_health_locked(
        inner,
        &damage.snapshot,
    ));
    if damage.killed {
        let rewards = projectile_entity_kill_rewards(inner, &damage.snapshot);
        let (_, death_dispatches) = begin_server_entity_death_locked(inner, &damage, &rewards);
        dispatches.extend(death_dispatches);
    } else {
        publish_arrow_knockback_locked(inner, &damage.snapshot, dispatches);
        dispatches.extend(entity_hurt_dispatches_locked(inner, damage.snapshot.id));
    }
    true
}

fn prepare_plain_hurting_projectile_entity_damage(
    expected: &EntitySnapshot,
    tick: u64,
    amount: f32,
) -> Option<EntityDamage> {
    if expected.lifecycle != EntityLifecycle::Alive
        || !expected.health.is_finite()
        || expected.health <= 0.0
        || !amount.is_finite()
        || amount <= 0.0
    {
        return None;
    }
    let mut next = expected.clone();
    next.health = (next.health - amount).max(0.0);
    next.retained.last_damage_tick = Some(tick);
    let killed = next.health <= 0.0;
    if killed {
        next.lifecycle = EntityLifecycle::Despawning;
        next.retained.death_remove_tick = Some(tick.saturating_add(ENTITY_DEATH_TICKS));
        next.retained.sheep_grazing_ticks = None;
    }
    Some(EntityDamage {
        snapshot: next,
        killed,
    })
}

fn prepare_shulker_bullet_entity_damage(
    expected: &EntitySnapshot,
    tick: u64,
    amount: f32,
) -> Option<EntityDamage> {
    prepare_plain_hurting_projectile_entity_damage(expected, tick, amount)
}

fn prepare_small_fireball_entity_damage(
    expected: &EntitySnapshot,
    tick: u64,
    amount: f32,
) -> Option<EntityDamage> {
    if expected.lifecycle != EntityLifecycle::Alive
        || !expected.health.is_finite()
        || expected.health <= 0.0
    {
        return None;
    }
    let mut next = expected.clone();
    next.retained.remaining_fire_ticks =
        mc_entity::fire_26_1_2::ignite_for_seconds(next.retained.remaining_fire_ticks, 5.0);
    next.health = (next.health - amount).max(0.0);
    next.retained.last_damage_tick = Some(tick);
    let killed = next.health <= 0.0;
    if killed {
        next.lifecycle = EntityLifecycle::Despawning;
        next.retained.death_remove_tick = Some(tick.saturating_add(ENTITY_DEATH_TICKS));
        next.retained.sheep_grazing_ticks = None;
    }
    Some(EntityDamage {
        snapshot: next,
        killed,
    })
}

fn projectile_entity_kill_rewards(
    inner: &SessionEntityGuards<'_>,
    snapshot: &EntitySnapshot,
) -> EntityKillRewards {
    EntityKillRewards {
        items: inner.arrow_kill_rewards.item_entity_type_id.map_or_else(
            Vec::new,
            |entity_type_id| {
                entity_kill_drop_stacks(
                    &inner.arrow_kill_rewards,
                    &snapshot.type_name,
                    snapshot.animal,
                    snapshot.id.0 as i64 as u64,
                )
                .into_iter()
                .map(|drop| (entity_type_id, entity_item_stack(drop)))
                .collect()
            },
        ),
        experience: inner
            .arrow_kill_rewards
            .xp_orb_entity_type_id
            .map(|entity_type_id| (entity_type_id, mob_xp_value(&snapshot.type_name))),
    }
}
fn defer_projectile_entity_damage_locked(
    registry: &super::SessionRegistry,
    inner: &mut SessionEntityGuards<'_>,
    expected_projectile: EntitySnapshot,
    next_projectile: EntitySnapshot,
    expected_target: EntitySnapshot,
    amount: f32,
    impact: ProjectileEntityDamageImpact,
) -> ProjectileDamageAdmission {
    if let Some(admission) = pending_projectile_admission_locked(inner, expected_projectile.id) {
        return admission;
    }
    if !registry
        .precommit_boundary()
        .is_some_and(|boundary| boundary.has_precommit_hooks(HookKind::Damage))
    {
        return ProjectileDamageAdmission::Direct;
    }
    let Some(handle) = registry.damage_precommit_handle() else {
        return ProjectileDamageAdmission::Refused;
    };
    let Ok(source) = u64::try_from(expected_projectile.id.0).map(HookActor::Entity) else {
        return ProjectileDamageAdmission::Refused;
    };
    let tick = inner.entity_lifecycle_tick;
    let request = EntityDamageRequest {
        amount,
        tick,
        death_remove_tick: tick.saturating_add(ENTITY_DEATH_TICKS),
        villager_gossip_event: None,
    };
    let pending = match begin_entity_damage(
        registry,
        expected_target.id,
        source,
        "projectile",
        expected_projectile.position,
        amount,
    ) {
        Ok(Some(pending)) => pending,
        Ok(None) => return ProjectileDamageAdmission::Direct,
        Err(_) => return ProjectileDamageAdmission::Refused,
    };
    inner
        .pending_projectile_damage
        .insert(expected_projectile.id, pending.deadline());
    handle.spawn_precommit_resume(pending, None, None, move |decision| {
        crate::play::simulation::SimulationCommand::ResumeEntityDamagePrecommit(Box::new(
            EntityDamagePrecommitResume {
                expected: expected_target.clone(),
                request,
                attacker_costs: None,
                completion: EntityDamagePrecommitCompletion::Projectile(
                    ProjectileEntityDamageContinuation {
                        expected_projectile,
                        next_projectile,
                        expected_target,
                        impact,
                    },
                ),
                decision,
            },
        ))
    });
    ProjectileDamageAdmission::Pending
}

fn finish_projectile_entity_impact_locked(
    inner: &mut SessionEntityGuards<'_>,
    projectile_id: EntityId,
    next_projectile: &EntitySnapshot,
    impact: ProjectileEntityDamageImpact,
    tick: u64,
    dispatches: &mut Vec<VisibilityDispatch>,
) {
    if let ProjectileEntityDamageImpact::Arrow { discard: false, .. } = impact {
        if inner.entities.contains(projectile_id) {
            synchronize_arrow_snapshot_locked(inner, projectile_id);
        }
    } else if next_projectile.retained.pending_explosion.is_some() {
        schedule_primed_tnt_deadline_locked(inner, projectile_id, Some(tick));
    } else if let Some((_, removed)) = remove_server_entity_locked(inner, projectile_id) {
        dispatches.extend(removed);
    }
}

fn commit_projectile_entity_impact_without_damage_locked(
    inner: &mut SessionEntityGuards<'_>,
    expected_projectile: EntitySnapshot,
    next_projectile: EntitySnapshot,
    impact: ProjectileEntityDamageImpact,
    tick: u64,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> bool {
    let projectile_id = expected_projectile.id;
    if !inner
        .entities
        .replace_snapshot_if_current(expected_projectile, next_projectile.clone())
    {
        return false;
    }
    finish_projectile_entity_impact_locked(
        inner,
        projectile_id,
        &next_projectile,
        impact,
        tick,
        dispatches,
    );
    true
}

pub(in crate::play) fn resume_projectile_entity_damage_precommit(
    registry: &SessionRegistry,
    continuation: ProjectileEntityDamageContinuation,
    mut request: EntityDamageRequest,
    decision: Result<Approval, HookFailure>,
) -> Vec<VisibilityDispatch> {
    let ProjectileEntityDamageContinuation {
        expected_projectile,
        next_projectile,
        expected_target,
        impact,
    } = continuation;
    let mut inner = registry.lock_session_entities("resume projectile entity damage precommit");
    inner
        .pending_projectile_damage
        .remove(&expected_projectile.id);
    if inner.entities.snapshot(expected_projectile.id).as_ref() != Some(&expected_projectile)
        || inner.entities.snapshot(expected_target.id).as_ref() != Some(&expected_target)
    {
        return Vec::new();
    }
    let amount = match decision {
        Ok(mut approval) => {
            let amount = match approval.decision() {
                HookDecision::Keep => Some(request.amount),
                HookDecision::Replace(amount) if amount.is_finite() && amount > 0.0 => Some(amount),
                HookDecision::Cancel | HookDecision::Replace(_) | _ => None,
            };
            approval.consume().ok().and(amount)
        }
        Err(_) => None,
    };
    let mut dispatches = Vec::new();
    let committed = if let Some(amount) = amount {
        request.amount = amount;
        commit_projectile_entity_impact_after_precommit_locked(
            &mut inner,
            expected_projectile,
            next_projectile.clone(),
            expected_target,
            request,
            impact,
            &mut dispatches,
        )
    } else {
        commit_projectile_entity_impact_without_damage_locked(
            &mut inner,
            expected_projectile,
            next_projectile.clone(),
            impact,
            request.tick,
            &mut dispatches,
        )
    };
    if !committed {
        return Vec::new();
    }
    finish_projectile_entity_impact_locked(
        &mut inner,
        next_projectile.id,
        &next_projectile,
        impact,
        request.tick,
        &mut dispatches,
    );
    drop(inner);
    registry.append_spawned_xp_pickup_candidates(&mut dispatches);
    dispatches
}

fn begin_projectile_player_damage_locked(
    registry: &super::SessionRegistry,
    inner: &SessionEntityGuards<'_>,
    target_session: SessionId,
    request: PlayerDamageRequest,
    position: Vec3,
    source: EntityId,
) -> Result<Option<mc_script::precommit::PendingDecision>, HookFailure> {
    let Some(boundary) = registry.precommit_boundary() else {
        return Ok(None);
    };
    if !boundary.has_precommit_hooks(HookKind::Damage) {
        return Ok(None);
    }
    let target_uuid = inner
        .sessions
        .get(&target_session)
        .map(|session| session.uuid.to_string())
        .ok_or(HookFailure::Unavailable)?;
    let target = HookPlayer::try_new(target_uuid, target_session).map_err(HookFailure::from)?;
    let source = u64::try_from(source.0)
        .map(HookActor::Entity)
        .map_err(|_| HookFailure::Invalid)?;
    let position =
        ScriptPosition::try_new(position.x, position.y, position.z).ok_or(HookFailure::Invalid)?;
    let context = DamageContext::try_new(
        source,
        DamageTarget::Player(target),
        damage_kind_name(request.kind),
        "minecraft:overworld",
        position,
        request.amount,
    )
    .map(HookContext::Damage)
    .map_err(HookFailure::from)?;
    boundary.begin_precommit(context).map(Some)
}

fn defer_projectile_player_damage_locked(
    registry: &super::SessionRegistry,
    inner: &mut SessionEntityGuards<'_>,
    target_session: SessionId,
    request: PlayerDamageRequest,
    expected_projectile: EntitySnapshot,
    next_projectile: EntitySnapshot,
) -> ProjectileDamageAdmission {
    if let Some(admission) = pending_projectile_admission_locked(inner, expected_projectile.id) {
        return admission;
    }
    if !registry
        .precommit_boundary()
        .is_some_and(|boundary| boundary.has_precommit_hooks(HookKind::Damage))
    {
        return ProjectileDamageAdmission::Direct;
    }
    let Some(handle) = registry.damage_precommit_handle() else {
        return ProjectileDamageAdmission::Refused;
    };
    let Some((target_fence, position)) =
        SessionRegistry::player_damage_target_fence_locked(inner, target_session)
    else {
        return ProjectileDamageAdmission::Refused;
    };
    let tick = inner.entity_lifecycle_tick;
    let pending = match begin_projectile_player_damage_locked(
        registry,
        inner,
        target_session,
        request,
        position,
        expected_projectile.id,
    ) {
        Ok(Some(pending)) => pending,
        Ok(None) => return ProjectileDamageAdmission::Direct,
        Err(_) => return ProjectileDamageAdmission::Refused,
    };
    inner
        .pending_projectile_damage
        .insert(expected_projectile.id, pending.deadline());
    handle.spawn_precommit_resume(pending, None, None, move |decision| {
        crate::play::simulation::SimulationCommand::ResumePlayerDamagePrecommit(Box::new(
            PlayerDamagePrecommitResume::Projectile {
                target_session,
                target_fence,
                tick,
                request,
                continuation: ProjectileDamageContinuation::Hit {
                    expected_projectile,
                    next_projectile,
                },
                decision,
            },
        ))
    });
    ProjectileDamageAdmission::Pending
}

fn commit_projectile_player_hit_locked(
    registry: &super::SessionRegistry,
    inner: &mut SessionEntityGuards<'_>,
    expected_projectile: EntitySnapshot,
    next_projectile: EntitySnapshot,
    target_session: SessionId,
    request: PlayerDamageRequest,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> bool {
    match defer_projectile_player_damage_locked(
        registry,
        inner,
        target_session,
        request,
        expected_projectile.clone(),
        next_projectile.clone(),
    ) {
        ProjectileDamageAdmission::Pending => return false,
        ProjectileDamageAdmission::Refused => {
            return commit_deferred_projectile_player_impact_without_damage_locked(
                inner,
                ProjectileDamageContinuation::Hit {
                    expected_projectile,
                    next_projectile,
                },
                dispatches,
            );
        }
        ProjectileDamageAdmission::Direct => {}
    }
    let preview = prepare_projectile_player_damage_locked(
        inner,
        target_session,
        inner.entity_lifecycle_tick,
        request,
    );
    let prepared = match preview {
        ProjectilePlayerDamagePreview::Accepted(prepared)
        | ProjectilePlayerDamagePreview::Rejected(Some(prepared)) => Some(prepared),
        ProjectilePlayerDamagePreview::Rejected(None) => None,
    };
    if let Some(prepared) = prepared {
        commit_projectile_player_damage_locked(
            inner,
            prepared,
            |inner| {
                inner
                    .entities
                    .replace_snapshot_if_current(expected_projectile, next_projectile)
            },
            dispatches,
        )
    } else {
        inner
            .entities
            .replace_snapshot_if_current(expected_projectile, next_projectile)
    }
}

fn finish_deferred_projectile_player_impact_locked(
    inner: &mut SessionEntityGuards<'_>,
    projectile_id: EntityId,
    next_projectile: &EntitySnapshot,
    dispatches: &mut Vec<VisibilityDispatch>,
) {
    if next_projectile.retained.pending_explosion.is_some() {
        let tick = inner.entity_lifecycle_tick;
        schedule_primed_tnt_deadline_locked(inner, projectile_id, Some(tick));
    } else if let Some((_, removed)) = remove_server_entity_locked(inner, projectile_id) {
        dispatches.extend(removed);
    }
}

pub(super) fn commit_deferred_projectile_player_impact_without_damage_locked(
    inner: &mut SessionEntityGuards<'_>,
    continuation: ProjectileDamageContinuation,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> bool {
    match continuation {
        ProjectileDamageContinuation::Hit {
            expected_projectile,
            next_projectile,
        } => {
            let projectile_id = expected_projectile.id;
            if !inner
                .entities
                .replace_snapshot_if_current(expected_projectile, next_projectile.clone())
            {
                return false;
            }
            finish_deferred_projectile_player_impact_locked(
                inner,
                projectile_id,
                &next_projectile,
                dispatches,
            );
            true
        }
    }
}

pub(super) fn commit_deferred_projectile_player_damage_locked(
    inner: &mut SessionEntityGuards<'_>,
    prepared: PreparedProjectilePlayerDamage,
    continuation: ProjectileDamageContinuation,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> bool {
    match continuation {
        ProjectileDamageContinuation::Hit {
            expected_projectile,
            next_projectile,
        } => {
            let projectile_id = expected_projectile.id;
            if !commit_projectile_player_damage_locked(
                inner,
                prepared,
                |inner| {
                    inner
                        .entities
                        .replace_snapshot_if_current(expected_projectile, next_projectile.clone())
                },
                dispatches,
            ) {
                return false;
            }
            finish_deferred_projectile_player_impact_locked(
                inner,
                projectile_id,
                &next_projectile,
                dispatches,
            );
            true
        }
    }
}
fn commit_shulker_bullet_player_hit_locked(
    registry: &super::SessionRegistry,
    inner: &mut SessionEntityGuards<'_>,
    expected_projectile: EntitySnapshot,
    next_projectile: EntitySnapshot,
    target_session: SessionId,
    source_origin: Vec3,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> bool {
    commit_projectile_player_hit_locked(
        registry,
        inner,
        expected_projectile,
        next_projectile,
        target_session,
        PlayerDamageRequest {
            kind: PlayerDamageKind::ShulkerBullet,
            amount: SHULKER_BULLET_ENTITY_DAMAGE,
            source_origin: Some(source_origin),
        },
        dispatches,
    )
}
fn commit_plain_hurting_projectile_player_hit_locked(
    registry: &super::SessionRegistry,
    inner: &mut SessionEntityGuards<'_>,
    hit: ProjectilePlayerHit,
    damage: PlainProjectileDamage,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> bool {
    let ProjectilePlayerHit {
        expected_projectile,
        next_projectile,
        target_session,
        source_origin,
    } = hit;
    commit_projectile_player_hit_locked(
        registry,
        inner,
        expected_projectile,
        next_projectile,
        target_session,
        PlayerDamageRequest {
            kind: damage.kind,
            amount: damage.amount,
            source_origin: Some(source_origin),
        },
        dispatches,
    )
}
fn commit_small_fireball_player_hit_locked(
    registry: &super::SessionRegistry,
    inner: &mut SessionEntityGuards<'_>,
    expected_projectile: EntitySnapshot,
    next_projectile: EntitySnapshot,
    target_session: SessionId,
    source_origin: Vec3,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> bool {
    commit_projectile_player_hit_locked(
        registry,
        inner,
        expected_projectile,
        next_projectile,
        target_session,
        PlayerDamageRequest {
            kind: PlayerDamageKind::Fireball,
            amount: SMALL_FIREBALL_ENTITY_DAMAGE,
            source_origin: Some(source_origin),
        },
        dispatches,
    )
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
            registry,
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

pub(super) fn initial_throwable_projectile_state(
    owner: Option<EntityIdentity>,
    type_name: &str,
    position: Vec3,
    velocity: Vec3,
    rotation: Rotation,
) -> Option<ThrowableState> {
    let geometry = entity_geometry(type_name, None).aabb;
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
    Some(ThrowableState::new(projectile))
}

pub(super) fn initial_hurting_projectile_state(
    owner: Option<EntityIdentity>,
    type_name: &str,
    position: Vec3,
    direction: Vec3,
    rotation: Rotation,
) -> Option<HurtingProjectileState> {
    initial_hurting_projectile_state_with_motion(
        owner,
        type_name,
        position,
        direction,
        rotation,
        HurtingProjectileMotionProfile {
            acceleration_power: HURTING_PROJECTILE_DEFAULT_ACCELERATION_POWER,
            air_inertia: mc_entity::projectile_26_1_2::HURTING_PROJECTILE_AIR_INERTIA,
            water_inertia: mc_entity::projectile_26_1_2::HURTING_PROJECTILE_WATER_INERTIA,
        },
    )
}

pub(super) fn initial_hurting_projectile_state_with_motion(
    owner: Option<EntityIdentity>,
    type_name: &str,
    position: Vec3,
    direction: Vec3,
    rotation: Rotation,
    motion: HurtingProjectileMotionProfile,
) -> Option<HurtingProjectileState> {
    let geometry = entity_geometry(type_name, None).aabb;
    let bounds = ProjectileAabb::new(
        position.x - geometry.half_width,
        position.y,
        position.z - geometry.half_width,
        position.x + geometry.half_width,
        position.y + geometry.height,
        position.z + geometry.half_width,
    )
    .ok()?;
    HurtingProjectileState::new(
        owner,
        projectile_vec(position),
        bounds,
        projectile_vec(direction),
        ProjectileRotation::new(rotation.yaw, rotation.pitch),
        motion.acceleration_power,
    )
    .and_then(|state| state.with_inertia(motion.air_inertia, motion.water_inertia))
    .ok()
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

#[expect(
    clippy::too_many_arguments,
    reason = "the atomic arrow transaction keeps its independent arrow fences, start position, publications, scratch state, and publication sink explicit"
)]
fn commit_arrow_transaction_locked(
    registry: &super::SessionRegistry,
    inner: &mut SessionEntityGuards<'_>,
    expected_arrow: EntitySnapshot,
    next_arrow: EntitySnapshot,
    start: Vec3,
    publications: &mc_entity::projectile_26_1_2::PublicationBatch,
    scratch: &mut ArrowTickScratch,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> Result<bool, ()> {
    let will_discard = publications
        .iter()
        .any(|publication| matches!(publication, ProjectilePublication::Discarded { .. }));
    for publication in publications.iter() {
        let ProjectilePublication::ArrowDamageAccepted { entity, .. } = publication else {
            continue;
        };
        let entity = session_entity(entity);
        let Some(target) = scratch
            .targets
            .iter()
            .find(|target| target.entity_id == entity)
        else {
            return Err(());
        };
        let (Some(location), Some(expected_target)) =
            (target.entity_location, target.expected_entity.as_ref())
        else {
            continue;
        };
        let impact = ProjectileEntityDamageImpact::Arrow {
            start,
            location,
            discard: will_discard,
        };
        match defer_projectile_entity_damage_locked(
            registry,
            inner,
            expected_arrow.clone(),
            next_arrow.clone(),
            expected_target.clone(),
            ARROW_ENTITY_HIT_DAMAGE,
            impact,
        ) {
            ProjectileDamageAdmission::Pending => return Err(()),
            ProjectileDamageAdmission::Refused => {
                if !commit_projectile_entity_impact_without_damage_locked(
                    inner,
                    expected_arrow.clone(),
                    next_arrow.clone(),
                    impact,
                    inner.entity_lifecycle_tick,
                    dispatches,
                ) {
                    return Err(());
                }
                return Ok(false);
            }
            ProjectileDamageAdmission::Direct => {}
        }
    }
    scratch.transaction.clear();
    scratch
        .transaction
        .push((expected_arrow.clone(), next_arrow.clone()));
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
                        ARROW_ENTITY_HIT_DAMAGE,
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
        match defer_projectile_player_damage_locked(
            registry,
            inner,
            player_damage.target_session(),
            PlayerDamageRequest {
                kind: PlayerDamageKind::Projectile,
                amount: ARROW_ENTITY_HIT_DAMAGE,
                source_origin: Some(start),
            },
            expected_arrow.clone(),
            next_arrow.clone(),
        ) {
            ProjectileDamageAdmission::Pending => return Err(()),
            ProjectileDamageAdmission::Refused => {
                if !commit_deferred_projectile_player_impact_without_damage_locked(
                    inner,
                    ProjectileDamageContinuation::Hit {
                        expected_projectile: expected_arrow,
                        next_projectile: next_arrow,
                    },
                    dispatches,
                ) {
                    return Err(());
                }
                return Ok(false);
            }
            ProjectileDamageAdmission::Direct => {}
        }
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
    amount: f32,
) -> Option<EntityDamage> {
    if expected.lifecycle != EntityLifecycle::Alive
        || !expected.health.is_finite()
        || expected.health <= 0.0
    {
        return None;
    }
    let mut next = expected.clone();
    next.health = (next.health - amount).max(0.0);
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
            let rewards = projectile_entity_kill_rewards(inner, &damage.snapshot);
            let (_, target_dispatches) = begin_server_entity_death_locked(inner, &damage, &rewards);
            dispatches.extend(target_dispatches);
        } else {
            publish_arrow_knockback_locked(inner, &damage.snapshot, dispatches);
            dispatches.extend(entity_hurt_dispatches_locked(inner, damage.snapshot.id));
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
    if chunk_count >= inner.simulation_inputs.tracked_chunk_count() as u64 {
        for id in inner.simulation_inputs.all_entity_ids() {
            if !push_arrow_candidate_snapshot(inner, snapshots, id) {
                return false;
            }
        }
        snapshots.sort_unstable_by_key(|snapshot| snapshot.id);
        return true;
    }

    for cz in min_cz..=max_cz {
        for cx in min_cx..=max_cx {
            if let Some(chunk_ids) = inner.simulation_inputs.entities_in_chunk((cx, cz)) {
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
