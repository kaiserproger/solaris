use serde::{Deserialize, Serialize};

use super::{
    Aabb, BlockHit, CandidateWork, EntityId, EntityIdentity, HitTarget, InputStamp,
    OwnerCollisionInput, OwnerInputError, ProjectileLifecycle, ProjectileState, Rotation,
    StateError, ThrowableEntityHit, TickCommitError, Vec3, compare_distance, lerp_rotation,
    move_projectile, outside_owner_collision_range, resolved_owner, select_throwable_entity,
    target_rotation, validate_owner_collision_input,
};

pub const HURTING_PROJECTILE_DEFAULT_ACCELERATION_POWER: f64 = 0.1;
pub const HURTING_PROJECTILE_AIR_INERTIA: f64 = 0.95_f32 as f64;
pub const HURTING_PROJECTILE_WATER_INERTIA: f64 = 0.8_f32 as f64;

const fn default_air_inertia() -> f64 {
    HURTING_PROJECTILE_AIR_INERTIA
}

const fn default_water_inertia() -> f64 {
    HURTING_PROJECTILE_WATER_INERTIA
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HurtingProjectileState {
    pub projectile: ProjectileState,
    pub acceleration_power: f64,
    #[serde(default = "default_air_inertia")]
    pub air_inertia: f64,
    #[serde(default = "default_water_inertia")]
    pub water_inertia: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HurtingProjectileError {
    InvalidProjectile(StateError),
    NonFiniteAcceleration,
    NegativeAcceleration,
    NonFiniteVelocity,
    NonFiniteInertia,
    NegativeInertia,
    InvalidRotation,
    RevisionExhausted,
}

#[derive(Debug)]
pub struct HurtingProjectileTickInput<'a> {
    pub stamp: InputStamp,
    pub in_water: bool,
    pub owner_collision: OwnerCollisionInput<'a>,
    pub block_hit: Option<BlockHit>,
    /// Complete generic projectile raycast candidates in deterministic caller order.
    pub entity_hits: &'a mut [ThrowableEntityHit],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HurtingProjectilePrepareError {
    Discarded,
    InvalidState(StateError),
    Motion(HurtingProjectileError),
    Owner(OwnerInputError),
    NonFiniteEntityHit,
    NonFiniteBlockHit,
    DuplicateCandidate(EntityId),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HurtingProjectileTickOutcome {
    pub hit: HitTarget,
    pub candidate_work: CandidateWork,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreparedHurtingProjectileTick {
    expected_state: HurtingProjectileState,
    stamp: InputStamp,
    next: HurtingProjectileState,
    outcome: HurtingProjectileTickOutcome,
}

impl HurtingProjectileState {
    pub fn new(
        owner: Option<EntityIdentity>,
        position: Vec3,
        bounds: Aabb,
        direction: Vec3,
        rotation: Rotation,
        acceleration_power: f64,
    ) -> Result<Self, HurtingProjectileError> {
        validate_acceleration(acceleration_power)?;
        let velocity = normalized(direction).scale(acceleration_power);
        let projectile = ProjectileState::new(owner, position, bounds, velocity, rotation)
            .map_err(HurtingProjectileError::InvalidProjectile)?;
        Ok(Self {
            projectile,
            acceleration_power,
            air_inertia: HURTING_PROJECTILE_AIR_INERTIA,
            water_inertia: HURTING_PROJECTILE_WATER_INERTIA,
        })
    }

    pub fn with_inertia(mut self, air: f64, water: f64) -> Result<Self, HurtingProjectileError> {
        validate_inertia(air)?;
        validate_inertia(water)?;
        self.air_inertia = air;
        self.water_inertia = water;
        Ok(self)
    }

    pub fn next_velocity(self, in_water: bool) -> Result<Vec3, HurtingProjectileError> {
        next_hurting_projectile_velocity_with_inertia(
            self.projectile.velocity,
            self.acceleration_power,
            if in_water {
                self.water_inertia
            } else {
                self.air_inertia
            },
        )
    }

    pub fn next_rotation(self, velocity: Vec3) -> Result<Rotation, HurtingProjectileError> {
        next_hurting_projectile_rotation(self.projectile.rotation, velocity)
    }

    pub fn retarget_velocity(self, velocity: Vec3) -> Result<Self, HurtingProjectileError> {
        if !velocity.is_finite() {
            return Err(HurtingProjectileError::NonFiniteVelocity);
        }
        let revision = self
            .projectile
            .revision
            .checked_add(1)
            .ok_or(HurtingProjectileError::RevisionExhausted)?;
        let mut projectile = self.projectile;
        projectile.revision = revision;
        projectile.velocity = velocity;
        projectile.rotation = self.next_rotation(velocity)?;
        Ok(Self {
            projectile,
            acceleration_power: self.acceleration_power,
            air_inertia: self.air_inertia,
            water_inertia: self.water_inertia,
        })
    }

    pub fn advance(
        self,
        position: Vec3,
        velocity: Vec3,
        rotation: Rotation,
        discarded: bool,
    ) -> Result<Self, HurtingProjectileError> {
        if !position.is_finite() || !velocity.is_finite() {
            return Err(HurtingProjectileError::NonFiniteVelocity);
        }
        if !rotation.is_finite() {
            return Err(HurtingProjectileError::InvalidRotation);
        }
        let revision = self
            .projectile
            .revision
            .checked_add(1)
            .ok_or(HurtingProjectileError::RevisionExhausted)?;
        let mut projectile = self.projectile;
        move_projectile(&mut projectile, position)
            .map_err(HurtingProjectileError::InvalidProjectile)?;
        projectile.revision = revision;
        projectile.velocity = velocity;
        projectile.rotation = rotation;
        projectile.has_been_shot = true;
        if discarded {
            projectile.lifecycle = ProjectileLifecycle::Discarded;
        }
        Ok(Self {
            projectile,
            acceleration_power: self.acceleration_power,
            air_inertia: self.air_inertia,
            water_inertia: self.water_inertia,
        })
    }
}

pub fn prepare_hurting_projectile_tick(
    current: &HurtingProjectileState,
    input: HurtingProjectileTickInput<'_>,
) -> Result<PreparedHurtingProjectileTick, HurtingProjectilePrepareError> {
    if current.projectile.lifecycle == ProjectileLifecycle::Discarded {
        return Err(HurtingProjectilePrepareError::Discarded);
    }
    current
        .projectile
        .validate()
        .map_err(HurtingProjectilePrepareError::InvalidState)?;
    validate_acceleration(current.acceleration_power)
        .map_err(HurtingProjectilePrepareError::Motion)?;
    let mut candidate_work = order_hurting_candidates(
        current.projectile.position,
        input.block_hit,
        input.entity_hits,
    )?;
    validate_owner_collision_input(&current.projectile, input.owner_collision)
        .map_err(HurtingProjectilePrepareError::Owner)?;
    let owner = resolved_owner(&current.projectile, input.owner_collision);
    let (selected, visited) = select_throwable_entity(
        &current.projectile,
        owner,
        input.block_hit,
        input.entity_hits,
    );
    candidate_work.hit_candidates_visited = visited;
    let hit = if let Some(entity) = selected {
        HitTarget::Entity {
            entity: entity.entity,
            location: entity.location,
        }
    } else if let Some(block) = input.block_hit {
        HitTarget::Block {
            block_state: block.block_state,
            location: block.location,
        }
    } else {
        HitTarget::Miss
    };
    let velocity = current
        .next_velocity(input.in_water)
        .map_err(HurtingProjectilePrepareError::Motion)?;
    let position = match hit {
        HitTarget::Miss => current.projectile.position.plus(velocity),
        HitTarget::Entity { location, .. } | HitTarget::Block { location, .. } => location,
    };
    let rotation = current
        .next_rotation(velocity)
        .map_err(HurtingProjectilePrepareError::Motion)?;
    let mut next = current
        .advance(
            position,
            velocity,
            rotation,
            !matches!(hit, HitTarget::Miss),
        )
        .map_err(HurtingProjectilePrepareError::Motion)?;
    if !next.projectile.left_owner {
        next.projectile.left_owner =
            outside_owner_collision_range(&next.projectile, input.owner_collision)
                .map_err(HurtingProjectilePrepareError::Owner)?;
    }
    Ok(PreparedHurtingProjectileTick {
        expected_state: *current,
        stamp: input.stamp,
        next,
        outcome: HurtingProjectileTickOutcome {
            hit,
            candidate_work,
        },
    })
}

pub fn commit_hurting_projectile_tick(
    current: &mut HurtingProjectileState,
    stamp: InputStamp,
    plan: PreparedHurtingProjectileTick,
) -> Result<HurtingProjectileTickOutcome, TickCommitError> {
    if current.projectile.revision != plan.expected_state.projectile.revision {
        return Err(TickCommitError::StaleState {
            expected: plan.expected_state.projectile.revision,
            actual: current.projectile.revision,
        });
    }
    if *current != plan.expected_state {
        return Err(TickCommitError::StateChangedAtRevision {
            revision: current.projectile.revision,
        });
    }
    if stamp.world_revision != plan.stamp.world_revision {
        return Err(TickCommitError::StaleWorld {
            expected: plan.stamp.world_revision,
            actual: stamp.world_revision,
        });
    }
    if stamp.collision_revision != plan.stamp.collision_revision {
        return Err(TickCommitError::StaleCollisions {
            expected: plan.stamp.collision_revision,
            actual: stamp.collision_revision,
        });
    }
    if stamp.resolution_revision != plan.stamp.resolution_revision {
        return Err(TickCommitError::StaleResolutions {
            expected: plan.stamp.resolution_revision,
            actual: stamp.resolution_revision,
        });
    }
    *current = plan.next;
    Ok(plan.outcome)
}

fn order_hurting_candidates(
    origin: Vec3,
    block_hit: Option<BlockHit>,
    candidates: &mut [ThrowableEntityHit],
) -> Result<CandidateWork, HurtingProjectilePrepareError> {
    if block_hit.is_some_and(|block| !block.location.is_finite()) {
        return Err(HurtingProjectilePrepareError::NonFiniteBlockHit);
    }
    for (index, candidate) in candidates.iter_mut().enumerate() {
        if !candidate.location.is_finite() {
            return Err(HurtingProjectilePrepareError::NonFiniteEntityHit);
        }
        candidate.input_order = index;
    }
    candidates.sort_unstable_by_key(|candidate| candidate.entity);
    for pair in candidates.windows(2) {
        if pair[0].entity == pair[1].entity {
            return Err(HurtingProjectilePrepareError::DuplicateCandidate(
                pair[1].entity,
            ));
        }
    }
    candidates.sort_unstable_by(|left, right| {
        compare_distance(origin, left.location, right.location)
            .then_with(|| left.input_order.cmp(&right.input_order))
    });
    Ok(CandidateWork {
        candidates: candidates.len(),
        duplicate_adjacencies_checked: candidates.len().saturating_sub(1),
        hit_candidates_visited: 0,
    })
}

pub fn next_hurting_projectile_velocity(
    velocity: Vec3,
    acceleration_power: f64,
    in_water: bool,
) -> Result<Vec3, HurtingProjectileError> {
    let inertia = if in_water {
        HURTING_PROJECTILE_WATER_INERTIA
    } else {
        HURTING_PROJECTILE_AIR_INERTIA
    };
    next_hurting_projectile_velocity_with_inertia(velocity, acceleration_power, inertia)
}

pub fn next_hurting_projectile_velocity_with_inertia(
    velocity: Vec3,
    acceleration_power: f64,
    inertia: f64,
) -> Result<Vec3, HurtingProjectileError> {
    validate_acceleration(acceleration_power)?;
    validate_inertia(inertia)?;
    if !velocity.is_finite() {
        return Err(HurtingProjectileError::NonFiniteVelocity);
    }
    let acceleration = normalized(velocity).scale(acceleration_power);
    let next = velocity.plus(acceleration).scale(inertia);
    next.is_finite()
        .then_some(next)
        .ok_or(HurtingProjectileError::NonFiniteVelocity)
}

pub fn next_hurting_projectile_rotation(
    current: Rotation,
    velocity: Vec3,
) -> Result<Rotation, HurtingProjectileError> {
    if !velocity.is_finite() {
        return Err(HurtingProjectileError::NonFiniteVelocity);
    }
    if velocity.length_squared() == 0.0 {
        return Ok(current);
    }
    let target = target_rotation(velocity, false);
    let yaw =
        lerp_rotation(current.yaw, target.yaw).ok_or(HurtingProjectileError::InvalidRotation)?;
    let pitch = lerp_rotation(current.pitch, target.pitch)
        .ok_or(HurtingProjectileError::InvalidRotation)?;
    Ok(Rotation::new(yaw, pitch))
}

fn validate_acceleration(acceleration_power: f64) -> Result<(), HurtingProjectileError> {
    if !acceleration_power.is_finite() {
        return Err(HurtingProjectileError::NonFiniteAcceleration);
    }
    if acceleration_power < 0.0 {
        return Err(HurtingProjectileError::NegativeAcceleration);
    }
    Ok(())
}

fn validate_inertia(inertia: f64) -> Result<(), HurtingProjectileError> {
    if !inertia.is_finite() {
        return Err(HurtingProjectileError::NonFiniteInertia);
    }
    if inertia < 0.0 {
        return Err(HurtingProjectileError::NegativeInertia);
    }
    Ok(())
}

fn normalized(value: Vec3) -> Vec3 {
    let length_squared = value.length_squared();
    if !length_squared.is_finite() || length_squared <= 1.0e-14 {
        return Vec3::ZERO;
    }
    value.scale(length_squared.sqrt().recip())
}
