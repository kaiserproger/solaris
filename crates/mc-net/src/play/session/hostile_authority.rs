use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use mc_data::mob_behavior_26_1_2::{MobBehaviorTable, MobCombatPolicy};
use mc_entity::{
    AttributeKind, EntityBlazeAttackState, EntityBreezeAttackPhase, EntityBreezeAttackState,
    EntityCrossbowAttackPhase, EntityCrossbowAttackState, EntityEvokerAttackPhase,
    EntityEvokerAttackState, EntityEvokerFangState, EntityExplosionInteraction,
    EntityGhastAttackState, EntityGuardianBeamPhase, EntityGuardianBeamState, EntityId,
    EntityLifecycle, EntityPendingExplosionState, EntityPrimedTntState, EntityShulkerAttackState,
    EntitySimulationProjection, EntitySnapshot, EntityWardenSonicBoomPhase,
    EntityWardenSonicBoomState, EntityWitchAttackState, EntityWitchPotionKind, GoalState, Rotation,
    SpawnEntity, Vec3,
};
use mc_world::BlockStateId;

#[cfg(test)]
use crate::play::HOSTILE_FOLLOW_SPEED;
use crate::play::combat::{PlayerDamageKind, PlayerDamageRequest};
use crate::play::simulation::SimulationAuthority;
use crate::play::{
    CREEPER_CANCEL_RANGE, CREEPER_FUSE_TICKS, CREEPER_TRIGGER_RANGE, HOSTILE_MELEE_PERIOD_TICKS,
    HOSTILE_MELEE_RANGE, HOSTILE_MELEE_VERTICAL_REACH, SKELETON_ARROW_SPEED,
    SKELETON_SHOT_PERIOD_TICKS, SKELETON_SHOT_RANGE,
};

#[cfg(test)]
use super::entity_lifecycle::nearby_entity_snapshots_locked;
use super::entity_lifecycle::{
    nearby_entity_candidate_ids_locked, remove_server_entity_locked, track_entity_chunk_locked,
};
use super::entity_owner::EntityOwnerAccess;
use super::explosion_authority::schedule_primed_tnt_deadline_locked;
use super::interaction_geometry::{distance_sq, entity_aabb};
#[cfg(test)]
use super::outbound::ServerEntitySnapshot;
use super::outbound::{OutboundCommand, VisibilityDispatch};
use super::player_effects::PlayerEffectFacts;
use super::projectiles::{
    HurtingProjectileMotionProfile, initial_arrow_state, initial_hurting_projectile_state,
    initial_hurting_projectile_state_with_motion, initial_throwable_projectile_state,
    projectile_identity,
};
use super::visibility::{
    entity_event_dispatches_locked, initialize_entity_wire_state_from_snapshot_locked,
    server_entity_snapshot_from, session_recipients, spawn_entity_visibility_from_snapshot_locked,
    visibility_dispatches, visible_entity_observers_locked,
};
use super::{
    SessionRegistry, apply_entity_facts, is_hostile_entity, record_entity_dispatches_locked,
};

// Exact 26.1.2 defaults from RangedCrossbowAttackGoal/CrossbowItem in the local client jar.
const PILLAGER_CROSSBOW_RANGE: f64 = 8.0;
const PILLAGER_AIM_VISIBLE_TICKS: u64 = 5;
const PILLAGER_CROSSBOW_CHARGE_TICKS: u64 = 25;
const PILLAGER_CROSSBOW_ATTACK_DELAY_MIN_TICKS: u64 = 20;
const PILLAGER_CROSSBOW_ATTACK_DELAY_SPAN_TICKS: u64 = 20;
const PLAYER_CROSSBOW_TARGET_Y_OFFSET: f64 = 0.6;

// Exact local 26.1.2 GuardianAttackGoal defaults. Solaris currently advertises EASY.
const GUARDIAN_BEAM_WARMUP_TICKS: u64 = 10;
const GUARDIAN_BEAM_DURATION_TICKS: u64 = 80;
const ELDER_GUARDIAN_BEAM_DURATION_TICKS: u64 = 60;
const GUARDIAN_MIN_TARGET_DISTANCE_SQ: f64 = 9.0;
const GUARDIAN_BEAM_START_EVENT: i8 = 21;
const GUARDIAN_EASY_MAGIC_DAMAGE: f32 = 1.0;

// Exact local 26.1.2 BlazeAttackGoal attackTime/attackStep defaults.
const BLAZE_CHARGE_TICKS: u64 = 60;
const BLAZE_SHOT_INTERVAL_TICKS: u64 = 6;
const BLAZE_BURST_COOLDOWN_TICKS: u64 = 100;
const BLAZE_CLOSE_MELEE_RANGE_SQ: f64 = 4.0;
const BLAZE_CLOSE_MELEE_PERIOD_TICKS: u64 = 20;
const BLAZE_PROJECTILE_Y_OFFSET: f64 = 1.4;
const BLAZE_TARGET_Y_OFFSET: f64 = 0.9;

// Exact local 26.1.2 SonicBoom behavior timings/ranges.
const WARDEN_SONIC_RANGE_XZ: f64 = 15.0;
const WARDEN_SONIC_RANGE_Y: f64 = 20.0;
const WARDEN_SONIC_CHARGE_TICKS: u64 = 34;
const WARDEN_SONIC_DURATION_TICKS: u64 = 60;
const WARDEN_SONIC_COOLDOWN_TICKS: u64 = 40;
const WARDEN_SONIC_EVENT: i8 = 62;
const WARDEN_SONIC_DAMAGE: f32 = 10.0;

const EVOKER_FANGS_WARMUP_TICKS: u64 = 20;
const EVOKER_FANGS_CASTING_TICKS: u64 = 40;
const EVOKER_FANGS_INTERVAL_TICKS: u64 = 100;
const EVOKER_FANGS_RANGE: f64 = 12.0;
const EVOKER_FANGS_DAMAGE: f32 = 6.0;
const EVOKER_FANGS_EVENT: i8 = 4;

const GHAST_FIREBALL_RANGE_SQ: f64 = 4096.0;
const GHAST_FIREBALL_SHOT_CHARGE: i32 = 20;
const GHAST_FIREBALL_RESET_CHARGE: i32 = -40;
const GHAST_FIREBALL_SPAWN_OFFSET: f64 = 4.0;
const GHAST_FIREBALL_Y_OFFSET: f64 = 2.5;
const GHAST_FIREBALL_EXPLOSION_POWER: f32 = 1.0;

const BREEZE_WIND_CHARGE_RANGE_SQ: f64 = 256.0;
const BREEZE_WIND_CHARGE_INITIAL_DELAY_TICKS: u64 = 15;
const BREEZE_WIND_CHARGE_RECOVERY_TICKS: u64 = 4;
const BREEZE_WIND_CHARGE_COOLDOWN_TICKS: u64 = 10;
const BREEZE_WIND_CHARGE_SPEED: f64 = 0.7;
const BREEZE_WIND_CHARGE_EXPLOSION_POWER: f32 = 3.0;
const BREEZE_WIND_CHARGE_FIRING_Y_OFFSET: f64 = 0.3;
const BREEZE_WIND_CHARGE_TARGET_Y_FRACTION: f64 = 0.3;

const WITHER_SKULL_RANGE: f64 = 20.0;
const WITHER_SKULL_SHOT_PERIOD_TICKS: u64 = 40;
const WITHER_SKULL_HEAD_Y_OFFSET: f64 = 3.0;
const WITHER_SKULL_EXPLOSION_POWER: f32 = 1.0;

const WITCH_ATTACK_INTERVAL_TICKS: u64 = 60;
const WITCH_ATTACK_RANGE_SQ: f64 = 100.0;
const WITCH_PROJECTILE_Y_OFFSET: f64 = 1.1;
const WITCH_POTION_SPEED_NEAR: f64 = 0.45;
const WITCH_POTION_SPEED_FAR: f64 = 0.75;

// Local 26.1.2 ShulkerAttackGoal: first shot after 20 ticks, range < 20 blocks.
const SHULKER_INITIAL_SHOT_DELAY_TICKS: u64 = 20;
const SHULKER_SHOT_DELAY_STEPS: u64 = 10;
const SHULKER_SHOT_DELAY_VARIANTS: u64 = 10;
const SHULKER_ATTACK_RANGE_SQ: f64 = 400.0;

struct HostileAttackTickEntity {
    id: EntityId,
    kind: HostileAttackKind,
    position: Vec3,
    rotation: Rotation,
    goal: GoalState,
    crossbow_attack: Option<EntityCrossbowAttackState>,
    blaze_attack: Option<EntityBlazeAttackState>,
    ghast_attack: Option<EntityGhastAttackState>,
    breeze_attack: Option<EntityBreezeAttackState>,
    witch_attack: Option<EntityWitchAttackState>,
    guardian_beam: Option<EntityGuardianBeamState>,
    warden_sonic_boom: Option<EntityWardenSonicBoomState>,
    shulker_attack: Option<EntityShulkerAttackState>,
    evoker_attack: Option<EntityEvokerAttackState>,
}

struct HostileTargetTickSession {
    id: super::SessionId,
    entity_id: i32,
    position: Vec3,
    visible_entities: Arc<HashSet<EntityId>>,
    effect_facts: Option<PlayerEffectFacts>,
}

struct PlannedCreeperFuse {
    hostile_id: EntityId,
    nearest_distance_sq: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
enum HostileAttackKind {
    Creeper,
    Skeleton,
    Crossbow,
    GuardianBeam {
        elder: bool,
        follow_range: f64,
        attack_damage: f32,
    },
    SmallFireball {
        follow_range: f64,
        attack_damage: f32,
    },
    SonicBoom,
    ShulkerBullet,
    EvokerFangs,
    LargeFireball,
    WindCharge,
    ThrownPotion,
    WitherSkull,
    Melee {
        attack_damage: f32,
    },
}

#[derive(Debug, Clone)]
struct PlannedArrowAttack {
    hostile_id: EntityId,
    arrow_entity_type_id: i32,
    position: Vec3,
    velocity: Vec3,
    rotation: Rotation,
    animate_shooter: bool,
}

#[derive(Debug, Clone)]
struct PlannedSmallFireballAttack {
    hostile_id: EntityId,
    entity_type_id: i32,
    position: Vec3,
    direction: Vec3,
    rotation: Rotation,
}

#[derive(Debug, Clone)]
struct PlannedLargeFireballAttack {
    hostile_id: EntityId,
    entity_type_id: i32,
    position: Vec3,
    direction: Vec3,
    rotation: Rotation,
    air_block_state: u32,
}

#[derive(Debug, Clone)]
struct PlannedWitherSkullAttack {
    hostile_id: EntityId,
    entity_type_id: i32,
    position: Vec3,
    direction: Vec3,
    rotation: Rotation,
    air_block_state: u32,
}

struct PlannedGhastTransition {
    hostile_id: EntityId,
    expected: Option<EntityGhastAttackState>,
    next: Option<EntityGhastAttackState>,
    shot: Option<PlannedLargeFireballAttack>,
}

#[derive(Debug, Clone)]
struct PlannedWindChargeAttack {
    hostile_id: EntityId,
    entity_type_id: i32,
    position: Vec3,
    velocity: Vec3,
    rotation: Rotation,
    air_block_state: u32,
}

struct PlannedBreezeTransition {
    hostile_id: EntityId,
    expected: Option<EntityBreezeAttackState>,
    next: Option<EntityBreezeAttackState>,
    shot: Option<PlannedWindChargeAttack>,
}

#[derive(Debug, Clone)]
struct PlannedThrownPotionAttack {
    hostile_id: EntityId,
    entity_type_id: i32,
    position: Vec3,
    velocity: Vec3,
    rotation: Rotation,
    potion: EntityWitchPotionKind,
}

struct PlannedWitchTransition {
    hostile_id: EntityId,
    expected: Option<EntityWitchAttackState>,
    next: Option<EntityWitchAttackState>,
    shot: Option<PlannedThrownPotionAttack>,
}

struct PlannedBlazeTransition {
    hostile_id: EntityId,
    expected: Option<EntityBlazeAttackState>,
    next: Option<EntityBlazeAttackState>,
    shot: Option<PlannedSmallFireballAttack>,
    melee: Option<PlannedMeleeAttack>,
}

struct PlannedCrossbowTransition {
    hostile_id: EntityId,
    expected: Option<EntityCrossbowAttackState>,
    next: Option<EntityCrossbowAttackState>,
    shot: Option<PlannedArrowAttack>,
}

struct PlannedGuardianBeamTransition {
    hostile_id: EntityId,
    expected: Option<EntityGuardianBeamState>,
    next: Option<EntityGuardianBeamState>,
    beam_started: bool,
    attack: Option<PlannedGuardianBeamAttack>,
}

#[derive(Debug, Clone, Copy)]
struct PlannedGuardianBeamAttack {
    hostile_id: EntityId,
    target_session: super::SessionId,
    magic_damage: f32,
    attack_damage: f32,
}

struct PlannedWardenSonicTransition {
    hostile_id: EntityId,
    expected: Option<EntityWardenSonicBoomState>,
    next: Option<EntityWardenSonicBoomState>,
    charge_started: bool,
    attack: Option<PlannedWardenSonicAttack>,
}

#[derive(Debug, Clone, Copy)]
struct PlannedWardenSonicAttack {
    hostile_id: EntityId,
    target_session: super::SessionId,
}

struct PlannedShulkerTransition {
    hostile_id: EntityId,
    expected: Option<EntityShulkerAttackState>,
    next: Option<EntityShulkerAttackState>,
    shot: Option<PlannedShulkerBulletAttack>,
}

#[derive(Debug, Clone)]
struct PlannedShulkerBulletAttack {
    hostile_id: EntityId,
    entity_type_id: i32,
    target_entity_id: i32,
    position: Vec3,
    rotation: Rotation,
}

struct PlannedEvokerTransition {
    hostile_id: EntityId,
    expected: Option<EntityEvokerAttackState>,
    next: Option<EntityEvokerAttackState>,
    fangs: Vec<PlannedEvokerFang>,
}

#[derive(Debug, Clone, Copy)]
struct PlannedEvokerFang {
    owner_id: EntityId,
    entity_type_id: i32,
    position: Vec3,
    rotation: Rotation,
    warmup_delay_ticks: i32,
}

struct PlannedMeleeAttack {
    hostile_id: EntityId,
    target_session: super::SessionId,
    amount: f32,
}

struct SpawnedHostileArrow {
    hostile_id: EntityId,
    snapshot: EntitySnapshot,
    animate_shooter: bool,
}

fn deterministic_crossbow_delay_ticks(entity_id: EntityId, tick: u64) -> u64 {
    let mixed = u64::from(entity_id.0.unsigned_abs()).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ tick.rotate_left(17);
    PILLAGER_CROSSBOW_ATTACK_DELAY_MIN_TICKS + mixed % PILLAGER_CROSSBOW_ATTACK_DELAY_SPAN_TICKS
}

fn deterministic_shulker_delay_ticks(entity_id: EntityId, tick: u64) -> u64 {
    let mixed = u64::from(entity_id.0.unsigned_abs()).wrapping_mul(0xD6E8_FEB8_6659_FD93)
        ^ tick.rotate_left(11);
    SHULKER_INITIAL_SHOT_DELAY_TICKS
        + (mixed % SHULKER_SHOT_DELAY_VARIANTS) * SHULKER_SHOT_DELAY_STEPS
}

fn plan_evoker_fang_pattern(
    hostile: &HostileAttackTickEntity,
    target: &HostileTargetTickSession,
    entity_type_id: i32,
) -> Vec<PlannedEvokerFang> {
    let dx = target.position.x - hostile.position.x;
    let dz = target.position.z - hostile.position.z;
    let angle = dz.atan2(dx);
    let y = hostile.position.y.min(target.position.y);
    let rotation = |angle: f64| Rotation {
        yaw: angle.to_degrees() as f32,
        pitch: 0.0,
        head_yaw: angle.to_degrees() as f32,
    };
    let fang = |reach: f64, angle: f64, warmup_delay_ticks: i32| PlannedEvokerFang {
        owner_id: hostile.id,
        entity_type_id,
        position: Vec3::new(
            hostile.position.x + angle.cos() * reach,
            y,
            hostile.position.z + angle.sin() * reach,
        ),
        rotation: rotation(angle),
        warmup_delay_ticks,
    };
    if distance_sq(hostile.position, target.position) < 9.0 {
        let mut fangs = Vec::with_capacity(13);
        for i in 0..5 {
            let fang_angle = angle + f64::from(i) * std::f64::consts::PI * 0.4;
            fangs.push(fang(1.5, fang_angle, 0));
        }
        for i in 0..8 {
            let fang_angle =
                angle + f64::from(i) * std::f64::consts::TAU / 8.0 + std::f64::consts::TAU / 5.0;
            fangs.push(fang(2.5, fang_angle, 3));
        }
        fangs
    } else {
        (0..16)
            .map(|i| fang(1.25 * f64::from(i + 1), angle, i))
            .collect()
    }
}

fn plan_evoker_transition(
    hostile: &HostileAttackTickEntity,
    targets: &[HostileTargetTickSession],
    fangs_entity_type_id: Option<i32>,
    tick: u64,
) -> Option<PlannedEvokerTransition> {
    let expected = hostile.evoker_attack;
    let target_valid = |target: &HostileTargetTickSession| {
        target.visible_entities.contains(&hostile.id)
            && distance_sq(hostile.position, target.position)
                <= EVOKER_FANGS_RANGE * EVOKER_FANGS_RANGE
    };
    match expected {
        None => {
            let target = targets
                .iter()
                .filter(|target| target_valid(target))
                .min_by(|left, right| {
                    distance_sq(hostile.position, left.position)
                        .total_cmp(&distance_sq(hostile.position, right.position))
                })?;
            Some(PlannedEvokerTransition {
                hostile_id: hostile.id,
                expected,
                next: Some(EntityEvokerAttackState::new(
                    EntityEvokerAttackPhase::Warmup,
                    target.id,
                    target.entity_id,
                    tick.saturating_add(EVOKER_FANGS_WARMUP_TICKS),
                )),
                fangs: Vec::new(),
            })
        }
        Some(state) if state.phase == EntityEvokerAttackPhase::Warmup => {
            let target = targets.iter().find(|target| {
                target.id == state.target_session
                    && target.entity_id == state.target_entity_id
                    && target_valid(target)
            });
            let Some(target) = target else {
                return Some(PlannedEvokerTransition {
                    hostile_id: hostile.id,
                    expected,
                    next: None,
                    fangs: Vec::new(),
                });
            };
            if tick < state.deadline_tick {
                return None;
            }
            let entity_type_id = fangs_entity_type_id?;
            Some(PlannedEvokerTransition {
                hostile_id: hostile.id,
                expected,
                next: Some(EntityEvokerAttackState::new(
                    EntityEvokerAttackPhase::Casting,
                    state.target_session,
                    state.target_entity_id,
                    tick.saturating_add(
                        EVOKER_FANGS_CASTING_TICKS.saturating_sub(EVOKER_FANGS_WARMUP_TICKS),
                    ),
                )),
                fangs: plan_evoker_fang_pattern(hostile, target, entity_type_id),
            })
        }
        Some(state) if state.phase == EntityEvokerAttackPhase::Casting => {
            if tick < state.deadline_tick {
                return None;
            }
            Some(PlannedEvokerTransition {
                hostile_id: hostile.id,
                expected,
                next: Some(EntityEvokerAttackState::new(
                    EntityEvokerAttackPhase::Cooldown,
                    state.target_session,
                    state.target_entity_id,
                    tick.saturating_add(
                        EVOKER_FANGS_INTERVAL_TICKS.saturating_sub(EVOKER_FANGS_CASTING_TICKS),
                    ),
                )),
                fangs: Vec::new(),
            })
        }
        Some(state) => {
            if tick < state.deadline_tick {
                return None;
            }
            Some(PlannedEvokerTransition {
                hostile_id: hostile.id,
                expected,
                next: None,
                fangs: Vec::new(),
            })
        }
    }
}

fn plan_shulker_transition(
    hostile: &HostileAttackTickEntity,
    targets: &[HostileTargetTickSession],
    bullet_entity_type_id: Option<i32>,
    tick: u64,
) -> Option<PlannedShulkerTransition> {
    let expected = hostile.shulker_attack;
    let target = targets
        .iter()
        .filter(|target| target.visible_entities.contains(&hostile.id))
        .map(|target| (distance_sq(hostile.position, target.position), target))
        .filter(|(distance, _)| *distance < SHULKER_ATTACK_RANGE_SQ)
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, target)| target);
    let Some(target) = target else {
        return expected.map(|_| PlannedShulkerTransition {
            hostile_id: hostile.id,
            expected,
            next: None,
            shot: None,
        });
    };
    match expected {
        None => Some(PlannedShulkerTransition {
            hostile_id: hostile.id,
            expected,
            next: Some(EntityShulkerAttackState::new(
                tick.saturating_add(SHULKER_INITIAL_SHOT_DELAY_TICKS),
            )),
            shot: None,
        }),
        Some(state) if tick < state.deadline_tick => None,
        Some(_) => {
            let entity_type_id = bullet_entity_type_id?;
            Some(PlannedShulkerTransition {
                hostile_id: hostile.id,
                expected,
                next: Some(EntityShulkerAttackState::new(tick.saturating_add(
                    deterministic_shulker_delay_ticks(hostile.id, tick),
                ))),
                shot: Some(PlannedShulkerBulletAttack {
                    hostile_id: hostile.id,
                    entity_type_id,
                    target_entity_id: target.entity_id,
                    position: Vec3::new(
                        hostile.position.x,
                        hostile.position.y + 0.5,
                        hostile.position.z,
                    ),
                    rotation: hostile.rotation,
                }),
            })
        }
    }
}

