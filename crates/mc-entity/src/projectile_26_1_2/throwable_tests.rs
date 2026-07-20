use super::*;

const OWNER: EntityIdentity = EntityIdentity::new(11);
const BLOCK: BlockStateId = BlockStateId::new(9);

fn bounds_at(position: Vec3) -> Aabb {
    Aabb::new(
        position.x - 0.125,
        position.y,
        position.z - 0.125,
        position.x + 0.125,
        position.y + 0.25,
        position.z + 0.125,
    )
    .unwrap()
}

fn state(owner: Option<EntityIdentity>, velocity: Vec3, rotation: Rotation) -> ThrowableState {
    let position = Vec3::new(0.0, 0.0, 0.0);
    ThrowableState::new(
        ProjectileState::new(owner, position, bounds_at(position), velocity, rotation).unwrap(),
    )
}

fn stamp(value: u64) -> InputStamp {
    InputStamp {
        world_revision: value,
        collision_revision: value,
        resolution_revision: value,
    }
}

fn eligible() -> HitEligibility {
    HitEligibility {
        can_be_hit_by_projectile: true,
        arrow_pvp_permitted: true,
        shares_owner_vehicle: false,
    }
}

fn impact(entity: i32, x: f64) -> ThrowableEntityHit {
    ThrowableEntityHit {
        entity: EntityId::new(entity),
        location: Vec3::new(x, 0.0, 0.0),
        eligibility: eligible(),
        resolution: EntityHitResolution::Impact,
        input_order: 0,
    }
}

fn input<'a>(
    owner_collision: OwnerCollisionInput<'a>,
    block_hit: Option<BlockHit>,
    entity_hits: &'a mut [ThrowableEntityHit],
) -> ThrowableTickInput<'a> {
    ThrowableTickInput {
        stamp: stamp(3),
        gravity: THROWABLE_DEFAULT_GRAVITY,
        no_gravity: false,
        in_water: false,
        owner_collision,
        block_hit,
        entity_hits,
    }
}

fn tick(
    state: &mut ThrowableState,
    input: ThrowableTickInput<'_>,
) -> Result<ThrowableTickOutcome, ThrowablePrepareError> {
    let plan = prepare_throwable_tick(state, input)?;
    Ok(commit_throwable_tick(state, stamp(3), plan).unwrap())
}

#[test]
fn throwable_tick_matches_java_gravity_drag_rotation_bits_and_shoot_order() {
    let mut state = state(None, Vec3::new(1.0, 1.0, 2.0), Rotation::new(170.0, -170.0));
    let outcome = tick(
        &mut state,
        input(OwnerCollisionInput::missing(), None, &mut []),
    )
    .unwrap();

    assert_eq!(state.projectile.velocity.x.to_bits(), 0x3fef_ae14_8000_0000);
    assert_eq!(state.projectile.velocity.y.to_bits(), 0x3fee_bac7_15c2_8f5c);
    assert_eq!(state.projectile.velocity.z.to_bits(), 0x3fff_ae14_8000_0000);
    assert_eq!(state.projectile.position, state.projectile.velocity);
    assert_eq!(state.projectile.rotation.yaw.to_bits(), 0x430d_5021);
    assert_eq!(state.projectile.rotation.pitch.to_bits(), 0x431c_b0b2);
    assert!(state.projectile.has_been_shot);
    assert!(state.projectile.left_owner);
    assert_eq!(
        outcome.mutation,
        ThrowableTickMutation::Flight {
            hit: HitTarget::Miss,
            deflection_applied: None,
        }
    );
    assert_eq!(
        outcome.publications.iter().collect::<Vec<_>>(),
        vec![ProjectilePublication::ProjectileShot { owner: None }]
    );

    let second = tick(
        &mut state,
        input(OwnerCollisionInput::missing(), None, &mut []),
    )
    .unwrap();
    assert!(second.publications.is_empty());
}

#[test]
fn water_drag_uses_the_exact_float_factor_and_no_gravity_skips_subtraction() {
    let mut state = state(None, Vec3::new(1.0, 1.0, 1.0), Rotation::default());
    let mut tick_input = input(OwnerCollisionInput::missing(), None, &mut []);
    tick_input.in_water = true;
    tick_input.no_gravity = true;
    tick(&mut state, tick_input).unwrap();

    let expected = f64::from(0.8_f32);
    assert_eq!(
        state.projectile.velocity,
        Vec3::new(expected, expected, expected)
    );
    assert_eq!(
        state.projectile.position,
        Vec3::new(expected, expected, expected)
    );
}

