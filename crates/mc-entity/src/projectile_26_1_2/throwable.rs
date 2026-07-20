use super::{
    BlockHit, CandidateWork, EntityHitResolution, EntityId, HitTarget, InputStamp,
    OwnerCollisionInput, OwnerInputError, ProjectileLifecycle, ProjectilePublication,
    ProjectileState, PublicationBatch, ResolvedDeflection, RevisionTransition, StateError,
    ThrowableEntityHit, TickCommitError, compare_distance, lerp_rotation, move_projectile,
    outside_owner_collision_range, resolved_owner, select_throwable_entity, target_rotation,
    validate_owner_collision_input,
};

pub const THROWABLE_DEFAULT_GRAVITY: f64 = 0.03;
const WATER_INERTIA: f64 = 0.8_f32 as f64;
const AIR_INERTIA: f64 = 0.99_f32 as f64;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThrowableState {
    pub projectile: ProjectileState,
}

impl ThrowableState {
    #[must_use]
    pub const fn new(projectile: ProjectileState) -> Self {
        Self { projectile }
    }
}

#[derive(Debug)]
pub struct ThrowableTickInput<'a> {
    pub stamp: InputStamp,
    pub gravity: f64,
    pub no_gravity: bool,
    pub in_water: bool,
    pub owner_collision: OwnerCollisionInput<'a>,
    pub block_hit: Option<BlockHit>,
    /// Mutable complete raycast candidates in deterministic caller order.
    ///
    /// Preparation reorders this caller-owned working slice in place and does
    /// not retain it. This provides unbounded candidate count without per-tick
    /// allocation.
    pub entity_hits: &'a mut [ThrowableEntityHit],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThrowableInputField {
    Gravity,
    EntityHitLocation,
    BlockHitLocation,
    DeflectionVelocity,
    DeflectionYaw,
    ComputedVelocity,
    ComputedRotation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThrowablePrepareError {
    Discarded,
    RevisionExhausted,
    InvalidState(StateError),
    NonFinite(ThrowableInputField),
    NegativeGravity,
    Owner(OwnerInputError),
    DuplicateCandidate(EntityId),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ThrowableTickMutation {
    Flight {
        hit: HitTarget,
        deflection_applied: Option<bool>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThrowableTickOutcome {
    pub revision: RevisionTransition,
    pub mutation: ThrowableTickMutation,
    pub rotation_before_deflection: super::Rotation,
    pub candidate_work: CandidateWork,
    pub publications: PublicationBatch,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreparedThrowableTick {
    expected_state: ThrowableState,
    stamp: InputStamp,
    next: ThrowableState,
    outcome: ThrowableTickOutcome,
}

pub fn prepare_throwable_tick(
    current: &ThrowableState,
    input: ThrowableTickInput<'_>,
) -> Result<PreparedThrowableTick, ThrowablePrepareError> {
    if current.projectile.lifecycle == ProjectileLifecycle::Discarded {
        return Err(ThrowablePrepareError::Discarded);
    }
    current
        .projectile
        .validate()
        .map_err(ThrowablePrepareError::InvalidState)?;
    let next_revision = current
        .projectile
        .revision
        .checked_add(1)
        .ok_or(ThrowablePrepareError::RevisionExhausted)?;
    if !input.gravity.is_finite() {
        return Err(ThrowablePrepareError::NonFinite(
            ThrowableInputField::Gravity,
        ));
    }
    if input.gravity < 0.0 {
        return Err(ThrowablePrepareError::NegativeGravity);
    }
    let mut candidate_work = order_candidates(
        current.projectile.position,
        input.block_hit,
        input.entity_hits,
    )?;

    // Validate the owner reference and complete streamed membership first.
    validate_owner_collision_input(&current.projectile, input.owner_collision)
        .map_err(ThrowablePrepareError::Owner)?;
    let owner = resolved_owner(&current.projectile, input.owner_collision);
    let (selected_entity, visited) = select_throwable_entity(
        &current.projectile,
        owner,
        input.block_hit,
        input.entity_hits,
    );
    candidate_work.hit_candidates_visited = visited;
    if let Some(selected) = selected_entity
        && current.projectile.last_deflected_by != Some(selected.entity)
        && let EntityHitResolution::Deflected(deflection) = selected.resolution
    {
        validate_deflection(deflection)?;
    }

    let mut next = *current;
    if !input.no_gravity && input.gravity != 0.0 {
        next.projectile.velocity.y -= input.gravity;
    }
    let inertia = if input.in_water {
        WATER_INERTIA
    } else {
        AIR_INERTIA
    };
    next.projectile.velocity = next.projectile.velocity.scale(inertia);
    if !next.projectile.velocity.is_finite() {
        return Err(ThrowablePrepareError::NonFinite(
            ThrowableInputField::ComputedVelocity,
        ));
    }

    let hit = if let Some(entity) = selected_entity {
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
    let position = match hit {
        HitTarget::Miss => next.projectile.position.plus(next.projectile.velocity),
        HitTarget::Entity { location, .. } | HitTarget::Block { location, .. } => location,
    };
    move_projectile(&mut next.projectile, position).map_err(ThrowablePrepareError::InvalidState)?;

    let target = target_rotation(next.projectile.velocity, false);
    next.projectile.rotation.pitch = lerp_rotation(next.projectile.rotation.pitch, target.pitch)
        .ok_or(ThrowablePrepareError::NonFinite(
            ThrowableInputField::ComputedRotation,
        ))?;
    next.projectile.rotation.yaw = lerp_rotation(next.projectile.rotation.yaw, target.yaw).ok_or(
        ThrowablePrepareError::NonFinite(ThrowableInputField::ComputedRotation),
    )?;
    if !next.projectile.rotation.is_finite() {
        return Err(ThrowablePrepareError::NonFinite(
            ThrowableInputField::ComputedRotation,
        ));
    }
    let rotation_before_deflection = next.projectile.rotation;

    if !next.projectile.left_owner {
        next.projectile.left_owner =
            outside_owner_collision_range(&next.projectile, input.owner_collision)
                .map_err(ThrowablePrepareError::Owner)?;
    }

    let mut publications = PublicationBatch::new();
    if !next.projectile.has_been_shot {
        publications
            .push(ProjectilePublication::ProjectileShot { owner })
            .expect("a throwable tick emits at most three publications");
        next.projectile.has_been_shot = true;
    }

    let mut deflection_applied = None;
    match hit {
        HitTarget::Miss => {}
        HitTarget::Entity { entity, location } => {
            let selected = selected_entity.expect("entity target came from selected candidate");
            match selected.resolution {
                EntityHitResolution::Impact => {
                    publications
                        .push(ProjectilePublication::EntityImpact { entity, location })
                        .expect("a throwable tick emits at most three publications");
                    publications
                        .push(ProjectilePublication::ProjectileLandedEntity { entity, location })
                        .expect("a throwable tick emits at most three publications");
                }
                EntityHitResolution::Deflected(deflection) => {
                    let applied = next.projectile.last_deflected_by != Some(entity);
                    if applied {
                        validate_deflection(deflection)?;
                        next.projectile.velocity = deflection.velocity;
                        next.projectile.rotation.yaw += deflection.yaw_delta;
                        if !next.projectile.rotation.yaw.is_finite() {
                            return Err(ThrowablePrepareError::NonFinite(
                                ThrowableInputField::ComputedRotation,
                            ));
                        }
                        next.projectile.last_deflected_by = Some(entity);
                    }
                    deflection_applied = Some(applied);
                    publications
                        .push(ProjectilePublication::Deflected {
                            by: Some(entity),
                            applied,
                        })
                        .expect("a throwable tick emits at most three publications");
                }
            }
        }
        HitTarget::Block {
            block_state,
            location,
        } => {
            publications
                .push(ProjectilePublication::BlockImpact {
                    block_state,
                    location,
                })
                .expect("a throwable tick emits at most three publications");
            publications
                .push(ProjectilePublication::ProjectileLandedBlock {
                    block_state,
                    location,
                })
                .expect("a throwable tick emits at most three publications");
        }
    }

    next.projectile.revision = next_revision;
    let outcome = ThrowableTickOutcome {
        revision: RevisionTransition {
            from: current.projectile.revision,
            to: next_revision,
        },
        mutation: ThrowableTickMutation::Flight {
            hit,
            deflection_applied,
        },
        rotation_before_deflection,
        candidate_work,
        publications,
    };
    Ok(PreparedThrowableTick {
        expected_state: *current,
        stamp: input.stamp,
        next,
        outcome,
    })
}

pub fn commit_throwable_tick(
    current: &mut ThrowableState,
    stamp: InputStamp,
    plan: PreparedThrowableTick,
) -> Result<ThrowableTickOutcome, TickCommitError> {
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

fn order_candidates(
    origin: super::Vec3,
    block_hit: Option<BlockHit>,
    candidates: &mut [ThrowableEntityHit],
) -> Result<CandidateWork, ThrowablePrepareError> {
    if let Some(block) = block_hit
        && !block.location.is_finite()
    {
        return Err(ThrowablePrepareError::NonFinite(
            ThrowableInputField::BlockHitLocation,
        ));
    }
    for (index, candidate) in candidates.iter_mut().enumerate() {
        if !candidate.location.is_finite() {
            return Err(ThrowablePrepareError::NonFinite(
                ThrowableInputField::EntityHitLocation,
            ));
        }
        candidate.input_order = index;
    }
    candidates.sort_unstable_by_key(|candidate| candidate.entity);
    for pair in candidates.windows(2) {
        if pair[0].entity == pair[1].entity {
            return Err(ThrowablePrepareError::DuplicateCandidate(pair[1].entity));
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

fn validate_deflection(deflection: ResolvedDeflection) -> Result<(), ThrowablePrepareError> {
    if !deflection.velocity.is_finite() {
        return Err(ThrowablePrepareError::NonFinite(
            ThrowableInputField::DeflectionVelocity,
        ));
    }
    if !deflection.yaw_delta.is_finite() {
        return Err(ThrowablePrepareError::NonFinite(
            ThrowableInputField::DeflectionYaw,
        ));
    }
    Ok(())
}