fn plan_hostile_arrow(
    hostile: &HostileAttackTickEntity,
    target_position: Vec3,
    arrow_entity_type_id: i32,
    crossbow: bool,
) -> Option<PlannedArrowAttack> {
    let shooter_eye = Vec3::new(
        hostile.position.x,
        hostile.position.y + 1.5,
        hostile.position.z,
    );
    let dx = target_position.x - shooter_eye.x;
    let dz = target_position.z - shooter_eye.z;
    let horizontal_distance = dx.hypot(dz);
    let dy = if crossbow {
        target_position.y + PLAYER_CROSSBOW_TARGET_Y_OFFSET - shooter_eye.y
            + horizontal_distance * 0.2
    } else {
        target_position.y + 1.0 - shooter_eye.y
    };
    let length = (dx * dx + dy * dy + dz * dz).sqrt();
    if length <= f64::EPSILON {
        return None;
    }
    let direction = Vec3::new(dx / length, dy / length, dz / length);
    let velocity = Vec3::new(
        direction.x * SKELETON_ARROW_SPEED,
        direction.y * SKELETON_ARROW_SPEED,
        direction.z * SKELETON_ARROW_SPEED,
    );
    let position = Vec3::new(
        shooter_eye.x + direction.x * 0.7,
        shooter_eye.y + direction.y * 0.7,
        shooter_eye.z + direction.z * 0.7,
    );
    let horizontal = velocity.x.hypot(velocity.z);
    let yaw = velocity.z.atan2(velocity.x).to_degrees() as f32 - 90.0;
    let pitch = (-velocity.y).atan2(horizontal).to_degrees() as f32;
    Some(PlannedArrowAttack {
        hostile_id: hostile.id,
        arrow_entity_type_id,
        position,
        velocity,
        rotation: Rotation {
            yaw,
            pitch,
            head_yaw: yaw,
        },
        animate_shooter: !crossbow,
    })
}

fn plan_hostile_small_fireball(
    hostile: &HostileAttackTickEntity,
    target_position: Vec3,
    entity_type_id: i32,
) -> Option<PlannedSmallFireballAttack> {
    let position = Vec3::new(
        hostile.position.x,
        hostile.position.y + BLAZE_PROJECTILE_Y_OFFSET,
        hostile.position.z,
    );
    let direction = Vec3::new(
        target_position.x - position.x,
        target_position.y + BLAZE_TARGET_Y_OFFSET - position.y,
        target_position.z - position.z,
    );
    let length_squared =
        direction.x * direction.x + direction.y * direction.y + direction.z * direction.z;
    if !length_squared.is_finite() || length_squared <= f64::EPSILON {
        return None;
    }
    let horizontal = direction.x.hypot(direction.z);
    let yaw = direction.z.atan2(direction.x).to_degrees() as f32 - 90.0;
    let pitch = (-direction.y).atan2(horizontal).to_degrees() as f32;
    if !yaw.is_finite() || !pitch.is_finite() {
        return None;
    }
    Some(PlannedSmallFireballAttack {
        hostile_id: hostile.id,
        entity_type_id,
        position,
        direction,
        rotation: Rotation {
            yaw,
            pitch,
            head_yaw: yaw,
        },
    })
}

fn plan_ghast_transition(
    hostile: &HostileAttackTickEntity,
    targets: &[HostileTargetTickSession],
    fireball_entity_type_id: Option<i32>,
    air: BlockStateId,
) -> Option<PlannedGhastTransition> {
    let expected = hostile.ghast_attack;
    let target = targets
        .iter()
        .filter(|target| target.visible_entities.contains(&hostile.id))
        .map(|target| (distance_sq(hostile.position, target.position), target))
        .filter(|(distance, _)| *distance < GHAST_FIREBALL_RANGE_SQ)
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, target)| target);
    let Some(target) = target else {
        return expected.map(|_| PlannedGhastTransition {
            hostile_id: hostile.id,
            expected,
            next: None,
            shot: None,
        });
    };

    let charge_time = expected.map_or(0, |state| state.charge_time) + 1;
    if charge_time != GHAST_FIREBALL_SHOT_CHARGE {
        return Some(PlannedGhastTransition {
            hostile_id: hostile.id,
            expected,
            next: Some(EntityGhastAttackState::new(charge_time)),
            shot: None,
        });
    }

    let entity_type_id = fireball_entity_type_id?;
    let dx = target.position.x - hostile.position.x;
    let dz = target.position.z - hostile.position.z;
    let horizontal = dx.hypot(dz);
    if !horizontal.is_finite() || horizontal <= 1.0e-9 {
        return None;
    }
    let spawn = Vec3::new(
        hostile.position.x + dx / horizontal * GHAST_FIREBALL_SPAWN_OFFSET,
        hostile.position.y + GHAST_FIREBALL_Y_OFFSET,
        hostile.position.z + dz / horizontal * GHAST_FIREBALL_SPAWN_OFFSET,
    );
    let target_center = Vec3::new(
        target.position.x,
        target.position.y + 0.9,
        target.position.z,
    );
    let direction = Vec3::new(
        target_center.x - spawn.x,
        target_center.y - spawn.y,
        target_center.z - spawn.z,
    );
    Some(PlannedGhastTransition {
        hostile_id: hostile.id,
        expected,
        next: Some(EntityGhastAttackState::new(GHAST_FIREBALL_RESET_CHARGE)),
        shot: Some(PlannedLargeFireballAttack {
            hostile_id: hostile.id,
            entity_type_id,
            position: spawn,
            direction,
            rotation: hostile.rotation,
            air_block_state: air.0,
        }),
    })
}

fn plan_witch_transition(
    hostile: &HostileAttackTickEntity,
    targets: &[HostileTargetTickSession],
    potion_entity_type_id: Option<i32>,
    tick: u64,
) -> Option<PlannedWitchTransition> {
    let expected = hostile.witch_attack;
    let target_valid = |target: &HostileTargetTickSession| {
        target.visible_entities.contains(&hostile.id)
            && distance_sq(hostile.position, target.position) <= WITCH_ATTACK_RANGE_SQ
    };
    let target = match expected {
        Some(state) => targets.iter().find(|target| {
            target.id == state.target_session
                && target.entity_id == state.target_entity_id
                && target_valid(target)
        }),
        None => targets
            .iter()
            .filter(|target| target_valid(target))
            .min_by(|left, right| {
                distance_sq(hostile.position, left.position)
                    .total_cmp(&distance_sq(hostile.position, right.position))
            }),
    };
    let Some(target) = target else {
        return expected.map(|_| PlannedWitchTransition {
            hostile_id: hostile.id,
            expected,
            next: None,
            shot: None,
        });
    };
    if expected.is_none() {
        return Some(PlannedWitchTransition {
            hostile_id: hostile.id,
            expected,
            next: Some(EntityWitchAttackState::new(
                target.id,
                target.entity_id,
                tick.saturating_add(WITCH_ATTACK_INTERVAL_TICKS),
            )),
            shot: None,
        });
    }
    let state = expected.expect("checked witch attack state");
    if tick < state.deadline_tick {
        return None;
    }
    let entity_type_id = potion_entity_type_id?;
    let dx = target.position.x - hostile.position.x;
    let dz = target.position.z - hostile.position.z;
    let horizontal = dx.hypot(dz);
    let dy = target.position.y + 1.62 - WITCH_PROJECTILE_Y_OFFSET - hostile.position.y
        + horizontal * 0.2;
    let length = (dx * dx + dy * dy + dz * dz).sqrt();
    if !length.is_finite() || length <= f64::EPSILON {
        return None;
    }
    let speed = if horizontal <= 2.0 {
        WITCH_POTION_SPEED_NEAR
    } else {
        WITCH_POTION_SPEED_FAR
    };
    let velocity = Vec3::new(
        dx / length * speed,
        dy / length * speed,
        dz / length * speed,
    );
    let facts = target.effect_facts;
    let potion = if horizontal >= 8.0 && !facts.is_some_and(|facts| facts.has_slowness) {
        EntityWitchPotionKind::Slowness
    } else if facts.is_some_and(|facts| facts.health >= 8.0 && !facts.has_poison) {
        EntityWitchPotionKind::Poison
    } else {
        // Vanilla has an additional 25% Weakness branch at <=3 blocks. Keep
        // that RNG-owned branch explicit rather than inventing a new random stream.
        EntityWitchPotionKind::Harming
    };
    Some(PlannedWitchTransition {
        hostile_id: hostile.id,
        expected,
        next: Some(EntityWitchAttackState::new(
            target.id,
            target.entity_id,
            tick.saturating_add(WITCH_ATTACK_INTERVAL_TICKS),
        )),
        shot: Some(PlannedThrownPotionAttack {
            hostile_id: hostile.id,
            entity_type_id,
            position: Vec3::new(
                hostile.position.x,
                hostile.position.y + WITCH_PROJECTILE_Y_OFFSET,
                hostile.position.z,
            ),
            velocity,
            rotation: hostile.rotation,
            potion,
        }),
    })
}

fn plan_wither_skull_attack(
    hostile: &HostileAttackTickEntity,
    targets: &[HostileTargetTickSession],
    wither_skull_entity_type_id: Option<i32>,
    air: BlockStateId,
) -> Option<PlannedWitherSkullAttack> {
    let entity_type_id = wither_skull_entity_type_id?;
    let max_distance_sq = WITHER_SKULL_RANGE * WITHER_SKULL_RANGE;
    let target = targets
        .iter()
        .filter(|target| target.visible_entities.contains(&hostile.id))
        .map(|target| (distance_sq(hostile.position, target.position), target))
        .filter(|(distance, _)| *distance <= max_distance_sq)
        .min_by(|left, right| left.0.total_cmp(&right.0))?
        .1;
    let position = Vec3::new(
        hostile.position.x,
        hostile.position.y + WITHER_SKULL_HEAD_Y_OFFSET,
        hostile.position.z,
    );
    let target_center = Vec3::new(
        target.position.x,
        target.position.y + 0.81,
        target.position.z,
    );
    let direction = Vec3::new(
        target_center.x - position.x,
        target_center.y - position.y,
        target_center.z - position.z,
    );
    Some(PlannedWitherSkullAttack {
        hostile_id: hostile.id,
        entity_type_id,
        position,
        direction,
        rotation: hostile.rotation,
        air_block_state: air.0,
    })
}