#[test]
fn nearest_eligible_entity_replaces_block_and_stable_ties_keep_input_order() {
    let mut state = state(None, Vec3::new(10.0, 0.0, 0.0), Rotation::default());
    let mut blocked = impact(1, 1.0);
    blocked.eligibility.can_be_hit_by_projectile = false;
    let mut hits = [blocked, impact(2, 2.0), impact(3, 2.0), impact(4, 4.0)];
    let block = BlockHit {
        block_state: BLOCK,
        location: Vec3::new(8.0, 0.0, 0.0),
    };
    let outcome = tick(
        &mut state,
        input(OwnerCollisionInput::missing(), Some(block), &mut hits),
    )
    .unwrap();

    assert_eq!(state.projectile.position, Vec3::new(2.0, 0.0, 0.0));
    assert_eq!(
        outcome.mutation,
        ThrowableTickMutation::Flight {
            hit: HitTarget::Entity {
                entity: EntityId::new(2),
                location: Vec3::new(2.0, 0.0, 0.0),
            },
            deflection_applied: None,
        }
    );
    assert_eq!(
        outcome.publications.iter().collect::<Vec<_>>(),
        vec![
            ProjectilePublication::ProjectileShot { owner: None },
            ProjectilePublication::EntityImpact {
                entity: EntityId::new(2),
                location: Vec3::new(2.0, 0.0, 0.0),
            },
            ProjectilePublication::ProjectileLandedEntity {
                entity: EntityId::new(2),
                location: Vec3::new(2.0, 0.0, 0.0),
            },
        ]
    );
}

#[test]
fn generic_throwable_does_not_apply_arrow_player_pvp_gate() {
    let mut state = state(None, Vec3::new(1.0, 0.0, 0.0), Rotation::default());
    let mut hit = impact(7, 0.5);
    hit.eligibility.arrow_pvp_permitted = false;

    let outcome = tick(
        &mut state,
        input(OwnerCollisionInput::missing(), None, &mut [hit]),
    )
    .unwrap();

    assert!(matches!(
        outcome.mutation,
        ThrowableTickMutation::Flight {
            hit: HitTarget::Entity { entity, .. },
            ..
        } if entity == EntityId::new(7)
    ));
}

#[test]
fn throwable_block_hit_truncates_entity_candidate_ray() {
    let block = BlockHit {
        block_state: BLOCK,
        location: Vec3::new(4.0, 0.0, 0.0),
    };
    let mut hits = [impact(1, 6.0), impact(2, 3.0)];
    let mut projectile = state(None, Vec3::new(10.0, 0.0, 0.0), Rotation::default());
    let outcome = tick(
        &mut projectile,
        input(OwnerCollisionInput::missing(), Some(block), &mut hits),
    )
    .unwrap();
    assert!(matches!(
        outcome.mutation,
        ThrowableTickMutation::Flight {
            hit: HitTarget::Entity { entity, .. },
            ..
        } if entity == EntityId::new(2)
    ));

    let mut blocked = state(None, Vec3::new(10.0, 0.0, 0.0), Rotation::default());
    let outcome = tick(
        &mut blocked,
        input(
            OwnerCollisionInput::missing(),
            Some(block),
            &mut [impact(1, 6.0)],
        ),
    )
    .unwrap();
    assert!(matches!(
        outcome.mutation,
        ThrowableTickMutation::Flight {
            hit: HitTarget::Block { .. },
            ..
        }
    ));

    let mut endpoint = state(None, Vec3::new(10.0, 0.0, 0.0), Rotation::default());
    let outcome = tick(
        &mut endpoint,
        input(
            OwnerCollisionInput::missing(),
            Some(block),
            &mut [impact(3, 4.0)],
        ),
    )
    .unwrap();
    assert!(matches!(
        outcome.mutation,
        ThrowableTickMutation::Flight {
            hit: HitTarget::Block { .. },
            ..
        }
    ));
}

#[test]
fn throwable_streams_arbitrarily_dense_candidates_to_the_nearest_hit() {
    let mut state = state(None, Vec3::new(300.0, 0.0, 0.0), Rotation::default());
    let mut hits = (0..256)
        .map(|index| impact(index, 256.0 - f64::from(index)))
        .collect::<Vec<_>>();

    let outcome = tick(
        &mut state,
        input(OwnerCollisionInput::missing(), None, &mut hits),
    )
    .unwrap();

    assert!(matches!(
        outcome.mutation,
        ThrowableTickMutation::Flight {
            hit: HitTarget::Entity { entity, .. },
            ..
        } if entity == EntityId::new(255)
    ));
    assert_eq!(
        outcome.candidate_work,
        CandidateWork {
            candidates: 256,
            duplicate_adjacencies_checked: 255,
            hit_candidates_visited: 1,
        }
    );
}

#[test]
fn throwable_orders_huge_finite_candidates_without_squared_distance_overflow() {
    let mut state = state(None, Vec3::new(1.0, 0.0, 0.0), Rotation::default());
    let mut hits = [impact(44, f64::MAX), impact(45, f64::MAX / 2.0)];
    let outcome = tick(
        &mut state,
        input(OwnerCollisionInput::missing(), None, &mut hits),
    )
    .unwrap();
    assert!(matches!(
        outcome.mutation,
        ThrowableTickMutation::Flight {
            hit: HitTarget::Entity { entity, .. },
            ..
        } if entity == EntityId::new(45)
    ));
}

#[test]
fn distance_order_scales_when_finite_coordinate_subtraction_overflows() {
    let origin = Vec3::new(-f64::MAX, 0.0, 0.0);
    let farther = Vec3::new(f64::MAX, 0.0, 0.0);
    let nearer = Vec3::new(f64::MAX / 2.0, 0.0, 0.0);

    assert_eq!(
        compare_distance(origin, farther, nearer),
        std::cmp::Ordering::Greater
    );
}

#[test]
fn owner_vehicle_eligibility_uses_left_owner_from_the_previous_tick() {
    let mut state = state(Some(OWNER), Vec3::new(2.0, 0.0, 0.0), Rotation::default());
    let mut owner_hit = impact(7, 1.0);
    owner_hit.eligibility.shares_owner_vehicle = true;
    let mut hits = [owner_hit];
    let block = BlockHit {
        block_state: BLOCK,
        location: Vec3::new(1.5, 0.0, 0.0),
    };

    let first = tick(
        &mut state,
        input(
            OwnerCollisionInput::resolved(OWNER, &[]),
            Some(block),
            &mut hits,
        ),
    )
    .unwrap();
    assert!(state.projectile.left_owner);
    assert!(matches!(
        first.mutation,
        ThrowableTickMutation::Flight {
            hit: HitTarget::Block { .. },
            ..
        }
    ));

    let second = tick(
        &mut state,
        input(OwnerCollisionInput::resolved(OWNER, &[]), None, &mut hits),
    )
    .unwrap();
    assert_eq!(
        second.mutation,
        ThrowableTickMutation::Flight {
            hit: HitTarget::Entity {
                entity: EntityId::new(7),
                location: Vec3::new(1.0, 0.0, 0.0),
            },
            deflection_applied: None,
        }
    );
}

#[test]
fn resolved_deflection_is_applied_once_per_deflector_but_still_wins_the_hit() {
    let deflection = ResolvedDeflection {
        velocity: Vec3::new(-0.25, 0.5, 0.75),
        yaw_delta: 175.0,
    };
    let hit = ThrowableEntityHit {
        resolution: EntityHitResolution::Deflected(deflection),
        ..impact(4, 1.0)
    };
    let mut state = state(None, Vec3::new(2.0, 0.0, 0.0), Rotation::new(10.0, 0.0));

    let first = tick(
        &mut state,
        input(OwnerCollisionInput::missing(), None, &mut [hit]),
    )
    .unwrap();
    assert_eq!(state.projectile.velocity, deflection.velocity);
    assert_eq!(
        state.projectile.rotation.yaw,
        first.rotation_before_deflection.yaw + 175.0
    );
    assert_eq!(state.projectile.last_deflected_by, Some(EntityId::new(4)));
    assert_eq!(
        first.publications.get(first.publications.len() - 1),
        Some(ProjectilePublication::Deflected {
            by: Some(EntityId::new(4)),
            applied: true,
        })
    );

    let velocity_before = state.projectile.velocity;
    let repeated = tick(
        &mut state,
        input(OwnerCollisionInput::missing(), None, &mut [hit]),
    )
    .unwrap();
    assert_ne!(state.projectile.velocity, deflection.velocity);
    assert_ne!(state.projectile.velocity, velocity_before);
    assert_eq!(
        repeated.publications.get(repeated.publications.len() - 1),
        Some(ProjectilePublication::Deflected {
            by: Some(EntityId::new(4)),
            applied: false,
        })
    );
}