fn plan_breeze_transition(
    hostile: &HostileAttackTickEntity,
    targets: &[HostileTargetTickSession],
    wind_charge_entity_type_id: Option<i32>,
    air: BlockStateId,
    tick: u64,
) -> Option<PlannedBreezeTransition> {
    let expected = hostile.breeze_attack;
    let target_valid = |target: &HostileTargetTickSession| {
        target.visible_entities.contains(&hostile.id)
            && distance_sq(hostile.position, target.position) < BREEZE_WIND_CHARGE_RANGE_SQ
    };
    match expected {
        None => {
            let target = targets
                .iter()
                .filter(|target| target_valid(target))
                .min_by(|left, right| {
                    distance_sq(hostile.position, left.position)
                        .total_cmp(&distance_sq(hostile.position, right.position))
                })?;
            Some(PlannedBreezeTransition {
                hostile_id: hostile.id,
                expected,
                next: Some(EntityBreezeAttackState::new(
                    EntityBreezeAttackPhase::Charging,
                    target.id,
                    target.entity_id,
                    tick.saturating_add(BREEZE_WIND_CHARGE_INITIAL_DELAY_TICKS),
                )),
                shot: None,
            })
        }
        Some(state) if state.phase == EntityBreezeAttackPhase::Charging => {
            let target = targets.iter().find(|target| {
                target.id == state.target_session
                    && target.entity_id == state.target_entity_id
                    && target_valid(target)
            });
            let Some(target) = target else {
                return Some(PlannedBreezeTransition {
                    hostile_id: hostile.id,
                    expected,
                    next: None,
                    shot: None,
                });
            };
            if tick < state.deadline_tick {
                return None;
            }
            let entity_type_id = wind_charge_entity_type_id?;
            let breeze_height = entity_aabb("minecraft:breeze").height;
            let position = Vec3::new(
                hostile.position.x,
                hostile.position.y + breeze_height / 2.0 + BREEZE_WIND_CHARGE_FIRING_Y_OFFSET,
                hostile.position.z,
            );
            let target_y = target.position.y + 1.8 * BREEZE_WIND_CHARGE_TARGET_Y_FRACTION;
            let dx = target.position.x - position.x;
            let dy = target_y - position.y;
            let dz = target.position.z - position.z;
            let length = (dx * dx + dy * dy + dz * dz).sqrt();
            if !length.is_finite() || length <= f64::EPSILON {
                return None;
            }
            let velocity = Vec3::new(
                dx / length * BREEZE_WIND_CHARGE_SPEED,
                dy / length * BREEZE_WIND_CHARGE_SPEED,
                dz / length * BREEZE_WIND_CHARGE_SPEED,
            );
            Some(PlannedBreezeTransition {
                hostile_id: hostile.id,
                expected,
                next: Some(EntityBreezeAttackState::new(
                    EntityBreezeAttackPhase::Recovery,
                    state.target_session,
                    state.target_entity_id,
                    tick.saturating_add(BREEZE_WIND_CHARGE_RECOVERY_TICKS),
                )),
                shot: Some(PlannedWindChargeAttack {
                    hostile_id: hostile.id,
                    entity_type_id,
                    position,
                    velocity,
                    rotation: hostile.rotation,
                    air_block_state: air.0,
                }),
            })
        }
        Some(state) if state.phase == EntityBreezeAttackPhase::Recovery => {
            if tick < state.deadline_tick {
                return None;
            }
            Some(PlannedBreezeTransition {
                hostile_id: hostile.id,
                expected,
                next: Some(EntityBreezeAttackState::new(
                    EntityBreezeAttackPhase::Cooldown,
                    state.target_session,
                    state.target_entity_id,
                    tick.saturating_add(BREEZE_WIND_CHARGE_COOLDOWN_TICKS),
                )),
                shot: None,
            })
        }
        Some(state) => {
            if tick < state.deadline_tick {
                return None;
            }
            Some(PlannedBreezeTransition {
                hostile_id: hostile.id,
                expected,
                next: None,
                shot: None,
            })
        }
    }
}

fn plan_blaze_transition(
    hostile: &HostileAttackTickEntity,
    target: Option<(f64, super::SessionId, Vec3)>,
    follow_range: f64,
    attack_damage: f32,
    small_fireball_entity_type_id: Option<i32>,
    tick: u64,
) -> Option<PlannedBlazeTransition> {
    let expected = hostile.blaze_attack;
    let Some((distance, target_session, target_position)) = target else {
        return expected.is_some().then_some(PlannedBlazeTransition {
            hostile_id: hostile.id,
            expected,
            next: None,
            shot: None,
            melee: None,
        });
    };
    let current = expected.unwrap_or_else(|| EntityBlazeAttackState::new(0, tick));
    if distance < BLAZE_CLOSE_MELEE_RANGE_SQ {
        if tick < current.deadline_tick {
            return None;
        }
        return Some(PlannedBlazeTransition {
            hostile_id: hostile.id,
            expected,
            next: Some(EntityBlazeAttackState::new(
                current.attack_step,
                tick.saturating_add(BLAZE_CLOSE_MELEE_PERIOD_TICKS),
            )),
            shot: None,
            melee: (attack_damage > 0.0).then_some(PlannedMeleeAttack {
                hostile_id: hostile.id,
                target_session,
                amount: attack_damage,
            }),
        });
    }
    if distance > follow_range * follow_range || tick < current.deadline_tick {
        return None;
    }
    let (next, shoot) = match current.attack_step {
        0 => (
            EntityBlazeAttackState::new(1, tick.saturating_add(BLAZE_CHARGE_TICKS)),
            false,
        ),
        1..=3 => (
            EntityBlazeAttackState::new(
                current.attack_step + 1,
                tick.saturating_add(BLAZE_SHOT_INTERVAL_TICKS),
            ),
            true,
        ),
        _ => (
            EntityBlazeAttackState::new(0, tick.saturating_add(BLAZE_BURST_COOLDOWN_TICKS)),
            false,
        ),
    };
    let shot = if shoot {
        small_fireball_entity_type_id.and_then(|entity_type_id| {
            plan_hostile_small_fireball(hostile, target_position, entity_type_id)
        })
    } else {
        None
    };
    Some(PlannedBlazeTransition {
        hostile_id: hostile.id,
        expected,
        next: Some(next),
        shot,
        melee: None,
    })
}

fn plan_crossbow_transition(
    hostile: &HostileAttackTickEntity,
    target: Option<(f64, Vec3)>,
    arrow_entity_type_id: Option<i32>,
    tick: u64,
) -> Option<PlannedCrossbowTransition> {
    let expected = hostile.crossbow_attack;
    let in_attack_range =
        target.is_some_and(|(distance, _)| distance <= PILLAGER_CROSSBOW_RANGE.powi(2));
    let (next, shot) = match expected {
        None if in_attack_range => (
            Some(EntityCrossbowAttackState::new(
                EntityCrossbowAttackPhase::Aiming,
                tick.saturating_add(PILLAGER_AIM_VISIBLE_TICKS.saturating_sub(1)),
            )),
            None,
        ),
        None => return None,
        Some(_) if target.is_none() => (None, None),
        Some(state) if state.phase == EntityCrossbowAttackPhase::Aiming => {
            if !in_attack_range || tick < state.deadline_tick {
                return None;
            }
            (
                Some(EntityCrossbowAttackState::new(
                    EntityCrossbowAttackPhase::Charging,
                    tick.saturating_add(PILLAGER_CROSSBOW_CHARGE_TICKS),
                )),
                None,
            )
        }
        Some(state) if state.phase == EntityCrossbowAttackPhase::Charging => {
            if tick < state.deadline_tick {
                return None;
            }
            (
                Some(EntityCrossbowAttackState::new(
                    EntityCrossbowAttackPhase::Charged,
                    tick.saturating_add(deterministic_crossbow_delay_ticks(hostile.id, tick)),
                )),
                None,
            )
        }
        Some(state) => {
            if tick < state.deadline_tick {
                return None;
            }
            let arrow_entity_type_id = arrow_entity_type_id?;
            let (_, target_position) = target?;
            let shot = plan_hostile_arrow(hostile, target_position, arrow_entity_type_id, true)?;
            (None, Some(shot))
        }
    };
    Some(PlannedCrossbowTransition {
        hostile_id: hostile.id,
        expected,
        next,
        shot,
    })
}

fn plan_warden_sonic_transition(
    hostile: &HostileAttackTickEntity,
    targets: &[HostileTargetTickSession],
    tick: u64,
) -> Option<PlannedWardenSonicTransition> {
    let expected = hostile.warden_sonic_boom;
    let target_valid = |target: &HostileTargetTickSession| {
        if !target.visible_entities.contains(&hostile.id) {
            return false;
        }
        let dx = target.position.x - hostile.position.x;
        let dz = target.position.z - hostile.position.z;
        let dy = (target.position.y - hostile.position.y).abs();
        dx * dx + dz * dz <= WARDEN_SONIC_RANGE_XZ * WARDEN_SONIC_RANGE_XZ
            && dy <= WARDEN_SONIC_RANGE_Y
    };
    match expected {
        None => {
            let target = targets
                .iter()
                .filter(|target| target_valid(target))
                .min_by(|left, right| {
                    distance_sq(hostile.position, left.position)
                        .total_cmp(&distance_sq(hostile.position, right.position))
                })?;
            Some(PlannedWardenSonicTransition {
                hostile_id: hostile.id,
                expected,
                next: Some(EntityWardenSonicBoomState::new(
                    EntityWardenSonicBoomPhase::Charging,
                    target.id,
                    target.entity_id,
                    tick.saturating_add(WARDEN_SONIC_CHARGE_TICKS),
                )),
                charge_started: true,
                attack: None,
            })
        }
        Some(state) if state.phase == EntityWardenSonicBoomPhase::Charging => {
            let target = targets.iter().find(|target| {
                target.id == state.target_session
                    && target.entity_id == state.target_entity_id
                    && target_valid(target)
            });
            let Some(target) = target else {
                return Some(PlannedWardenSonicTransition {
                    hostile_id: hostile.id,
                    expected,
                    next: None,
                    charge_started: false,
                    attack: None,
                });
            };
            if tick < state.deadline_tick {
                return None;
            }
            Some(PlannedWardenSonicTransition {
                hostile_id: hostile.id,
                expected,
                next: Some(EntityWardenSonicBoomState::new(
                    EntityWardenSonicBoomPhase::Recovery,
                    state.target_session,
                    state.target_entity_id,
                    tick.saturating_add(
                        WARDEN_SONIC_DURATION_TICKS.saturating_sub(WARDEN_SONIC_CHARGE_TICKS),
                    ),
                )),
                charge_started: false,
                attack: Some(PlannedWardenSonicAttack {
                    hostile_id: hostile.id,
                    target_session: target.id,
                }),
            })
        }
        Some(state) if state.phase == EntityWardenSonicBoomPhase::Recovery => {
            if tick < state.deadline_tick {
                return None;
            }
            Some(PlannedWardenSonicTransition {
                hostile_id: hostile.id,
                expected,
                next: Some(EntityWardenSonicBoomState::new(
                    EntityWardenSonicBoomPhase::Cooldown,
                    state.target_session,
                    state.target_entity_id,
                    tick.saturating_add(WARDEN_SONIC_COOLDOWN_TICKS),
                )),
                charge_started: false,
                attack: None,
            })
        }
        Some(state) => {
            if tick < state.deadline_tick {
                return None;
            }
            Some(PlannedWardenSonicTransition {
                hostile_id: hostile.id,
                expected,
                next: None,
                charge_started: false,
                attack: None,
            })
        }
    }
}

fn plan_guardian_beam_transition(
    hostile: &HostileAttackTickEntity,
    elder: bool,
    follow_range: f64,
    attack_damage: f32,
    targets: &[HostileTargetTickSession],
    tick: u64,
) -> Option<PlannedGuardianBeamTransition> {
    let expected = hostile.guardian_beam;
    let duration = if elder {
        ELDER_GUARDIAN_BEAM_DURATION_TICKS
    } else {
        GUARDIAN_BEAM_DURATION_TICKS
    };
    let target_valid = |target: &HostileTargetTickSession, continuing: bool| {
        if !target.visible_entities.contains(&hostile.id) {
            return false;
        }
        let distance = distance_sq(hostile.position, target.position);
        let beyond_minimum = distance > GUARDIAN_MIN_TARGET_DISTANCE_SQ;
        distance <= follow_range * follow_range && (beyond_minimum || (continuing && elder))
    };

    let (next, beam_started, attack) = match expected {
        None => {
            let target = targets
                .iter()
                .filter(|target| target_valid(target, false))
                .min_by(|left, right| {
                    distance_sq(hostile.position, left.position)
                        .total_cmp(&distance_sq(hostile.position, right.position))
                })?;
            (
                Some(EntityGuardianBeamState::new(
                    EntityGuardianBeamPhase::Warmup,
                    target.id,
                    target.entity_id,
                    tick.saturating_add(GUARDIAN_BEAM_WARMUP_TICKS),
                )),
                false,
                None,
            )
        }
        Some(state) => {
            let target = targets.iter().find(|target| {
                target.id == state.target_session
                    && target.entity_id == state.target_entity_id
                    && target_valid(target, true)
            });
            let Some(target) = target else {
                return Some(PlannedGuardianBeamTransition {
                    hostile_id: hostile.id,
                    expected,
                    next: None,
                    beam_started: false,
                    attack: None,
                });
            };
            match state.phase {
                EntityGuardianBeamPhase::Warmup if tick >= state.deadline_tick => (
                    Some(EntityGuardianBeamState::new(
                        EntityGuardianBeamPhase::Beam,
                        state.target_session,
                        state.target_entity_id,
                        tick.saturating_add(duration),
                    )),
                    true,
                    None,
                ),
                EntityGuardianBeamPhase::Beam if tick >= state.deadline_tick => (
                    None,
                    false,
                    Some(PlannedGuardianBeamAttack {
                        hostile_id: hostile.id,
                        target_session: target.id,
                        magic_damage: GUARDIAN_EASY_MAGIC_DAMAGE + if elder { 2.0 } else { 0.0 },
                        attack_damage,
                    }),
                ),
                _ if hostile.goal != GoalState::Idle => (Some(state), false, None),
                _ => return None,
            }
        }
    };
    Some(PlannedGuardianBeamTransition {
        hostile_id: hostile.id,
        expected,
        next,
        beam_started,
        attack,
    })
}

#[cfg(test)]
#[derive(Debug)]
pub(super) struct HostileScanProbe {
    pub(super) reached: std::sync::mpsc::Sender<()>,
    pub(super) resume: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
#[derive(Debug)]
pub(super) struct HostileCommitProbe {
    pub(super) reached: std::sync::mpsc::Sender<()>,
    pub(super) resume: std::sync::mpsc::Receiver<()>,
}

impl SessionRegistry {
    pub(in crate::play::session) fn reconcile_hostile_targets_after_live_session_change(&self) {
        loop {
            let (generation, player_positions) = {
                let inner = self.lock_inner("snapshot live players for hostile reconciliation");
                let generation = self.live_session_generation.load(Ordering::Acquire);
                let player_positions = inner
                    .sessions
                    .iter()
                    .filter(|(session_id, _)| {
                        !inner.dead_sessions.contains(session_id)
                            && !inner.spectator_sessions.contains(session_id)
                            && !inner.client_unloaded_sessions.contains(session_id)
                    })
                    .map(|(_, session)| Vec3::new(session.pose.x, session.pose.y, session.pose.z))
                    .collect::<Vec<_>>();
                (generation, player_positions)
            };
            #[cfg(test)]
            self.pause_before_hostile_reconciliation_for_test();
            let mob_behaviors = self.mob_behavior_table();
            let mut entities = self.lock_entities("reconcile hostiles after live session change");
            update_hostile_targets(&mut entities, &player_positions, None, &mob_behaviors);
            drop(entities);
            if self.live_session_generation.load(Ordering::Acquire) == generation {
                return;
            }
        }
    }

    #[cfg(test)]
    fn pause_before_hostile_reconciliation_for_test(&self) {
        let probe = self
            .hostile_reconcile_probe
            .lock()
            .expect("test lock poisoned")
            .take();
        if let Some(probe) = probe {
            probe.reached.send(()).expect("hostile reconcile receiver");
            probe.resume.recv().expect("hostile reconcile release");
        }
    }

    #[cfg(test)]
    fn pause_during_hostile_scan_for_test(&self) {
        let probe = self
            .hostile_scan_probe
            .lock()
            .expect("test lock poisoned")
            .take();
        if let Some(probe) = probe {
            probe.reached.send(()).expect("hostile scan receiver");
            probe.resume.recv().expect("hostile scan release");
        }
    }

    #[cfg(test)]
    fn pause_between_hostile_entity_and_session_commit_for_test(&self) {
        let probe = self
            .hostile_commit_probe
            .lock()
            .expect("test lock poisoned")
            .take();
        if let Some(probe) = probe {
            probe.reached.send(()).expect("hostile commit receiver");
            probe.resume.recv().expect("hostile commit release");
        }
    }

    #[cfg(test)]
    fn pause_before_hostile_session_publication_for_test(&self) {
        let probe = self
            .hostile_publication_probe
            .lock()
            .expect("test lock poisoned")
            .take();
        if let Some(probe) = probe {
            probe
                .reached
                .send(())
                .expect("hostile publication receiver");
            probe.resume.recv().expect("hostile publication release");
        }
    }

    #[cfg(test)]
    fn pause_after_hostile_target_snapshot_for_test(&self) {
        let probe = self
            .hostile_target_snapshot_probe
            .lock()
            .expect("test lock poisoned")
            .take();
        if let Some(probe) = probe {
            probe.reached.send(()).expect("hostile target receiver");
            probe.resume.recv().expect("hostile target release");
        }
    }

    fn tick_evoker_fangs(&self) -> Vec<VisibilityDispatch> {
        let active_ids = self.active_simulation_entities.load_full();
        if active_ids.is_empty() {
            return Vec::new();
        }
        let targets = self
            .movement_recipients
            .load_full()
            .values()
            .filter_map(|publication| {
                let (target, _) = publication.combat_target_snapshot()?;
                target.is_targetable().then_some(HostileTargetTickSession {
                    id: publication.id(),
                    entity_id: publication.entity_id(),
                    position: Vec3::new(target.pose().x, target.pose().y, target.pose().z),
                    visible_entities: Arc::new(HashSet::new()),
                    effect_facts: None,
                })
            })
            .collect::<Vec<_>>();
        let mut ids = {
            let entities = self.lock_entities("scan active evoker fangs");
            entities.prefetch(&active_ids);
            active_ids
                .iter()
                .copied()
                .filter(|&id| {
                    entities.snapshot(id).is_some_and(|snapshot| {
                        snapshot.type_name == "minecraft:evoker_fangs"
                            && snapshot.lifecycle == EntityLifecycle::Alive
                            && snapshot.retained.evoker_fangs.is_some()
                    })
                })
                .collect::<Vec<_>>()
        };
        if ids.is_empty() {
            return Vec::new();
        }
        ids.sort_unstable();
        let fang_ids = ids.iter().copied().collect::<HashSet<_>>();
        let fang_aabb = entity_aabb("minecraft:evoker_fangs");
        let horizontal_reach = fang_aabb.half_width + 0.2 + 0.3;
        let mut inner = self.lock_session_entities("tick evoker fangs");
        inner.entities.prefetch(&fang_ids);
        let mut dispatches = Vec::new();
        for id in ids {
            let Some(expected) = inner.entities.snapshot(id) else {
                continue;
            };
            if expected.type_name != "minecraft:evoker_fangs"
                || expected.lifecycle != EntityLifecycle::Alive
            {
                continue;
            }
            let Some(mut state) = expected.retained.evoker_fangs else {
                continue;
            };
            state.warmup_delay_ticks = state.warmup_delay_ticks.saturating_sub(1);
            let active = state.warmup_delay_ticks < 0;
            let damage_now = state.warmup_delay_ticks == -8;
            let event_now = active && !state.sent_spike_event;
            if event_now {
                state.sent_spike_event = true;
            }
            if active {
                state.life_ticks = state.life_ticks.saturating_sub(1);
            }
            if active && state.life_ticks < 0 {
                if let Some((_, removed)) = remove_server_entity_locked(&mut inner, id) {
                    dispatches.extend(removed);
                }
                continue;
            }
            let mut next = expected.clone();
            next.retained.evoker_fangs = Some(state);
            if !inner
                .entities
                .replace_snapshot_if_current(expected, next.clone())
            {
                continue;
            }
            if event_now {
                dispatches.extend(entity_event_dispatches_locked(
                    &inner,
                    id,
                    EVOKER_FANGS_EVENT,
                ));
            }
            if !damage_now {
                continue;
            }
            for target in &targets {
                if target.entity_id == state.owner_entity_id
                    || (target.position.x - next.position.x).abs() > horizontal_reach
                    || (target.position.z - next.position.z).abs() > horizontal_reach
                    || target.position.y > next.position.y + fang_aabb.height
                    || target.position.y + 1.8 < next.position.y
                {
                    continue;
                }
                for recipient in session_recipients(&inner, [target.id]) {
                    dispatches.push(VisibilityDispatch {
                        recipient,
                        command: OutboundCommand::DamagePlayer {
                            damage: PlayerDamageRequest {
                                kind: PlayerDamageKind::IndirectMagic,
                                amount: EVOKER_FANGS_DAMAGE,
                                source_origin: Some(next.position),
                            },
                        },
                    });
                }
            }
        }
        dispatches
    }