#[test]
fn block_hit_publishes_impact_then_land_after_the_first_shoot_event() {
    let mut state = state(None, Vec3::new(2.0, 0.0, 0.0), Rotation::default());
    let block = BlockHit {
        block_state: BLOCK,
        location: Vec3::new(1.0, 0.0, 0.0),
    };
    let outcome = tick(
        &mut state,
        input(OwnerCollisionInput::missing(), Some(block), &mut []),
    )
    .unwrap();
    assert_eq!(
        outcome.publications.iter().collect::<Vec<_>>(),
        vec![
            ProjectilePublication::ProjectileShot { owner: None },
            ProjectilePublication::BlockImpact {
                block_state: BLOCK,
                location: block.location,
            },
            ProjectilePublication::ProjectileLandedBlock {
                block_state: BLOCK,
                location: block.location,
            },
        ]
    );
}

#[test]
fn throwable_prepare_rejects_every_invalid_input_branch_without_mutation() {
    let baseline = state(None, Vec3::new(1.0, 0.0, 0.0), Rotation::default());

    let mut discarded = baseline;
    discarded.projectile.lifecycle = ProjectileLifecycle::Discarded;
    assert_eq!(
        prepare_throwable_tick(
            &discarded,
            input(OwnerCollisionInput::missing(), None, &mut []),
        ),
        Err(ThrowablePrepareError::Discarded)
    );

    let mut exhausted = baseline;
    exhausted.projectile.revision = u64::MAX;
    assert_eq!(
        prepare_throwable_tick(
            &exhausted,
            input(OwnerCollisionInput::missing(), None, &mut []),
        ),
        Err(ThrowablePrepareError::RevisionExhausted)
    );

    let mut invalid = baseline;
    invalid.projectile.velocity.x = f64::NAN;
    assert_eq!(
        prepare_throwable_tick(
            &invalid,
            input(OwnerCollisionInput::missing(), None, &mut []),
        ),
        Err(ThrowablePrepareError::InvalidState(StateError::NonFinite(
            StateField::Velocity,
        )))
    );

    let mut invalid_gravity = input(OwnerCollisionInput::missing(), None, &mut []);
    invalid_gravity.gravity = f64::NAN;
    assert_eq!(
        prepare_throwable_tick(&baseline, invalid_gravity),
        Err(ThrowablePrepareError::NonFinite(
            ThrowableInputField::Gravity,
        ))
    );
    let mut negative_gravity = input(OwnerCollisionInput::missing(), None, &mut []);
    negative_gravity.gravity = -0.01;
    assert_eq!(
        prepare_throwable_tick(&baseline, negative_gravity),
        Err(ThrowablePrepareError::NegativeGravity)
    );

    let overflow_state = state(None, Vec3::new(0.0, -f64::MAX, 0.0), Rotation::default());
    let mut overflow = input(OwnerCollisionInput::missing(), None, &mut []);
    overflow.gravity = f64::MAX;
    assert_eq!(
        prepare_throwable_tick(&overflow_state, overflow),
        Err(ThrowablePrepareError::NonFinite(
            ThrowableInputField::ComputedVelocity,
        ))
    );

    let mismatch = state(Some(OWNER), Vec3::new(1.0, 0.0, 0.0), Rotation::default());
    assert_eq!(
        prepare_throwable_tick(
            &mismatch,
            input(
                OwnerCollisionInput::resolved(EntityIdentity::new(99), &[]),
                None,
                &mut [],
            ),
        ),
        Err(ThrowablePrepareError::Owner(
            OwnerInputError::OwnerMismatch {
                expected: OWNER,
                actual: EntityIdentity::new(99),
            }
        ))
    );

    let mut nonfinite_hit = impact(1, 0.5);
    nonfinite_hit.location.y = f64::INFINITY;
    assert_eq!(
        prepare_throwable_tick(
            &baseline,
            input(OwnerCollisionInput::missing(), None, &mut [nonfinite_hit]),
        ),
        Err(ThrowablePrepareError::NonFinite(
            ThrowableInputField::EntityHitLocation,
        ))
    );
    assert_eq!(
        prepare_throwable_tick(
            &baseline,
            input(
                OwnerCollisionInput::missing(),
                None,
                &mut [impact(1, 0.5), impact(1, 0.75)],
            ),
        ),
        Err(ThrowablePrepareError::DuplicateCandidate(EntityId::new(1)))
    );

    let nonfinite_block = BlockHit {
        block_state: BLOCK,
        location: Vec3::new(f64::NAN, 0.0, 0.0),
    };
    assert_eq!(
        prepare_throwable_tick(
            &baseline,
            input(
                OwnerCollisionInput::missing(),
                Some(nonfinite_block),
                &mut [],
            ),
        ),
        Err(ThrowablePrepareError::NonFinite(
            ThrowableInputField::BlockHitLocation,
        ))
    );

    let nonfinite_deflection = ThrowableEntityHit {
        resolution: EntityHitResolution::Deflected(ResolvedDeflection {
            velocity: Vec3::new(f64::NAN, 0.0, 0.0),
            yaw_delta: 0.0,
        }),
        ..impact(2, 0.5)
    };
    assert_eq!(
        prepare_throwable_tick(
            &baseline,
            input(
                OwnerCollisionInput::missing(),
                None,
                &mut [nonfinite_deflection],
            ),
        ),
        Err(ThrowablePrepareError::NonFinite(
            ThrowableInputField::DeflectionVelocity,
        ))
    );

    let nonfinite_yaw = ThrowableEntityHit {
        resolution: EntityHitResolution::Deflected(ResolvedDeflection {
            velocity: Vec3::ZERO,
            yaw_delta: f32::NAN,
        }),
        ..impact(3, 0.5)
    };
    assert_eq!(
        prepare_throwable_tick(
            &baseline,
            input(OwnerCollisionInput::missing(), None, &mut [nonfinite_yaw],),
        ),
        Err(ThrowablePrepareError::NonFinite(
            ThrowableInputField::DeflectionYaw,
        ))
    );

    assert_eq!(baseline.projectile.revision, 0);
}