    pub(in crate::play) fn tick_hostile_attacks(
        &self,
        _authority: &SimulationAuthority,
        tick: u64,
        air: BlockStateId,
    ) -> (usize, Vec<VisibilityDispatch>) {
        let fang_dispatches = self.tick_evoker_fangs();
        let loaded_entity_ids = self.active_hostile_entities.load_full();
        if loaded_entity_ids.is_empty() {
            return (0, fang_dispatches);
        }
        let mob_behaviors = self.mob_behavior_table();
        let mut hostiles = Vec::new();
        {
            let entities = self.lock_entities("scan hostile attack candidates");
            #[cfg(test)]
            self.pause_during_hostile_scan_for_test();
            entities.visit_simulation_entities_for_ids(&loaded_entity_ids, |entity| {
                #[cfg(test)]
                self.hostile_entity_scan_visits
                    .fetch_add(1, Ordering::Relaxed);
                if entity.lifecycle != EntityLifecycle::Alive {
                    return;
                }
                let Some(profile) = mob_behaviors.get_by_name(entity.type_name) else {
                    return;
                };
                let kind = match profile.combat {
                    MobCombatPolicy::CreeperFuse => HostileAttackKind::Creeper,
                    MobCombatPolicy::Arrow => HostileAttackKind::Skeleton,
                    MobCombatPolicy::Crossbow => HostileAttackKind::Crossbow,
                    MobCombatPolicy::GuardianBeam => HostileAttackKind::GuardianBeam {
                        elder: entity.type_name == "minecraft:elder_guardian",
                        follow_range: entity
                            .attributes
                            .base(&AttributeKind::FollowRange)
                            .unwrap_or(16.0)
                            .clamp(1.0, 2_048.0),
                        attack_damage: entity
                            .attributes
                            .base(&AttributeKind::AttackDamage)
                            .filter(|damage| damage.is_finite() && *damage > 0.0)
                            .unwrap_or(if entity.type_name == "minecraft:elder_guardian" {
                                8.0
                            } else {
                                6.0
                            }) as f32,
                    },
                    MobCombatPolicy::SmallFireball => HostileAttackKind::SmallFireball {
                        follow_range: entity
                            .attributes
                            .base(&AttributeKind::FollowRange)
                            .unwrap_or(48.0)
                            .clamp(1.0, 2_048.0),
                        attack_damage: entity
                            .attributes
                            .base(&AttributeKind::AttackDamage)
                            .filter(|damage| damage.is_finite() && *damage > 0.0)
                            .unwrap_or(6.0) as f32,
                    },
                    MobCombatPolicy::SonicBoom => HostileAttackKind::SonicBoom,
                    MobCombatPolicy::ShulkerBullet => HostileAttackKind::ShulkerBullet,
                    MobCombatPolicy::EvokerFangs => HostileAttackKind::EvokerFangs,
                    MobCombatPolicy::LargeFireball => HostileAttackKind::LargeFireball,
                    MobCombatPolicy::WindCharge => HostileAttackKind::WindCharge,
                    MobCombatPolicy::ThrownPotion => HostileAttackKind::ThrownPotion,
                    MobCombatPolicy::WitherSkull => HostileAttackKind::WitherSkull,
                    // Ender Dragon owns movement/combat in dragon_authority; never
                    // fall through to the common hostile planner or generic melee.
                    MobCombatPolicy::DragonBoss => return,
                    MobCombatPolicy::Melee => HostileAttackKind::Melee {
                        attack_damage: entity
                            .attributes
                            .base(&AttributeKind::AttackDamage)
                            .unwrap_or(3.0) as f32,
                    },
                    MobCombatPolicy::None | MobCombatPolicy::UnsupportedSpecial => return,
                };
                let period = match kind {
                    HostileAttackKind::Creeper
                    | HostileAttackKind::Crossbow
                    | HostileAttackKind::GuardianBeam { .. }
                    | HostileAttackKind::SmallFireball { .. }
                    | HostileAttackKind::SonicBoom
                    | HostileAttackKind::ShulkerBullet
                    | HostileAttackKind::EvokerFangs
                    | HostileAttackKind::LargeFireball
                    | HostileAttackKind::WindCharge
                    | HostileAttackKind::ThrownPotion => 1,
                    HostileAttackKind::WitherSkull => WITHER_SKULL_SHOT_PERIOD_TICKS,
                    HostileAttackKind::Skeleton => SKELETON_SHOT_PERIOD_TICKS,
                    HostileAttackKind::Melee { .. } => HOSTILE_MELEE_PERIOD_TICKS,
                };
                let phase = u64::from(entity.id.0.unsigned_abs());
                if !tick.wrapping_add(phase).is_multiple_of(period) {
                    return;
                }
                hostiles.push(HostileAttackTickEntity {
                    id: entity.id,
                    kind,
                    position: entity.position,
                    rotation: entity.rotation,
                    goal: entity.goal.clone(),
                    crossbow_attack: entity.retained.crossbow_attack,
                    blaze_attack: entity.retained.blaze_attack,
                    ghast_attack: entity.retained.ghast_attack,
                    breeze_attack: entity.retained.breeze_attack,
                    guardian_beam: entity.retained.guardian_beam,
                    warden_sonic_boom: entity.retained.warden_sonic_boom,
                    shulker_attack: entity.retained.shulker_attack,
                    evoker_attack: entity.retained.evoker_attack,
                    witch_attack: entity.retained.witch_attack,
                });
            });
        }
        #[cfg(test)]
        self.hostile_attack_candidates
            .fetch_add(hostiles.len() as u64, Ordering::Relaxed);
        if hostiles.is_empty() {
            return (0, fang_dispatches);
        }
        let needs_witch_effect_facts = hostiles
            .iter()
            .any(|hostile| matches!(hostile.kind, HostileAttackKind::ThrownPotion));

        let targets = self
            .movement_recipients
            .load_full()
            .values()
            .filter_map(|publication| {
                let (target, visible_entities) = publication.combat_target_snapshot()?;
                target.is_targetable().then_some(HostileTargetTickSession {
                    id: publication.id(),
                    entity_id: publication.entity_id(),
                    position: Vec3::new(target.pose().x, target.pose().y, target.pose().z),
                    visible_entities,
                    effect_facts: needs_witch_effect_facts
                        .then(|| self.player_effect_facts(publication.id()))
                        .flatten(),
                })
            })
            .collect::<Vec<_>>();
        let arrow_entity_type_id = self.hostile_arrow_entity_type_id.load(Ordering::Acquire);
        let arrow_entity_type_id = (arrow_entity_type_id >= 0).then_some(arrow_entity_type_id);
        let small_fireball_entity_type_id = self
            .hostile_small_fireball_entity_type_id
            .load(Ordering::Acquire);
        let small_fireball_entity_type_id =
            (small_fireball_entity_type_id >= 0).then_some(small_fireball_entity_type_id);
        let fireball_entity_type_id = self.hostile_fireball_entity_type_id.load(Ordering::Acquire);
        let fireball_entity_type_id =
            (fireball_entity_type_id >= 0).then_some(fireball_entity_type_id);
        let breeze_wind_charge_entity_type_id = self
            .hostile_breeze_wind_charge_entity_type_id
            .load(Ordering::Acquire);
        let breeze_wind_charge_entity_type_id =
            (breeze_wind_charge_entity_type_id >= 0).then_some(breeze_wind_charge_entity_type_id);
        let wither_skull_entity_type_id = self
            .hostile_wither_skull_entity_type_id
            .load(Ordering::Acquire);
        let wither_skull_entity_type_id =
            (wither_skull_entity_type_id >= 0).then_some(wither_skull_entity_type_id);
        let splash_potion_entity_type_id = self
            .hostile_splash_potion_entity_type_id
            .load(Ordering::Acquire);
        let splash_potion_entity_type_id =
            (splash_potion_entity_type_id >= 0).then_some(splash_potion_entity_type_id);
        let shulker_bullet_entity_type_id = self
            .hostile_shulker_bullet_entity_type_id
            .load(Ordering::Acquire);
        let shulker_bullet_entity_type_id =
            (shulker_bullet_entity_type_id >= 0).then_some(shulker_bullet_entity_type_id);
        let evoker_fangs_entity_type_id = self
            .hostile_evoker_fangs_entity_type_id
            .load(Ordering::Acquire);
        let evoker_fangs_entity_type_id =
            (evoker_fangs_entity_type_id >= 0).then_some(evoker_fangs_entity_type_id);

        let mut creeper_fuses = Vec::new();
        let mut arrow_attacks = Vec::new();
        let mut small_fireball_attacks = Vec::new();
        let mut wither_skull_attacks = Vec::new();
        let mut blaze_transitions = Vec::new();
        let mut ghast_transitions = Vec::new();
        let mut breeze_transitions = Vec::new();
        let mut witch_transitions = Vec::new();
        let mut crossbow_transitions = Vec::new();
        let mut guardian_beam_transitions = Vec::new();
        let mut warden_sonic_transitions = Vec::new();
        let mut shulker_transitions = Vec::new();
        let mut evoker_transitions = Vec::new();
        let mut melee_attacks = Vec::new();
        for hostile in hostiles {
            match hostile.kind {
                HostileAttackKind::Creeper => {
                    let nearest_distance_sq = targets
                        .iter()
                        .filter(|target| target.visible_entities.contains(&hostile.id))
                        .map(|target| distance_sq(hostile.position, target.position))
                        .min_by(f64::total_cmp);
                    creeper_fuses.push(PlannedCreeperFuse {
                        hostile_id: hostile.id,
                        nearest_distance_sq,
                    });
                }
                HostileAttackKind::Skeleton => {
                    let Some(arrow_entity_type_id) = arrow_entity_type_id else {
                        continue;
                    };
                    let max_distance_sq = SKELETON_SHOT_RANGE * SKELETON_SHOT_RANGE;
                    let target = targets
                        .iter()
                        .filter_map(|target| {
                            if !target.visible_entities.contains(&hostile.id) {
                                return None;
                            }
                            let distance = distance_sq(hostile.position, target.position);
                            (distance <= max_distance_sq).then_some((distance, target.position))
                        })
                        .min_by(|left, right| left.0.total_cmp(&right.0));
                    let Some((_, target_position)) = target else {
                        continue;
                    };
                    if let Some(attack) =
                        plan_hostile_arrow(&hostile, target_position, arrow_entity_type_id, false)
                    {
                        arrow_attacks.push(attack);
                    }
                }
                HostileAttackKind::Crossbow => {
                    let target = targets
                        .iter()
                        .filter(|target| target.visible_entities.contains(&hostile.id))
                        .map(|target| {
                            (
                                distance_sq(hostile.position, target.position),
                                target.position,
                            )
                        })
                        .min_by(|left, right| left.0.total_cmp(&right.0));
                    if let Some(transition) =
                        plan_crossbow_transition(&hostile, target, arrow_entity_type_id, tick)
                    {
                        crossbow_transitions.push(transition);
                    }
                }
                HostileAttackKind::GuardianBeam {
                    elder,
                    follow_range,
                    attack_damage,
                } => {
                    if let Some(transition) = plan_guardian_beam_transition(
                        &hostile,
                        elder,
                        follow_range,
                        attack_damage,
                        &targets,
                        tick,
                    ) {
                        guardian_beam_transitions.push(transition);
                    }
                }
                HostileAttackKind::SmallFireball {
                    follow_range,
                    attack_damage,
                } => {
                    let target = targets
                        .iter()
                        .filter(|target| target.visible_entities.contains(&hostile.id))
                        .map(|target| {
                            (
                                distance_sq(hostile.position, target.position),
                                target.id,
                                target.position,
                            )
                        })
                        .min_by(|left, right| left.0.total_cmp(&right.0));
                    if let Some(transition) = plan_blaze_transition(
                        &hostile,
                        target,
                        follow_range,
                        attack_damage,
                        small_fireball_entity_type_id,
                        tick,
                    ) {
                        blaze_transitions.push(transition);
                    }
                }
                HostileAttackKind::SonicBoom => {
                    if let Some(transition) = plan_warden_sonic_transition(&hostile, &targets, tick)
                    {
                        warden_sonic_transitions.push(transition);
                    }
                }
                HostileAttackKind::ShulkerBullet => {
                    if let Some(transition) = plan_shulker_transition(
                        &hostile,
                        &targets,
                        shulker_bullet_entity_type_id,
                        tick,
                    ) {
                        shulker_transitions.push(transition);
                    }
                }
                HostileAttackKind::EvokerFangs => {
                    if let Some(transition) = plan_evoker_transition(
                        &hostile,
                        &targets,
                        evoker_fangs_entity_type_id,
                        tick,
                    ) {
                        evoker_transitions.push(transition);
                    }
                }
                HostileAttackKind::LargeFireball => {
                    if let Some(transition) =
                        plan_ghast_transition(&hostile, &targets, fireball_entity_type_id, air)
                    {
                        ghast_transitions.push(transition);
                    }
                }
                HostileAttackKind::WindCharge => {
                    if let Some(transition) = plan_breeze_transition(
                        &hostile,
                        &targets,
                        breeze_wind_charge_entity_type_id,
                        air,
                        tick,
                    ) {
                        breeze_transitions.push(transition);
                    }
                }
                HostileAttackKind::ThrownPotion => {
                    if let Some(transition) = plan_witch_transition(
                        &hostile,
                        &targets,
                        splash_potion_entity_type_id,
                        tick,
                    ) {
                        witch_transitions.push(transition);
                    }
                }
                HostileAttackKind::WitherSkull => {
                    if let Some(attack) = plan_wither_skull_attack(
                        &hostile,
                        &targets,
                        wither_skull_entity_type_id,
                        air,
                    ) {
                        wither_skull_attacks.push(attack);
                    }
                }
                HostileAttackKind::Melee {
                    attack_damage: amount,
                } => {
                    if amount <= 0.0 {
                        continue;
                    }
                    let max_distance_sq = HOSTILE_MELEE_RANGE * HOSTILE_MELEE_RANGE;
                    let target = targets
                        .iter()
                        .filter_map(|target| {
                            if !target.visible_entities.contains(&hostile.id)
                                || (target.position.y - hostile.position.y).abs()
                                    > HOSTILE_MELEE_VERTICAL_REACH
                            {
                                return None;
                            }
                            let dx = target.position.x - hostile.position.x;
                            let dz = target.position.z - hostile.position.z;
                            let distance = dx * dx + dz * dz;
                            (distance <= max_distance_sq
                                && hostile_faces_target(
                                    hostile.position,
                                    hostile.rotation,
                                    target.position,
                                ))
                            .then_some((distance, target.id))
                        })
                        .min_by(|left, right| left.0.total_cmp(&right.0));
                    let Some((_, recipient)) = target else {
                        continue;
                    };
                    melee_attacks.push(PlannedMeleeAttack {
                        hostile_id: hostile.id,
                        target_session: recipient,
                        amount,
                    });
                }
            }
        }

        let creeper_ignitions = if creeper_fuses.is_empty() {
            0
        } else {
            let creeper_ids = creeper_fuses
                .iter()
                .map(|plan| plan.hostile_id)
                .collect::<HashSet<_>>();
            let mut guards = self.lock_session_entities("commit hostile creeper fuses");
            guards.entities.prefetch(&creeper_ids);
            let mut ignitions = 0;
            for plan in creeper_fuses {
                let Some(expected) = guards.entities.snapshot(plan.hostile_id) else {
                    continue;
                };
                let previous_fuse = expected.retained.primed_tnt;
                let cancel_distance_sq = CREEPER_CANCEL_RANGE * CREEPER_CANCEL_RANGE;
                let trigger_distance_sq = CREEPER_TRIGGER_RANGE * CREEPER_TRIGGER_RANGE;
                let next_fuse = match (previous_fuse, plan.nearest_distance_sq) {
                    (None, Some(distance)) if distance < trigger_distance_sq => {
                        Some(EntityPrimedTntState {
                            expires_tick: tick.saturating_add(CREEPER_FUSE_TICKS),
                            air_block_state: air.0,
                        })
                    }
                    (Some(_), Some(distance)) if distance <= cancel_distance_sq => continue,
                    (Some(fuse), _) => {
                        let remaining = fuse.expires_tick.saturating_sub(tick);
                        let progress = CREEPER_FUSE_TICKS.saturating_sub(remaining);
                        (progress > 1).then_some(EntityPrimedTntState {
                            expires_tick: fuse.expires_tick.saturating_add(2),
                            air_block_state: fuse.air_block_state,
                        })
                    }
                    (None, _) => continue,
                };
                let mut next = expected.clone();
                next.retained.primed_tnt = next_fuse;
                if guards.entities.replace_snapshot_if_current(expected, next) {
                    schedule_primed_tnt_deadline_locked(
                        &mut guards,
                        plan.hostile_id,
                        next_fuse.map(|fuse| fuse.expires_tick),
                    );
                    if previous_fuse.is_none() {
                        ignitions += 1;
                    }
                }
            }
            ignitions
        };
        let (crossbow_state_updates, committed_crossbow_attacks) =
            if crossbow_transitions.is_empty() {
                (Vec::new(), Vec::new())
            } else {
                let crossbow_ids = crossbow_transitions
                    .iter()
                    .map(|transition| transition.hostile_id)
                    .collect::<HashSet<_>>();
                let mut entities = self.lock_entities("commit hostile crossbow states");
                entities.prefetch(&crossbow_ids);
                let mut updates = Vec::new();
                let mut attacks = Vec::new();
                for transition in crossbow_transitions {
                    let Some(expected) = entities.snapshot(transition.hostile_id) else {
                        continue;
                    };
                    if expected.lifecycle != EntityLifecycle::Alive
                        || expected.type_name != "minecraft:pillager"
                        || expected.retained.crossbow_attack != transition.expected
                    {
                        continue;
                    }
                    let previous_charging = transition
                        .expected
                        .is_some_and(EntityCrossbowAttackState::is_charging);
                    let next_charging = transition
                        .next
                        .is_some_and(EntityCrossbowAttackState::is_charging);
                    let mut next = expected.clone();
                    next.retained.crossbow_attack = transition.next;
                    if !entities.replace_snapshot_if_current(expected, next.clone()) {
                        continue;
                    }
                    if previous_charging != next_charging {
                        updates.push(next);
                    }
                    if let Some(attack) = transition.shot {
                        attacks.push(attack);
                    }
                }
                (updates, attacks)
            };
        arrow_attacks.extend(committed_crossbow_attacks);

        let (blaze_state_updates, committed_small_fireballs, committed_blaze_melee) =
            if blaze_transitions.is_empty() {
                (Vec::new(), Vec::new(), Vec::new())
            } else {
                let blaze_ids = blaze_transitions
                    .iter()
                    .map(|transition| transition.hostile_id)
                    .collect::<HashSet<_>>();
                let mut entities = self.lock_entities("commit hostile blaze attack states");
                entities.prefetch(&blaze_ids);
                let mut updates = Vec::new();
                let mut shots = Vec::new();
                let mut melee = Vec::new();
                for transition in blaze_transitions {
                    let Some(expected) = entities.snapshot(transition.hostile_id) else {
                        continue;
                    };
                    if expected.lifecycle != EntityLifecycle::Alive
                        || expected.type_name != "minecraft:blaze"
                        || expected.retained.blaze_attack != transition.expected
                    {
                        continue;
                    }
                    let previous_charged = transition
                        .expected
                        .is_some_and(EntityBlazeAttackState::is_charged);
                    let next_charged = transition
                        .next
                        .is_some_and(EntityBlazeAttackState::is_charged);
                    let mut next = expected.clone();
                    next.retained.blaze_attack = transition.next;
                    if !entities.replace_snapshot_if_current(expected, next.clone()) {
                        continue;
                    }
                    if previous_charged != next_charged {
                        updates.push(next);
                    }
                    if let Some(shot) = transition.shot {
                        shots.push(shot);
                    }
                    if let Some(attack) = transition.melee {
                        melee.push(attack);
                    }
                }
                (updates, shots, melee)
            };
        small_fireball_attacks.extend(committed_small_fireballs);
        melee_attacks.extend(committed_blaze_melee);

        let committed_large_fireballs = if ghast_transitions.is_empty() {
            Vec::new()
        } else {
            let ghast_ids = ghast_transitions
                .iter()
                .map(|transition| transition.hostile_id)
                .collect::<HashSet<_>>();
            let mut entities = self.lock_entities("commit hostile ghast attack states");
            entities.prefetch(&ghast_ids);
            let mut shots = Vec::new();
            for transition in ghast_transitions {
                let Some(expected) = entities.snapshot(transition.hostile_id) else {
                    continue;
                };
                if expected.lifecycle != EntityLifecycle::Alive
                    || expected.type_name != "minecraft:ghast"
                    || expected.retained.ghast_attack != transition.expected
                {
                    continue;
                }
                let mut next = expected.clone();
                next.retained.ghast_attack = transition.next;
                if !entities.replace_snapshot_if_current(expected, next) {
                    continue;
                }
                if let Some(shot) = transition.shot {
                    shots.push(shot);
                }
            }
            shots
        };

        let committed_wind_charges = if breeze_transitions.is_empty() {
            Vec::new()
        } else {
            let breeze_ids = breeze_transitions
                .iter()
                .map(|transition| transition.hostile_id)
                .collect::<HashSet<_>>();
            let mut entities = self.lock_entities("commit hostile breeze attack states");
            entities.prefetch(&breeze_ids);
            let mut shots = Vec::new();
            for transition in breeze_transitions {
                let Some(expected) = entities.snapshot(transition.hostile_id) else {
                    continue;
                };
                if expected.lifecycle != EntityLifecycle::Alive
                    || expected.type_name != "minecraft:breeze"
                    || expected.retained.breeze_attack != transition.expected
                {
                    continue;
                }
                let mut next = expected.clone();
                next.retained.breeze_attack = transition.next;
                if transition.next.is_some_and(|state| {
                    matches!(
                        state.phase,
                        EntityBreezeAttackPhase::Charging | EntityBreezeAttackPhase::Recovery
                    )
                }) {
                    next.goal = GoalState::Idle;
                }
                if !entities.replace_snapshot_if_current(expected, next) {
                    continue;
                }
                if let Some(shot) = transition.shot {
                    shots.push(shot);
                }
            }
            shots
        };

        let committed_witch_potions = if witch_transitions.is_empty() {
            Vec::new()
        } else {
            let witch_ids = witch_transitions
                .iter()
                .map(|transition| transition.hostile_id)
                .collect::<HashSet<_>>();
            let mut entities = self.lock_entities("commit hostile witch attack states");
            entities.prefetch(&witch_ids);
            let mut shots = Vec::new();
            for transition in witch_transitions {
                let Some(expected) = entities.snapshot(transition.hostile_id) else {
                    continue;
                };
                if expected.lifecycle != EntityLifecycle::Alive
                    || expected.type_name != "minecraft:witch"
                    || expected.retained.witch_attack != transition.expected
                {
                    continue;
                }
                let mut next = expected.clone();
                next.retained.witch_attack = transition.next;
                if !entities.replace_snapshot_if_current(expected, next) {
                    continue;
                }
                if let Some(shot) = transition.shot {
                    shots.push(shot);
                }
            }
            shots
        };

        let (guardian_state_updates, guardian_beam_started, committed_guardian_attacks) =
            if guardian_beam_transitions.is_empty() {
                (Vec::new(), HashSet::new(), Vec::new())
            } else {
                let guardian_ids = guardian_beam_transitions
                    .iter()
                    .map(|transition| transition.hostile_id)
                    .collect::<HashSet<_>>();
                let mut entities = self.lock_entities("commit hostile guardian beam states");
                entities.prefetch(&guardian_ids);
                let mut updates = Vec::new();
                let mut started = HashSet::new();
                let mut attacks = Vec::new();
                for transition in guardian_beam_transitions {
                    let Some(expected) = entities.snapshot(transition.hostile_id) else {
                        continue;
                    };
                    if expected.lifecycle != EntityLifecycle::Alive
                        || !matches!(
                            expected.type_name.as_str(),
                            "minecraft:guardian" | "minecraft:elder_guardian"
                        )
                        || expected.retained.guardian_beam != transition.expected
                    {
                        continue;
                    }
                    let previous_target = transition
                        .expected
                        .map_or(0, EntityGuardianBeamState::active_target_entity_id);
                    let next_target = transition
                        .next
                        .map_or(0, EntityGuardianBeamState::active_target_entity_id);
                    let mut next = expected.clone();
                    next.retained.guardian_beam = transition.next;
                    if transition.next.is_some() {
                        next.goal = GoalState::Idle;
                    }
                    if !entities.replace_snapshot_if_current(expected, next.clone()) {
                        continue;
                    }
                    if previous_target != next_target {
                        updates.push(next);
                    }
                    if transition.beam_started {
                        started.insert(transition.hostile_id);
                    }
                    if let Some(attack) = transition.attack {
                        attacks.push(attack);
                    }
                }
                (updates, started, attacks)
            };

        let (warden_charge_started, committed_warden_attacks) = if warden_sonic_transitions
            .is_empty()
        {
            (HashSet::new(), Vec::new())
        } else {
            let warden_ids = warden_sonic_transitions
                .iter()
                .map(|transition| transition.hostile_id)
                .collect::<HashSet<_>>();
            let mut entities = self.lock_entities("commit hostile warden sonic states");
            entities.prefetch(&warden_ids);
            let mut started = HashSet::new();
            let mut attacks = Vec::new();
            for transition in warden_sonic_transitions {
                let Some(expected) = entities.snapshot(transition.hostile_id) else {
                    continue;
                };
                if expected.lifecycle != EntityLifecycle::Alive
                    || expected.type_name != "minecraft:warden"
                    || expected.retained.warden_sonic_boom != transition.expected
                {
                    continue;
                }
                let mut next = expected.clone();
                next.retained.warden_sonic_boom = transition.next;
                if transition.next.is_some_and(|state| {
                    matches!(
                        state.phase,
                        EntityWardenSonicBoomPhase::Charging | EntityWardenSonicBoomPhase::Recovery
                    )
                }) {
                    next.goal = GoalState::Idle;
                }
                if !entities.replace_snapshot_if_current(expected, next) {
                    continue;
                }
                if transition.charge_started {
                    started.insert(transition.hostile_id);
                }
                if let Some(attack) = transition.attack {
                    attacks.push(attack);
                }
            }
            (started, attacks)
        };

        let committed_shulker_shots = if shulker_transitions.is_empty() {
            Vec::new()
        } else {
            let shulker_ids = shulker_transitions
                .iter()
                .map(|transition| transition.hostile_id)
                .collect::<HashSet<_>>();
            let mut entities = self.lock_entities("commit hostile shulker attack states");
            entities.prefetch(&shulker_ids);
            let mut shots = Vec::new();
            for transition in shulker_transitions {
                let Some(expected) = entities.snapshot(transition.hostile_id) else {
                    continue;
                };
                if expected.lifecycle != EntityLifecycle::Alive
                    || expected.type_name != "minecraft:shulker"
                    || expected.retained.shulker_attack != transition.expected
                {
                    continue;
                }
                let mut next = expected.clone();
                next.retained.shulker_attack = transition.next;
                if !entities.replace_snapshot_if_current(expected, next) {
                    continue;
                }
                if let Some(shot) = transition.shot {
                    shots.push(shot);
                }
            }
            shots
        };

        let committed_evoker_fangs = if evoker_transitions.is_empty() {
            Vec::new()
        } else {
            let evoker_ids = evoker_transitions
                .iter()
                .map(|transition| transition.hostile_id)
                .collect::<HashSet<_>>();
            let mut entities = self.lock_entities("commit hostile evoker attack states");
            entities.prefetch(&evoker_ids);
            let mut fangs = Vec::new();
            for transition in evoker_transitions {
                let Some(expected) = entities.snapshot(transition.hostile_id) else {
                    continue;
                };
                if expected.lifecycle != EntityLifecycle::Alive
                    || expected.type_name != "minecraft:evoker"
                    || expected.retained.evoker_attack != transition.expected
                {
                    continue;
                }
                let mut next = expected.clone();
                next.retained.evoker_attack = transition.next;
                if transition.next.is_some_and(|state| {
                    matches!(
                        state.phase,
                        EntityEvokerAttackPhase::Warmup | EntityEvokerAttackPhase::Casting
                    )
                }) {
                    next.goal = GoalState::Idle;
                }
                if !entities.replace_snapshot_if_current(expected, next) {
                    continue;
                }
                fangs.extend(transition.fangs);
            }
            fangs
        };

        let crossbow_state_updates = if crossbow_state_updates.is_empty() {
            Vec::new()
        } else {
            self.current_expected_entity_snapshots(crossbow_state_updates)
        };
        let blaze_state_updates = if blaze_state_updates.is_empty() {
            Vec::new()
        } else {
            self.current_expected_entity_snapshots(blaze_state_updates)
        };
        let guardian_state_updates = if guardian_state_updates.is_empty() {
            Vec::new()
        } else {
            self.current_expected_entity_snapshots(guardian_state_updates)
        };
        let mut dispatches = fang_dispatches;
        if !crossbow_state_updates.is_empty() {
            let mut inner = self.lock_inner("publish hostile crossbow state");
            for entity in crossbow_state_updates {
                let published = server_entity_snapshot_from(entity);
                let entity_id = published.id;
                inner
                    .published_entity_snapshots
                    .insert(entity_id, published.clone());
                let recipients =
                    session_recipients(&inner, visible_entity_observers_locked(&inner, entity_id));
                let updates = visibility_dispatches(recipients, || {
                    OutboundCommand::UpdateEntityData(published.clone())
                });
                record_entity_dispatches_locked(&mut inner, &updates);
                dispatches.extend(updates);
            }
        }
        if !blaze_state_updates.is_empty() {
            let mut inner = self.lock_inner("publish hostile blaze state");
            for entity in blaze_state_updates {
                let published = server_entity_snapshot_from(entity);
                let entity_id = published.id;
                inner
                    .published_entity_snapshots
                    .insert(entity_id, published.clone());
                let recipients =
                    session_recipients(&inner, visible_entity_observers_locked(&inner, entity_id));
                let updates = visibility_dispatches(recipients, || {
                    OutboundCommand::UpdateEntityData(published.clone())
                });
                record_entity_dispatches_locked(&mut inner, &updates);
                dispatches.extend(updates);
            }
        }
        if !guardian_state_updates.is_empty() {
            let mut inner = self.lock_inner("publish hostile guardian beam state");
            for entity in guardian_state_updates {
                let published = server_entity_snapshot_from(entity);
                let entity_id = published.id;
                inner
                    .published_entity_snapshots
                    .insert(entity_id, published.clone());
                let recipients =
                    session_recipients(&inner, visible_entity_observers_locked(&inner, entity_id));
                let updates = visibility_dispatches(recipients, || {
                    OutboundCommand::UpdateEntityData(published.clone())
                });
                record_entity_dispatches_locked(&mut inner, &updates);
                dispatches.extend(updates);
                if guardian_beam_started.contains(&entity_id) {
                    let events = entity_event_dispatches_locked(
                        &inner,
                        entity_id,
                        GUARDIAN_BEAM_START_EVENT,
                    );
                    record_entity_dispatches_locked(&mut inner, &events);
                    dispatches.extend(events);
                }
            }
        }
        if !warden_charge_started.is_empty() {
            let mut inner = self.lock_inner("publish hostile warden sonic charge");
            for entity_id in warden_charge_started {
                let events = entity_event_dispatches_locked(&inner, entity_id, WARDEN_SONIC_EVENT);
                record_entity_dispatches_locked(&mut inner, &events);
                dispatches.extend(events);
            }
        }

        if arrow_attacks.is_empty()
            && small_fireball_attacks.is_empty()
            && wither_skull_attacks.is_empty()
            && melee_attacks.is_empty()
            && committed_guardian_attacks.is_empty()
            && committed_warden_attacks.is_empty()
            && committed_shulker_shots.is_empty()
            && committed_evoker_fangs.is_empty()
            && committed_large_fireballs.is_empty()
            && committed_wind_charges.is_empty()
            && committed_witch_potions.is_empty()
        {
            return (creeper_ignitions, dispatches);
        }

        let spawned_arrows = if arrow_attacks.is_empty() {
            Vec::new()
        } else {
            let mut entities = self.lock_entities("spawn hostile arrows ECS");
            let mut hostile_sources = Vec::with_capacity(arrow_attacks.len());
            let mut arrows = Vec::with_capacity(arrow_attacks.len());
            for attack in arrow_attacks {
                let mut arrow = SpawnEntity::new(
                    attack.arrow_entity_type_id,
                    "minecraft:arrow",
                    attack.position,
                );
                arrow.retained.spawn_tick = tick;
                arrow.velocity = attack.velocity;
                arrow.rotation = attack.rotation;
                arrow.on_ground = false;
                apply_entity_facts(&mut arrow);
                hostile_sources.push((attack.hostile_id, attack.animate_shooter));
                arrows.push(arrow);
            }
            let arrow_ids = entities.spawn_batch(arrows);
            let arrow_id_set = arrow_ids.iter().copied().collect::<HashSet<_>>();
            entities.prefetch(&arrow_id_set);
            let mut spawned = Vec::with_capacity(arrow_ids.len());
            let mut transaction = Vec::with_capacity(arrow_ids.len());
            for ((hostile_id, animate_shooter), arrow_id) in
                hostile_sources.into_iter().zip(arrow_ids)
            {
                let Some(expected) = entities.snapshot(arrow_id) else {
                    continue;
                };
                let mut next = expected.clone();
                next.retained.arrow_state = Some(
                    initial_arrow_state(
                        Some(projectile_identity(hostile_id)),
                        expected.position,
                        expected.velocity,
                        expected.rotation,
                    )
                    .expect("finite hostile arrow must produce a valid kernel state"),
                );
                transaction.push((expected, next.clone()));
                spawned.push(SpawnedHostileArrow {
                    hostile_id,
                    snapshot: next,
                    animate_shooter,
                });
            }
            assert!(
                entities.replace_snapshots_if_current(transaction),
                "hostile arrows must retain owners before session publication"
            );
            spawned
        };
        let spawned_small_fireballs = if small_fireball_attacks.is_empty() {
            Vec::new()
        } else {
            let mut entities = self.lock_entities("spawn hostile small fireballs ECS");
            let mut fireballs = Vec::with_capacity(small_fireball_attacks.len());
            for attack in small_fireball_attacks {
                let state = initial_hurting_projectile_state(
                    Some(projectile_identity(attack.hostile_id)),
                    "minecraft:small_fireball",
                    attack.position,
                    attack.direction,
                    attack.rotation,
                )
                .expect("finite hostile small fireball must produce a valid kernel state");
                let mut fireball = SpawnEntity::new(
                    attack.entity_type_id,
                    "minecraft:small_fireball",
                    attack.position,
                );
                fireball.retained.spawn_tick = tick;
                fireball.velocity = Vec3::new(
                    state.projectile.velocity.x,
                    state.projectile.velocity.y,
                    state.projectile.velocity.z,
                );
                fireball.rotation = attack.rotation;
                fireball.on_ground = false;
                apply_entity_facts(&mut fireball);
                fireball.retained.hurting_projectile_state = Some(state);
                fireballs.push(fireball);
            }
            let ids = entities.spawn_batch(fireballs);
            let id_set = ids.iter().copied().collect::<HashSet<_>>();
            entities.prefetch(&id_set);
            ids.into_iter()
                .filter_map(|id| entities.snapshot(id))
                .collect::<Vec<_>>()
        };
        let spawned_large_fireballs = if committed_large_fireballs.is_empty() {
            Vec::new()
        } else {
            let mut entities = self.lock_entities("spawn hostile large fireballs ECS");
            let mut fireballs = Vec::with_capacity(committed_large_fireballs.len());
            for attack in committed_large_fireballs {
                let state = initial_hurting_projectile_state(
                    Some(projectile_identity(attack.hostile_id)),
                    "minecraft:fireball",
                    attack.position,
                    attack.direction,
                    attack.rotation,
                )
                .expect("finite hostile large fireball must produce a valid kernel state");
                let mut fireball =
                    SpawnEntity::new(attack.entity_type_id, "minecraft:fireball", attack.position);
                fireball.retained.spawn_tick = tick;
                fireball.velocity = Vec3::new(
                    state.projectile.velocity.x,
                    state.projectile.velocity.y,
                    state.projectile.velocity.z,
                );
                fireball.rotation = attack.rotation;
                fireball.on_ground = false;
                apply_entity_facts(&mut fireball);
                fireball.retained.hurting_projectile_state = Some(state);
                fireball.retained.pending_explosion = EntityPendingExplosionState::new(
                    u64::MAX,
                    GHAST_FIREBALL_EXPLOSION_POWER,
                    EntityExplosionInteraction::Mob,
                    true,
                    attack.air_block_state,
                );
                fireballs.push(fireball);
            }
            let ids = entities.spawn_batch(fireballs);
            let id_set = ids.iter().copied().collect::<HashSet<_>>();
            entities.prefetch(&id_set);
            ids.into_iter()
                .filter_map(|id| entities.snapshot(id))
                .collect::<Vec<_>>()
        };
        let spawned_wither_skulls = if wither_skull_attacks.is_empty() {
            Vec::new()
        } else {
            let mut entities = self.lock_entities("spawn hostile wither skulls ECS");
            let mut skulls = Vec::with_capacity(wither_skull_attacks.len());
            for attack in wither_skull_attacks {
                let state = initial_hurting_projectile_state(
                    Some(projectile_identity(attack.hostile_id)),
                    "minecraft:wither_skull",
                    attack.position,
                    attack.direction,
                    attack.rotation,
                )
                .expect("finite hostile wither skull must produce a valid kernel state");
                let mut skull = SpawnEntity::new(
                    attack.entity_type_id,
                    "minecraft:wither_skull",
                    attack.position,
                );
                skull.retained.spawn_tick = tick;
                skull.velocity = Vec3::new(
                    state.projectile.velocity.x,
                    state.projectile.velocity.y,
                    state.projectile.velocity.z,
                );
                skull.rotation = attack.rotation;
                skull.on_ground = false;
                apply_entity_facts(&mut skull);
                skull.retained.hurting_projectile_state = Some(state);
                skull.retained.pending_explosion = EntityPendingExplosionState::new(
                    u64::MAX,
                    WITHER_SKULL_EXPLOSION_POWER,
                    EntityExplosionInteraction::Mob,
                    true,
                    attack.air_block_state,
                );
                skulls.push(skull);
            }
            let ids = entities.spawn_batch(skulls);
            let id_set = ids.iter().copied().collect::<HashSet<_>>();
            entities.prefetch(&id_set);
            ids.into_iter()
                .filter_map(|id| entities.snapshot(id))
                .collect::<Vec<_>>()
        };
        let spawned_wind_charges = if committed_wind_charges.is_empty() {
            Vec::new()
        } else {
            let mut entities = self.lock_entities("spawn hostile breeze wind charges ECS");
            let mut projectiles = Vec::with_capacity(committed_wind_charges.len());
            for attack in committed_wind_charges {
                let state = initial_hurting_projectile_state_with_motion(
                    Some(projectile_identity(attack.hostile_id)),
                    "minecraft:breeze_wind_charge",
                    attack.position,
                    Vec3::ZERO,
                    attack.rotation,
                    HurtingProjectileMotionProfile {
                        acceleration_power: 0.0,
                        air_inertia: 1.0,
                        water_inertia: 1.0,
                    },
                )
                .expect("finite breeze wind charge must produce a valid kernel state")
                .retarget_velocity(mc_entity::projectile_26_1_2::Vec3::new(
                    attack.velocity.x,
                    attack.velocity.y,
                    attack.velocity.z,
                ))
                .expect("finite breeze wind charge velocity must retarget kernel state");
                let mut projectile = SpawnEntity::new(
                    attack.entity_type_id,
                    "minecraft:breeze_wind_charge",
                    attack.position,
                );
                projectile.retained.spawn_tick = tick;
                projectile.velocity = attack.velocity;
                projectile.rotation = Rotation {
                    yaw: state.projectile.rotation.yaw,
                    pitch: state.projectile.rotation.pitch,
                    head_yaw: state.projectile.rotation.yaw,
                };
                projectile.on_ground = false;
                apply_entity_facts(&mut projectile);
                projectile.retained.hurting_projectile_state = Some(state);
                projectile.retained.pending_explosion = EntityPendingExplosionState::new(
                    u64::MAX,
                    BREEZE_WIND_CHARGE_EXPLOSION_POWER,
                    EntityExplosionInteraction::Trigger,
                    false,
                    attack.air_block_state,
                );
                projectiles.push(projectile);
            }
            let ids = entities.spawn_batch(projectiles);
            let id_set = ids.iter().copied().collect::<HashSet<_>>();
            entities.prefetch(&id_set);
            ids.into_iter()
                .filter_map(|id| entities.snapshot(id))
                .collect::<Vec<_>>()
        };
        let spawned_witch_potions = if committed_witch_potions.is_empty() {
            Vec::new()
        } else {
            let mut entities = self.lock_entities("spawn hostile witch potions ECS");
            let mut potions = Vec::with_capacity(committed_witch_potions.len());
            for attack in committed_witch_potions {
                let state = initial_throwable_projectile_state(
                    Some(projectile_identity(attack.hostile_id)),
                    "minecraft:splash_potion",
                    attack.position,
                    attack.velocity,
                    attack.rotation,
                )
                .expect("finite witch splash potion must produce a valid throwable state");
                let mut potion = SpawnEntity::new(
                    attack.entity_type_id,
                    "minecraft:splash_potion",
                    attack.position,
                );
                potion.retained.spawn_tick = tick;
                potion.velocity = attack.velocity;
                potion.rotation = attack.rotation;
                potion.on_ground = false;
                apply_entity_facts(&mut potion);
                potion.retained.throwable_projectile_state = Some(state);
                potion.retained.witch_potion = Some(attack.potion);
                potions.push(potion);
            }
            let ids = entities.spawn_batch(potions);
            let id_set = ids.iter().copied().collect::<HashSet<_>>();
            entities.prefetch(&id_set);
            ids.into_iter()
                .filter_map(|id| entities.snapshot(id))
                .collect::<Vec<_>>()
        };
        let spawned_shulker_bullets = if committed_shulker_shots.is_empty() {
            Vec::new()
        } else {
            let mut entities = self.lock_entities("spawn hostile shulker bullets ECS");
            let mut bullets = Vec::with_capacity(committed_shulker_shots.len());
            for attack in committed_shulker_shots {
                let state = initial_hurting_projectile_state_with_motion(
                    Some(projectile_identity(attack.hostile_id)),
                    "minecraft:shulker_bullet",
                    attack.position,
                    Vec3::ZERO,
                    attack.rotation,
                    HurtingProjectileMotionProfile {
                        acceleration_power: 0.0,
                        air_inertia: 1.0,
                        water_inertia: 1.0,
                    },
                )
                .expect("finite hostile shulker bullet must produce a valid kernel state");
                let mut bullet = SpawnEntity::new(
                    attack.entity_type_id,
                    "minecraft:shulker_bullet",
                    attack.position,
                );
                bullet.retained.spawn_tick = tick;
                bullet.rotation = attack.rotation;
                bullet.on_ground = false;
                apply_entity_facts(&mut bullet);
                bullet.retained.hurting_projectile_state = Some(state);
                bullet.retained.shulker_bullet = Some(mc_entity::EntityShulkerBulletState::new(
                    attack.target_entity_id,
                ));
                bullets.push(bullet);
            }
            let ids = entities.spawn_batch(bullets);
            let id_set = ids.iter().copied().collect::<HashSet<_>>();
            entities.prefetch(&id_set);
            ids.into_iter()
                .filter_map(|id| entities.snapshot(id))
                .collect::<Vec<_>>()
        };
        let spawned_evoker_fangs = if committed_evoker_fangs.is_empty() {
            Vec::new()
        } else {
            let mut entities = self.lock_entities("spawn hostile evoker fangs ECS");
            let mut fangs = Vec::with_capacity(committed_evoker_fangs.len());
            for fang in committed_evoker_fangs {
                let mut entity =
                    SpawnEntity::new(fang.entity_type_id, "minecraft:evoker_fangs", fang.position);
                entity.retained.spawn_tick = tick;
                entity.rotation = fang.rotation;
                entity.on_ground = true;
                apply_entity_facts(&mut entity);
                entity.retained.evoker_fangs = Some(EntityEvokerFangState::new(
                    fang.owner_id.0,
                    fang.warmup_delay_ticks,
                ));
                fangs.push(entity);
            }
            let ids = entities.spawn_batch(fangs);
            let id_set = ids.iter().copied().collect::<HashSet<_>>();
            entities.prefetch(&id_set);
            ids.into_iter()
                .filter_map(|id| entities.snapshot(id))
                .collect::<Vec<_>>()
        };
        #[cfg(test)]
        self.pause_between_hostile_entity_and_session_commit_for_test();

        let mut attacks = 0;
        let source_by_arrow = spawned_arrows
            .iter()
            .map(|arrow| (arrow.snapshot.id, (arrow.hostile_id, arrow.animate_shooter)))
            .collect::<HashMap<_, _>>();
        let spawned_arrows = self
            .current_expected_entity_snapshots(
                spawned_arrows.into_iter().map(|arrow| arrow.snapshot),
            )
            .into_iter()
            .map(|snapshot| {
                let (hostile_id, animate_shooter) = source_by_arrow[&snapshot.id];
                SpawnedHostileArrow {
                    hostile_id,
                    snapshot,
                    animate_shooter,
                }
            })
            .collect::<Vec<_>>();
        let spawned_small_fireballs = if spawned_small_fireballs.is_empty() {
            Vec::new()
        } else {
            self.current_expected_entity_snapshots(spawned_small_fireballs)
        };
        let spawned_large_fireballs = if spawned_large_fireballs.is_empty() {
            Vec::new()
        } else {
            self.current_expected_entity_snapshots(spawned_large_fireballs)
        };
        let spawned_wither_skulls = if spawned_wither_skulls.is_empty() {
            Vec::new()
        } else {
            self.current_expected_entity_snapshots(spawned_wither_skulls)
        };
        let spawned_wind_charges = if spawned_wind_charges.is_empty() {
            Vec::new()
        } else {
            self.current_expected_entity_snapshots(spawned_wind_charges)
        };
        let spawned_witch_potions = if spawned_witch_potions.is_empty() {
            Vec::new()
        } else {
            self.current_expected_entity_snapshots(spawned_witch_potions)
        };
        let spawned_shulker_bullets = if spawned_shulker_bullets.is_empty() {
            Vec::new()
        } else {
            self.current_expected_entity_snapshots(spawned_shulker_bullets)
        };
        let spawned_evoker_fangs = if spawned_evoker_fangs.is_empty() {
            Vec::new()
        } else {
            self.current_expected_entity_snapshots(spawned_evoker_fangs)
        };
        if !spawned_arrows.is_empty() {
            let mut inner = self.lock_inner("publish hostile arrows");
            for arrow in spawned_arrows {
                let snapshot = server_entity_snapshot_from(arrow.snapshot);
                let arrow_id = snapshot.id;
                let arrow_position = snapshot.position;
                let arrow_type_id = snapshot.type_id;
                inner
                    .entity_type_aabbs
                    .entry(arrow_type_id)
                    .or_insert_with(|| entity_aabb(&snapshot.type_name));
                track_entity_chunk_locked(&mut inner, arrow_id, arrow_position);
                initialize_entity_wire_state_from_snapshot_locked(&mut inner, &snapshot);
                dispatches.extend(spawn_entity_visibility_from_snapshot_locked(
                    &mut inner, snapshot,
                ));
                if arrow.animate_shooter {
                    let animation_recipients = session_recipients(
                        &inner,
                        visible_entity_observers_locked(&inner, arrow.hostile_id),
                    );
                    dispatches.extend(visibility_dispatches(animation_recipients, || {
                        OutboundCommand::AnimatePlayer {
                            entity_id: arrow.hostile_id.0,
                        }
                    }));
                }
                attacks += 1;
            }
        }
        if !spawned_small_fireballs.is_empty() {
            let mut inner = self.lock_inner("publish hostile small fireballs");
            for fireball in spawned_small_fireballs {
                let snapshot = server_entity_snapshot_from(fireball);
                let fireball_id = snapshot.id;
                let fireball_position = snapshot.position;
                let fireball_type_id = snapshot.type_id;
                inner
                    .entity_type_aabbs
                    .entry(fireball_type_id)
                    .or_insert_with(|| entity_aabb(&snapshot.type_name));
                track_entity_chunk_locked(&mut inner, fireball_id, fireball_position);
                initialize_entity_wire_state_from_snapshot_locked(&mut inner, &snapshot);
                dispatches.extend(spawn_entity_visibility_from_snapshot_locked(
                    &mut inner, snapshot,
                ));
                attacks += 1;
            }
        }
        if !spawned_large_fireballs.is_empty() {
            let mut inner = self.lock_inner("publish hostile large fireballs");
            for fireball in spawned_large_fireballs {
                let snapshot = server_entity_snapshot_from(fireball);
                let fireball_id = snapshot.id;
                let fireball_position = snapshot.position;
                let fireball_type_id = snapshot.type_id;
                inner
                    .entity_type_aabbs
                    .entry(fireball_type_id)
                    .or_insert_with(|| entity_aabb(&snapshot.type_name));
                track_entity_chunk_locked(&mut inner, fireball_id, fireball_position);
                initialize_entity_wire_state_from_snapshot_locked(&mut inner, &snapshot);
                dispatches.extend(spawn_entity_visibility_from_snapshot_locked(
                    &mut inner, snapshot,
                ));
                attacks += 1;
            }
        }
        if !spawned_wither_skulls.is_empty() {
            let mut inner = self.lock_inner("publish hostile wither skulls");
            for skull in spawned_wither_skulls {
                let snapshot = server_entity_snapshot_from(skull);
                let skull_id = snapshot.id;
                let skull_position = snapshot.position;
                let skull_type_id = snapshot.type_id;
                inner
                    .entity_type_aabbs
                    .entry(skull_type_id)
                    .or_insert_with(|| entity_aabb(&snapshot.type_name));
                track_entity_chunk_locked(&mut inner, skull_id, skull_position);
                initialize_entity_wire_state_from_snapshot_locked(&mut inner, &snapshot);
                dispatches.extend(spawn_entity_visibility_from_snapshot_locked(
                    &mut inner, snapshot,
                ));
                attacks += 1;
            }
        }
        if !spawned_wind_charges.is_empty() {
            let mut inner = self.lock_inner("publish hostile breeze wind charges");
            for projectile in spawned_wind_charges {
                let snapshot = server_entity_snapshot_from(projectile);
                let projectile_id = snapshot.id;
                let projectile_position = snapshot.position;
                let projectile_type_id = snapshot.type_id;
                inner
                    .entity_type_aabbs
                    .entry(projectile_type_id)
                    .or_insert_with(|| entity_aabb(&snapshot.type_name));
                track_entity_chunk_locked(&mut inner, projectile_id, projectile_position);
                initialize_entity_wire_state_from_snapshot_locked(&mut inner, &snapshot);
                dispatches.extend(spawn_entity_visibility_from_snapshot_locked(
                    &mut inner, snapshot,
                ));
                attacks += 1;
            }
        }
        if !spawned_witch_potions.is_empty() {
            let mut inner = self.lock_inner("publish hostile witch potions");
            for potion in spawned_witch_potions {
                let snapshot = server_entity_snapshot_from(potion);
                let potion_id = snapshot.id;
                let potion_position = snapshot.position;
                let potion_type_id = snapshot.type_id;
                inner
                    .entity_type_aabbs
                    .entry(potion_type_id)
                    .or_insert_with(|| entity_aabb(&snapshot.type_name));
                track_entity_chunk_locked(&mut inner, potion_id, potion_position);
                initialize_entity_wire_state_from_snapshot_locked(&mut inner, &snapshot);
                dispatches.extend(spawn_entity_visibility_from_snapshot_locked(
                    &mut inner, snapshot,
                ));
                attacks += 1;
            }
        }
        if !spawned_shulker_bullets.is_empty() {
            let mut inner = self.lock_inner("publish hostile shulker bullets");
            for bullet in spawned_shulker_bullets {
                let snapshot = server_entity_snapshot_from(bullet);
                let bullet_id = snapshot.id;
                let bullet_position = snapshot.position;
                let bullet_type_id = snapshot.type_id;
                inner
                    .entity_type_aabbs
                    .entry(bullet_type_id)
                    .or_insert_with(|| entity_aabb(&snapshot.type_name));
                track_entity_chunk_locked(&mut inner, bullet_id, bullet_position);
                initialize_entity_wire_state_from_snapshot_locked(&mut inner, &snapshot);
                dispatches.extend(spawn_entity_visibility_from_snapshot_locked(
                    &mut inner, snapshot,
                ));
                attacks += 1;
            }
        }
        if !spawned_evoker_fangs.is_empty() {
            let mut inner = self.lock_inner("publish hostile evoker fangs");
            for fang in spawned_evoker_fangs {
                let snapshot = server_entity_snapshot_from(fang);
                let fang_id = snapshot.id;
                let fang_position = snapshot.position;
                let fang_type_id = snapshot.type_id;
                inner
                    .entity_type_aabbs
                    .entry(fang_type_id)
                    .or_insert_with(|| entity_aabb(&snapshot.type_name));
                track_entity_chunk_locked(&mut inner, fang_id, fang_position);
                initialize_entity_wire_state_from_snapshot_locked(&mut inner, &snapshot);
                dispatches.extend(spawn_entity_visibility_from_snapshot_locked(
                    &mut inner, snapshot,
                ));
                attacks += 1;
            }
        }

        if !committed_guardian_attacks.is_empty() {
            let guardian_ids = committed_guardian_attacks
                .iter()
                .map(|attack| attack.hostile_id)
                .collect::<HashSet<_>>();
            let current_guardians = {
                let entities = self.lock_entities("validate hostile guardian beam attackers");
                entities.prefetch(&guardian_ids);
                guardian_ids
                    .iter()
                    .filter_map(|&entity_id| {
                        entities
                            .snapshot(entity_id)
                            .map(|entity| (entity_id, entity))
                    })
                    .collect::<HashMap<_, _>>()
            };
            let recipients = self.movement_recipients.load_full();
            let mut reserved_attacks = Vec::with_capacity(committed_guardian_attacks.len());
            for attack in committed_guardian_attacks {
                if attack.magic_damage <= 0.0 || attack.attack_damage <= 0.0 {
                    continue;
                }
                let Some(guardian) = current_guardians.get(&attack.hostile_id) else {
                    continue;
                };
                let elder = guardian.type_name == "minecraft:elder_guardian";
                if guardian.lifecycle != EntityLifecycle::Alive
                    || !matches!(
                        guardian.type_name.as_str(),
                        "minecraft:guardian" | "minecraft:elder_guardian"
                    )
                    || guardian.retained.guardian_beam.is_some()
                {
                    continue;
                }
                let follow_range = guardian
                    .attributes
                    .base(&AttributeKind::FollowRange)
                    .unwrap_or(16.0)
                    .clamp(1.0, 2_048.0);
                let Some(target_publication) = recipients.get(&attack.target_session) else {
                    continue;
                };
                let Some((_, damage_recipients)) = target_publication.reserve_combat_recipients_if(
                    2,
                    |target, visible_entities| {
                        if !target.is_targetable() || !visible_entities.contains(&attack.hostile_id)
                        {
                            return false;
                        }
                        let target_pose = target.pose();
                        let target_position =
                            Vec3::new(target_pose.x, target_pose.y, target_pose.z);
                        let distance = distance_sq(guardian.position, target_position);
                        distance <= follow_range * follow_range
                            && (elder || distance > GUARDIAN_MIN_TARGET_DISTANCE_SQ)
                    },
                ) else {
                    continue;
                };
                reserved_attacks.push((attack, guardian.clone(), damage_recipients));
            }
            let current_attacker_ids = self
                .current_expected_entity_snapshots(
                    reserved_attacks
                        .iter()
                        .map(|(_, guardian, _)| guardian.clone()),
                )
                .into_iter()
                .filter(|guardian| guardian.retained.guardian_beam.is_none())
                .map(|guardian| guardian.id)
                .collect::<HashSet<_>>();
            for (attack, guardian, damage_recipients) in reserved_attacks {
                if !current_attacker_ids.contains(&attack.hostile_id) {
                    continue;
                }
                let mut damage_recipients = damage_recipients.into_iter();
                let magic_recipient = damage_recipients
                    .next()
                    .expect("two guardian beam recipients were reserved");
                let mob_recipient = damage_recipients
                    .next()
                    .expect("two guardian beam recipients were reserved");
                dispatches.push(VisibilityDispatch {
                    recipient: magic_recipient,
                    command: OutboundCommand::DamagePlayer {
                        damage: PlayerDamageRequest {
                            kind: PlayerDamageKind::IndirectMagic,
                            amount: attack.magic_damage,
                            source_origin: Some(guardian.position),
                        },
                    },
                });
                dispatches.push(VisibilityDispatch {
                    recipient: mob_recipient,
                    command: OutboundCommand::DamagePlayer {
                        damage: PlayerDamageRequest {
                            kind: PlayerDamageKind::MobAttack,
                            amount: attack.attack_damage,
                            source_origin: Some(guardian.position),
                        },
                    },
                });
                attacks += 1;
            }
        }

        if !committed_warden_attacks.is_empty() {
            let warden_ids = committed_warden_attacks
                .iter()
                .map(|attack| attack.hostile_id)
                .collect::<HashSet<_>>();
            let current_wardens = {
                let entities = self.lock_entities("validate hostile warden sonic attackers");
                entities.prefetch(&warden_ids);
                warden_ids
                    .iter()
                    .filter_map(|&entity_id| {
                        entities
                            .snapshot(entity_id)
                            .map(|entity| (entity_id, entity))
                    })
                    .collect::<HashMap<_, _>>()
            };
            let recipients = self.movement_recipients.load_full();
            let mut reserved_attacks = Vec::with_capacity(committed_warden_attacks.len());
            for attack in committed_warden_attacks {
                let Some(warden) = current_wardens.get(&attack.hostile_id) else {
                    continue;
                };
                if warden.lifecycle != EntityLifecycle::Alive
                    || warden.type_name != "minecraft:warden"
                    || !warden.retained.warden_sonic_boom.is_some_and(|state| {
                        state.phase == EntityWardenSonicBoomPhase::Recovery
                            && state.target_session == attack.target_session
                    })
                {
                    continue;
                }
                let Some(target_publication) = recipients.get(&attack.target_session) else {
                    continue;
                };
                let Some((_, target_recipient)) =
                    target_publication.reserve_combat_recipient_if(|target, visible_entities| {
                        if !target.is_targetable() || !visible_entities.contains(&attack.hostile_id)
                        {
                            return false;
                        }
                        let target_pose = target.pose();
                        let dx = target_pose.x - warden.position.x;
                        let dz = target_pose.z - warden.position.z;
                        let dy = (target_pose.y - warden.position.y).abs();
                        dx * dx + dz * dz <= WARDEN_SONIC_RANGE_XZ * WARDEN_SONIC_RANGE_XZ
                            && dy <= WARDEN_SONIC_RANGE_Y
                    })
                else {
                    continue;
                };
                reserved_attacks.push((attack, warden.clone(), target_recipient));
            }
            let current_attacker_ids = self
                .current_expected_entity_snapshots(
                    reserved_attacks.iter().map(|(_, warden, _)| warden.clone()),
                )
                .into_iter()
                .filter(|warden| {
                    warden
                        .retained
                        .warden_sonic_boom
                        .is_some_and(|state| state.phase == EntityWardenSonicBoomPhase::Recovery)
                })
                .map(|warden| warden.id)
                .collect::<HashSet<_>>();
            for (attack, warden, recipient) in reserved_attacks {
                if !current_attacker_ids.contains(&attack.hostile_id) {
                    continue;
                }
                dispatches.push(VisibilityDispatch {
                    recipient,
                    command: OutboundCommand::DamagePlayer {
                        damage: PlayerDamageRequest {
                            kind: PlayerDamageKind::SonicBoom,
                            amount: WARDEN_SONIC_DAMAGE,
                            source_origin: Some(warden.position),
                        },
                    },
                });
                attacks += 1;
            }
        }

        if !melee_attacks.is_empty() {
            let melee_ids = melee_attacks
                .iter()
                .map(|attack| attack.hostile_id)
                .collect::<HashSet<_>>();
            let current_hostiles = {
                let entities = self.lock_entities("validate hostile melee attackers");
                entities.prefetch(&melee_ids);
                melee_ids
                    .iter()
                    .filter_map(|&entity_id| {
                        entities
                            .snapshot(entity_id)
                            .map(|entity| (entity_id, entity))
                    })
                    .collect::<HashMap<_, _>>()
            };
            #[cfg(test)]
            self.pause_before_hostile_session_publication_for_test();
            let recipients = self.movement_recipients.load_full();
            let mut reserved_attacks = Vec::with_capacity(melee_attacks.len());
            for attack in melee_attacks {
                let Some(hostile) = current_hostiles.get(&attack.hostile_id) else {
                    continue;
                };
                if hostile.lifecycle != EntityLifecycle::Alive {
                    continue;
                }
                let Some(target_publication) = recipients.get(&attack.target_session) else {
                    continue;
                };
                let Some((_, target_recipient)) =
                    target_publication.reserve_combat_recipient_if(|target, visible_entities| {
                        #[cfg(test)]
                        self.pause_after_hostile_target_snapshot_for_test();
                        let target_pose = target.pose();
                        if !target.is_targetable()
                            || !visible_entities.contains(&attack.hostile_id)
                            || (target_pose.y - hostile.position.y).abs()
                                > HOSTILE_MELEE_VERTICAL_REACH
                        {
                            return false;
                        }
                        let dx = target_pose.x - hostile.position.x;
                        let dz = target_pose.z - hostile.position.z;
                        dx * dx + dz * dz <= HOSTILE_MELEE_RANGE * HOSTILE_MELEE_RANGE
                            && hostile_faces_target(
                                hostile.position,
                                hostile.rotation,
                                Vec3::new(target_pose.x, target_pose.y, target_pose.z),
                            )
                    })
                else {
                    continue;
                };
                reserved_attacks.push((attack, hostile.clone(), target_recipient));
            }
            let current_attacker_ids = self
                .current_expected_entity_snapshots(
                    reserved_attacks
                        .iter()
                        .map(|(_, hostile, _)| hostile.clone()),
                )
                .into_iter()
                .map(|hostile| hostile.id)
                .collect::<HashSet<_>>();
            for (attack, hostile, target_recipient) in reserved_attacks {
                if !current_attacker_ids.contains(&attack.hostile_id) {
                    continue;
                }
                dispatches.push(VisibilityDispatch {
                    recipient: target_recipient,
                    command: OutboundCommand::DamagePlayer {
                        damage: PlayerDamageRequest {
                            kind: PlayerDamageKind::MobAttack,
                            amount: attack.amount,
                            source_origin: Some(hostile.position),
                        },
                    },
                });
                let animation_recipients = recipients
                    .values()
                    .filter_map(|publication| {
                        publication.reserve_observer_if_visible(attack.hostile_id)
                    })
                    .collect::<Vec<_>>();
                dispatches.extend(visibility_dispatches(animation_recipients, || {
                    OutboundCommand::AnimatePlayer {
                        entity_id: attack.hostile_id.0,
                    }
                }));
                attacks += 1;
            }
        }

        (attacks + creeper_ignitions, dispatches)
    }

    #[cfg(test)]
    pub(super) fn hostile_attack_candidate_count(&self) -> u64 {
        self.hostile_attack_candidates.load(Ordering::Relaxed)
    }

    pub(in crate::play) fn has_rest_preventing_hostile_near_bed(
        &self,
        bed: mc_world::BlockPos,
    ) -> bool {
        const HORIZONTAL_RANGE: f64 = 8.0;
        const VERTICAL_RANGE: f64 = 5.0;

        let center = Vec3::new(
            f64::from(bed.x) + 0.5,
            f64::from(bed.y),
            f64::from(bed.z) + 0.5,
        );
        let inner = self.lock_session_entities("check monsters near bed");
        nearby_entity_candidate_ids_locked(&inner, center, HORIZONTAL_RANGE + 1.0)
            .into_iter()
            .filter_map(|id| inner.entities.snapshot(id))
            .any(|entity| {
                let aabb = entity_aabb(&entity.type_name);
                entity.lifecycle == EntityLifecycle::Alive
                    && entity.item_stack.is_none()
                    && is_hostile_entity(&entity.type_name)
                    && entity.position.x + aabb.half_width >= center.x - HORIZONTAL_RANGE
                    && entity.position.x - aabb.half_width <= center.x + HORIZONTAL_RANGE
                    && entity.position.y + aabb.height >= center.y - VERTICAL_RANGE
                    && entity.position.y <= center.y + VERTICAL_RANGE
                    && entity.position.z + aabb.half_width >= center.z - HORIZONTAL_RANGE
                    && entity.position.z - aabb.half_width <= center.z + HORIZONTAL_RANGE
            })
    }