#[test]
fn throwable_commit_rejects_every_stale_precondition_atomically() {
    let base = state(None, Vec3::new(1.0, 0.0, 0.0), Rotation::default());
    let plan = prepare_throwable_tick(&base, input(OwnerCollisionInput::missing(), None, &mut []))
        .unwrap();

    let mut stale_revision = base;
    stale_revision.projectile.revision = 1;
    let before = stale_revision;
    assert_eq!(
        commit_throwable_tick(&mut stale_revision, stamp(3), plan),
        Err(TickCommitError::StaleState {
            expected: 0,
            actual: 1,
        })
    );
    assert_eq!(stale_revision, before);

    let mut changed = base;
    changed.projectile.left_owner = true;
    let before = changed;
    assert_eq!(
        commit_throwable_tick(&mut changed, stamp(3), plan),
        Err(TickCommitError::StateChangedAtRevision { revision: 0 })
    );
    assert_eq!(changed, before);

    for (actual, expected) in [
        (
            InputStamp {
                world_revision: 4,
                ..stamp(3)
            },
            TickCommitError::StaleWorld {
                expected: 3,
                actual: 4,
            },
        ),
        (
            InputStamp {
                collision_revision: 4,
                ..stamp(3)
            },
            TickCommitError::StaleCollisions {
                expected: 3,
                actual: 4,
            },
        ),
        (
            InputStamp {
                resolution_revision: 4,
                ..stamp(3)
            },
            TickCommitError::StaleResolutions {
                expected: 3,
                actual: 4,
            },
        ),
    ] {
        let mut current = base;
        assert_eq!(
            commit_throwable_tick(&mut current, actual, plan),
            Err(expected)
        );
        assert_eq!(current, base);
    }
}