    #[cfg(test)]
    pub(in crate::play) fn nearby_hostile_entities(
        &self,
        position: Vec3,
        radius: f64,
    ) -> Vec<ServerEntitySnapshot> {
        let inner = self.lock_session_entities("nearby hostile entities");
        nearby_entity_snapshots_locked(&inner, position, radius, |entity| {
            entity.item_stack.is_none() && is_hostile_entity(&entity.type_name)
        })
    }
}

fn hostile_faces_target(position: Vec3, rotation: Rotation, target: Vec3) -> bool {
    let dx = target.x - position.x;
    let dz = target.z - position.z;
    let distance = dx.hypot(dz);
    if distance <= f64::EPSILON {
        return true;
    }
    if !rotation.head_yaw.is_finite() {
        return false;
    }
    let yaw = f64::from(rotation.head_yaw).to_radians();
    let facing_x = -yaw.sin();
    let facing_z = yaw.cos();
    (facing_x * dx + facing_z * dz) / distance > 0.0
}

#[derive(Debug, Clone)]
struct HostileTargetCandidate {
    id: EntityId,
    position: Vec3,
    follow_range: f64,
    uses_ranged_attack: bool,
    uses_small_fireball: bool,
    is_creeper: bool,
    fuse_active: bool,
    guardian_beam_active: bool,
    exclusive_flight: bool,
    wander_speed: f64,
    wander_period_ticks: u32,
    pursuit_speed: f64,
    current: GoalState,
}

fn hostile_target_candidate_from_projection(
    entity: &EntitySimulationProjection,
    mob_behaviors: &MobBehaviorTable,
) -> Option<HostileTargetCandidate> {
    if entity.lifecycle != EntityLifecycle::Alive || !is_hostile_entity(&entity.type_name) {
        return None;
    }
    let profile = mob_behaviors.get_by_name(&entity.type_name)?;
    Some(HostileTargetCandidate {
        id: entity.id,
        position: entity.position,
        follow_range: entity.follow_range,
        uses_ranged_attack: matches!(
            profile.combat,
            MobCombatPolicy::Arrow
                | MobCombatPolicy::Crossbow
                | MobCombatPolicy::GuardianBeam
                | MobCombatPolicy::SmallFireball
                | MobCombatPolicy::SonicBoom
                | MobCombatPolicy::ShulkerBullet
                | MobCombatPolicy::EvokerFangs
                | MobCombatPolicy::LargeFireball
                | MobCombatPolicy::WindCharge
                | MobCombatPolicy::ThrownPotion
                | MobCombatPolicy::WitherSkull
                | MobCombatPolicy::DragonBoss
                | MobCombatPolicy::UnsupportedSpecial
        ),
        uses_small_fireball: profile.combat == MobCombatPolicy::SmallFireball,
        is_creeper: profile.combat == MobCombatPolicy::CreeperFuse,
        fuse_active: entity.primed_tnt,
        guardian_beam_active: entity.guardian_beam_active,
        exclusive_flight: profile.combat == MobCombatPolicy::DragonBoss,
        wander_speed: profile.wander_speed,
        wander_period_ticks: profile.wander_period_ticks,
        pursuit_speed: profile.pursuit_speed,
        current: entity.goal.clone(),
    })
}

fn apply_hostile_target_candidates(
    entities: &mut EntityOwnerAccess,
    players: &[Vec3],
    hostiles: impl IntoIterator<Item = HostileTargetCandidate>,
) {
    let changed = hostiles
        .into_iter()
        .filter_map(|hostile| {
            let target = if players.is_empty() {
                None
            } else {
                let max_distance_sq = hostile.follow_range * hostile.follow_range;
                players
                    .iter()
                    .copied()
                    .filter(|position| distance_sq(*position, hostile.position) <= max_distance_sq)
                    .min_by(|left, right| {
                        distance_sq(*left, hostile.position)
                            .total_cmp(&distance_sq(*right, hostile.position))
                    })
            };
            let goal = if hostile.guardian_beam_active || hostile.exclusive_flight {
                GoalState::Idle
            } else {
                match target {
                    None if hostile.is_creeper && hostile.fuse_active => GoalState::Idle,
                    None => {
                        hostile_wander_goal_for(hostile.wander_speed, hostile.wander_period_ticks)
                    }
                    Some(target)
                        if hostile.is_creeper
                            && (hostile.fuse_active
                                || distance_sq(target, hostile.position)
                                    < CREEPER_TRIGGER_RANGE * CREEPER_TRIGGER_RANGE) =>
                    {
                        GoalState::Idle
                    }
                    Some(target)
                        if hostile.uses_small_fireball
                            && distance_sq(target, hostile.position)
                                >= BLAZE_CLOSE_MELEE_RANGE_SQ =>
                    {
                        GoalState::Idle
                    }
                    Some(target)
                        if !hostile.uses_ranged_attack
                            && (target.y - hostile.position.y).abs()
                                <= HOSTILE_MELEE_VERTICAL_REACH
                            && (target.x - hostile.position.x).powi(2)
                                + (target.z - hostile.position.z).powi(2)
                                <= HOSTILE_MELEE_RANGE * HOSTILE_MELEE_RANGE =>
                    {
                        GoalState::FollowPosition { target, speed: 0.0 }
                    }
                    Some(target) => GoalState::FollowPosition {
                        target,
                        speed: hostile.pursuit_speed,
                    },
                }
            };
            changed_hostile_goal(hostile.id, &hostile.current, goal)
        })
        .collect::<Vec<_>>();
    if !changed.is_empty() {
        let _ = entities.set_goals_deferred_journal(changed);
    }
}

pub(super) fn update_hostile_targets_from_projections<'a>(
    entities: &mut EntityOwnerAccess,
    players: &[Vec3],
    projections: impl IntoIterator<Item = &'a EntitySimulationProjection>,
    mob_behaviors: &MobBehaviorTable,
) {
    let hostiles = projections
        .into_iter()
        .filter_map(|entity| hostile_target_candidate_from_projection(entity, mob_behaviors))
        .collect::<Vec<_>>();
    apply_hostile_target_candidates(entities, players, hostiles);
}

pub(super) fn update_hostile_targets(
    entities: &mut EntityOwnerAccess,
    players: &[Vec3],
    active_ids: Option<&HashSet<EntityId>>,
    mob_behaviors: &MobBehaviorTable,
) {
    let mut hostiles = Vec::new();
    let mut collect = |entity: mc_entity::EntityView<'_>| {
        if entity.lifecycle != EntityLifecycle::Alive || !is_hostile_entity(entity.type_name) {
            return;
        }
        let Some(profile) = mob_behaviors.get_by_name(entity.type_name) else {
            return;
        };
        hostiles.push(HostileTargetCandidate {
            id: entity.id,
            position: entity.position,
            follow_range: entity
                .attributes
                .base(&AttributeKind::FollowRange)
                .unwrap_or(16.0),
            uses_ranged_attack: matches!(
                profile.combat,
                MobCombatPolicy::Arrow
                    | MobCombatPolicy::Crossbow
                    | MobCombatPolicy::GuardianBeam
                    | MobCombatPolicy::SmallFireball
                    | MobCombatPolicy::SonicBoom
                    | MobCombatPolicy::ShulkerBullet
                    | MobCombatPolicy::EvokerFangs
                    | MobCombatPolicy::LargeFireball
                    | MobCombatPolicy::WindCharge
                    | MobCombatPolicy::ThrownPotion
                    | MobCombatPolicy::WitherSkull
                    | MobCombatPolicy::DragonBoss
                    | MobCombatPolicy::UnsupportedSpecial
            ),
            uses_small_fireball: profile.combat == MobCombatPolicy::SmallFireball,
            is_creeper: profile.combat == MobCombatPolicy::CreeperFuse,
            fuse_active: entity.retained.primed_tnt.is_some(),
            guardian_beam_active: entity.retained.guardian_beam.is_some(),
            exclusive_flight: profile.combat == MobCombatPolicy::DragonBoss,
            wander_speed: profile.wander_speed,
            wander_period_ticks: profile.wander_period_ticks,
            pursuit_speed: profile.pursuit_speed,
            current: entity.goal.clone(),
        });
    };
    if let Some(active_ids) = active_ids {
        entities.visit_simulation_entities_for_ids(active_ids, &mut collect);
    } else {
        entities.visit_simulation_entities(&mut collect);
    }
    apply_hostile_target_candidates(entities, players, hostiles);
}

#[cfg(test)]
pub(super) fn hostile_wander_goal() -> GoalState {
    hostile_wander_goal_for(HOSTILE_FOLLOW_SPEED, 20)
}

fn hostile_wander_goal_for(speed: f64, period_ticks: u32) -> GoalState {
    GoalState::Wander {
        speed,
        period_ticks,
    }
}

pub(super) fn changed_hostile_goal(
    entity: EntityId,
    current: &GoalState,
    next: GoalState,
) -> Option<(EntityId, GoalState)> {
    (current != &next).then_some((entity, next))
}
