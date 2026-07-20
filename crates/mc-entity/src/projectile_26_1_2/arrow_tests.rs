use super::*;

const OWNER: EntityIdentity = EntityIdentity::new(31);
const BLOCK: BlockStateId = BlockStateId::new(41);

fn bounds_at(position: Vec3) -> Aabb {
    Aabb::new(
        position.x - 0.25,
        position.y,
        position.z - 0.25,
        position.x + 0.25,
        position.y + 0.5,
        position.z + 0.25,
    )
    .unwrap()
}

fn arrow(velocity: Vec3, pierce_level: i8) -> ArrowState {
    let position = Vec3::ZERO;
    ArrowState::new(
        ProjectileState::new(
            None,
            position,
            bounds_at(position),
            velocity,
            Rotation::new(170.0, -170.0),
        )
        .unwrap(),
        PickupMode::Disallowed,
        pierce_level,
    )
}

fn stamp(value: u64) -> InputStamp {
    InputStamp {
        world_revision: value,
        collision_revision: value,
        resolution_revision: value,
    }
}

fn eligibility() -> HitEligibility {
    HitEligibility {
        can_be_hit_by_projectile: true,
        arrow_pvp_permitted: true,
        shares_owner_vehicle: false,
    }
}

fn accepted(entity: i32, hit_x: f64, center_x: f64) -> ArrowEntityHit {
    ArrowEntityHit {
        entity: EntityId::new(entity),
        location: Vec3::new(hit_x, 0.0, 0.0),
        entity_position: Vec3::new(center_x, 0.0, 0.0),
        eligibility: eligibility(),
        resolution: ArrowEntityResolution::Damage(ArrowDamageResolution::Accepted {
            enderman: false,
            living: true,
            killed: false,
        }),
        input_order: 0,
    }
}

fn input<'a>(
    owner_collision: OwnerCollisionInput<'a>,
    block_hit: Option<ArrowBlockHit>,
    entity_hits: &'a mut [ArrowEntityHit],
) -> ArrowTickInput<'a> {
    ArrowTickInput {
        stamp: stamp(7),
        owner_collision,
        embedded_in_block: false,
        current_block_state: BLOCK,
        should_fall: false,
        fall_velocity_scale: None,
        in_water: false,
        in_water_or_rain: false,
        no_gravity: false,
        block_hit,
        entity_hits,
    }
}

fn tick(
    state: &mut ArrowState,
    input: ArrowTickInput<'_>,
) -> Result<ArrowTickOutcome, ArrowPrepareError> {
    let plan = prepare_arrow_tick(state, input)?;
    Ok(commit_arrow_tick(state, stamp(7), plan).unwrap())
}

#[test]
fn water_flight_uses_pre_drag_motion_for_position_and_rotation_then_applies_gravity() {
    let mut state = arrow(Vec3::new(1.0, 1.0, 2.0), 0);
    let mut tick_input = input(OwnerCollisionInput::missing(), None, &mut []);
    tick_input.in_water = true;
    let outcome = tick(&mut state, tick_input).unwrap();

    assert_eq!(state.projectile.position, Vec3::new(1.0, 1.0, 2.0));
    assert_eq!(state.projectile.velocity.x.to_bits(), 0x3fe3_3333_4000_0000);
    assert_eq!(state.projectile.velocity.y.to_bits(), 0x3fe1_9999_a666_6666);
    assert_eq!(state.projectile.velocity.z.to_bits(), 0x3ff3_3333_4000_0000);
    assert_eq!(state.projectile.rotation.yaw.to_bits(), 0x430d_5021);
    assert_eq!(state.projectile.rotation.pitch.to_bits(), 0x431c_d1a6);
    assert_eq!(
        outcome.mutation,
        ArrowTickMutation::Flight {
            no_physics: false,
            ordered_entity_hits: 0,
            block_processed: false,
            target_deflection: false,
        }
    );
    assert_eq!(
        outcome.publications.iter().collect::<Vec<_>>(),
        vec![ProjectilePublication::ProjectileShot { owner: None }]
    );
}

#[test]
fn no_physics_keeps_in_ground_flag_moves_by_pre_drag_motion_and_skips_gravity() {
    let mut state = arrow(Vec3::new(1.0, 1.0, 2.0), 0);
    state.in_ground = true;
    state.in_ground_time = 9;
    state.no_physics = true;
    let outcome = tick(
        &mut state,
        input(OwnerCollisionInput::missing(), None, &mut []),
    )
    .unwrap();

    let drag = f64::from(0.99_f32);
    assert_eq!(state.projectile.position, Vec3::new(1.0, 1.0, 2.0));
    assert_eq!(state.projectile.velocity, Vec3::new(drag, drag, 2.0 * drag));
    assert!(state.in_ground);
    assert_eq!(state.in_ground_time, 0);
    assert!(matches!(
        outcome.mutation,
        ArrowTickMutation::Flight {
            no_physics: true,
            ..
        }
    ));
    assert_ne!(state.projectile.rotation.yaw.to_bits(), 0x430d_5021);
}

#[test]
fn grounded_ticks_decrement_shake_despawn_at_1200_and_do_not_run_projectile_tick() {
    let mut state = arrow(Vec3::ZERO, 0);
    state.in_ground = true;
    state.shake_time = 1;
    state.despawn_age = ARROW_DESPAWN_TICKS - 1;
    state.last_block_state = Some(BLOCK);
    let outcome = tick(
        &mut state,
        input(OwnerCollisionInput::missing(), None, &mut []),
    )
    .unwrap();

    assert_eq!(state.shake_time, 0);
    assert_eq!(state.despawn_age, ARROW_DESPAWN_TICKS);
    assert_eq!(state.in_ground_time, 1);
    assert_eq!(state.projectile.lifecycle, ProjectileLifecycle::Discarded);
    assert!(!state.projectile.has_been_shot);
    assert_eq!(
        outcome.mutation,
        ArrowTickMutation::Grounded {
            started_falling: false,
            despawned: true,
        }
    );
    assert_eq!(
        outcome.publications.iter().collect::<Vec<_>>(),
        vec![ProjectilePublication::Discarded {
            reason: DiscardReason::DespawnAge,
        }]
    );
}

#[test]
fn changed_support_starts_falling_with_resolved_rng_scale_and_resets_age() {
    let mut state = arrow(Vec3::new(2.0, 3.0, 4.0), 0);
    state.in_ground = true;
    state.in_ground_time = 5;
    state.despawn_age = 700;
    state.last_block_state = Some(BlockStateId::new(99));
    let mut tick_input = input(OwnerCollisionInput::missing(), None, &mut []);
    tick_input.should_fall = true;
    tick_input.fall_velocity_scale = Some(Vec3::new(0.1, 0.15, 0.199));
    let outcome = tick(&mut state, tick_input).unwrap();

    assert!(!state.in_ground);
    assert_eq!(
        state.projectile.velocity,
        Vec3::new(2.0 * 0.1, 3.0 * 0.15, 4.0 * 0.199)
    );
    assert_eq!(state.despawn_age, 0);
    assert_eq!(state.in_ground_time, 6);
    assert!(!state.projectile.has_been_shot);
    assert_eq!(
        outcome.mutation,
        ArrowTickMutation::Grounded {
            started_falling: true,
            despawned: false,
        }
    );
}

#[test]
fn piercing_order_is_stable_by_entity_center_and_block_follows_surviving_hits() {
    let mut state = arrow(Vec3::new(10.0, 0.0, 0.0), 1);
    let mut first = accepted(1, 7.0, 1.0);
    first.resolution = ArrowEntityResolution::Damage(ArrowDamageResolution::Accepted {
        enderman: false,
        living: true,
        killed: true,
    });
    let mut hits = [accepted(2, 2.0, 2.0), first];
    let block = ArrowBlockHit::block(BLOCK, BlockPosition::new(8, 0, 0), Vec3::new(8.0, 0.0, 0.0));
    let outcome = tick(
        &mut state,
        input(OwnerCollisionInput::missing(), Some(block), &mut hits),
    )
    .unwrap();

    let impacts = outcome
        .publications
        .iter()
        .filter_map(|publication| match publication {
            ProjectilePublication::EntityImpact { entity, .. } => Some(entity),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(impacts, vec![EntityId::new(1), EntityId::new(2)]);
    assert_eq!(
        state.projectile.position,
        Vec3::new(8.0 - f64::from(0.05_f32), 0.0, 0.0)
    );
    assert_eq!(state.projectile.velocity, Vec3::ZERO);
    assert!(state.in_ground);
    assert_eq!(state.shake_time, 7);
    assert_eq!(state.pierce_level, 0);
    assert!(state.pierced_entities.is_empty());
    assert!(state.pierced_and_killed.is_empty());
    assert_eq!(state.last_block_state, Some(BLOCK));
    assert_eq!(state.last_block_position, Some(BlockPosition::new(8, 0, 0)));
    assert_eq!(
        outcome.publications.get(outcome.publications.len() - 1),
        Some(ProjectilePublication::ProjectileShot { owner: None })
    );
}

#[test]
fn arrow_block_hit_truncates_entity_candidate_ray() {
    let mut state = arrow(Vec3::new(10.0, 0.0, 0.0), 0);
    let block = ArrowBlockHit::block(BLOCK, BlockPosition::new(4, 0, 0), Vec3::new(4.0, 0.0, 0.0));
    let behind_block = accepted(1, 6.0, 6.0);
    let outcome = tick(
        &mut state,
        input(
            OwnerCollisionInput::missing(),
            Some(block),
            &mut [behind_block],
        ),
    )
    .unwrap();

    assert_eq!(
        outcome.mutation,
        ArrowTickMutation::Flight {
            no_physics: false,
            ordered_entity_hits: 0,
            block_processed: true,
            target_deflection: false,
        }
    );
    assert!(state.in_ground);
    assert_eq!(state.last_block_position, Some(BlockPosition::new(4, 0, 0)));
    assert!(!outcome.publications.iter().any(|event| matches!(
        event,
        ProjectilePublication::EntityImpact { entity, .. } if entity == behind_block.entity
    )));

    let mut endpoint = arrow(Vec3::new(10.0, 0.0, 0.0), 0);
    let at_block = accepted(2, 4.0, 4.0);
    let outcome = tick(
        &mut endpoint,
        input(OwnerCollisionInput::missing(), Some(block), &mut [at_block]),
    )
    .unwrap();
    assert!(!outcome.publications.iter().any(|event| matches!(
        event,
        ProjectilePublication::EntityImpact { entity, .. } if entity == at_block.entity
    )));
    assert!(
        outcome
            .publications
            .iter()
            .any(|event| matches!(event, ProjectilePublication::BlockImpact { .. }))
    );
}

#[test]
fn authoritative_block_hit_precedes_embedded_grounded_path() {
    let mut state = arrow(Vec3::new(4.0, 0.0, 0.0), 0);
    let hit = ArrowBlockHit::block(
        BlockStateId::new(77),
        BlockPosition::new(3, 0, 0),
        Vec3::new(3.0, 0.0, 0.0),
    );
    let mut tick_input = input(OwnerCollisionInput::missing(), Some(hit), &mut []);
    tick_input.embedded_in_block = true;

    let outcome = tick(&mut state, tick_input).unwrap();

    assert_eq!(
        state.projectile.position,
        Vec3::new(3.0 - f64::from(0.05_f32), 0.0, 0.0)
    );
    assert_eq!(state.last_block_state, Some(BlockStateId::new(77)));
    assert_eq!(state.last_block_position, Some(BlockPosition::new(3, 0, 0)));
    assert!(state.in_ground);
    assert!(matches!(
        outcome.mutation,
        ArrowTickMutation::Flight {
            block_processed: true,
            ..
        }
    ));
}

#[test]
fn arrow_orders_huge_finite_centers_without_squared_distance_overflow() {
    let mut state = arrow(Vec3::new(1.0, 0.0, 0.0), 0);
    let mut hits = [
        accepted(70, f64::MAX, f64::MAX),
        accepted(71, f64::MAX / 2.0, f64::MAX / 2.0),
    ];
    let outcome = tick(
        &mut state,
        input(OwnerCollisionInput::missing(), None, &mut hits),
    )
    .unwrap();

    assert!(outcome.publications.iter().any(|event| matches!(
        event,
        ProjectilePublication::EntityImpact { entity, .. } if entity == EntityId::new(71)
    )));
    assert!(!outcome.publications.iter().any(|event| matches!(
        event,
        ProjectilePublication::EntityImpact { entity, .. } if entity == EntityId::new(70)
    )));
}

#[test]
fn arrow_streams_dense_candidates_and_preserves_nearest_order() {
    let mut state = arrow(Vec3::new(300.0, 0.0, 0.0), 0);
    let mut hits = (0..256)
        .map(|index| accepted(index, 256.0 - f64::from(index), 256.0 - f64::from(index)))
        .collect::<Vec<_>>();
    let outcome = tick(
        &mut state,
        input(OwnerCollisionInput::missing(), None, &mut hits),
    )
    .unwrap();

    assert_eq!(
        outcome.mutation,
        ArrowTickMutation::Flight {
            no_physics: false,
            ordered_entity_hits: 256,
            block_processed: false,
            target_deflection: false,
        }
    );
    assert_eq!(
        outcome.candidate_work,
        CandidateWork {
            candidates: 256,
            duplicate_adjacencies_checked: 255,
            hit_candidates_visited: 256,
        }
    );
    assert!(outcome.publications.iter().any(|event| matches!(
        event,
        ProjectilePublication::EntityImpact { entity, .. } if entity == EntityId::new(255)
    )));
}

#[test]
fn dense_ineligible_arrow_candidates_are_visited_once_after_ordering() {
    const CANDIDATES: usize = 4_096;
    let mut state = arrow(Vec3::new(5_000.0, 0.0, 0.0), 0);
    let mut hits = (0..CANDIDATES)
        .rev()
        .map(|index| {
            let mut hit = accepted(index as i32, index as f64 + 1.0, index as f64 + 1.0);
            hit.eligibility.can_be_hit_by_projectile = false;
            hit
        })
        .collect::<Vec<_>>();

    let outcome = tick(
        &mut state,
        input(OwnerCollisionInput::missing(), None, &mut hits),
    )
    .unwrap();

    assert_eq!(
        outcome.candidate_work,
        CandidateWork {
            candidates: CANDIDATES,
            duplicate_adjacencies_checked: CANDIDATES - 1,
            hit_candidates_visited: CANDIDATES,
        }
    );
    assert_eq!(hits.first().unwrap().entity, EntityId::new(0));
    assert_eq!(hits.last().unwrap().entity, EntityId::new(4_095));
}

#[test]
fn arrow_keeps_abstract_arrow_player_pvp_gate() {
    let mut state = arrow(Vec3::new(1.0, 0.0, 0.0), 0);
    let mut hit = accepted(9, 0.5, 0.5);
    hit.eligibility.arrow_pvp_permitted = false;
    let outcome = tick(
        &mut state,
        input(OwnerCollisionInput::missing(), None, &mut [hit]),
    )
    .unwrap();

    assert_eq!(
        outcome.mutation,
        ArrowTickMutation::Flight {
            no_physics: false,
            ordered_entity_hits: 0,
            block_processed: false,
            target_deflection: false,
        }
    );
}

#[test]
fn dense_non_discarding_hits_return_capacity_error_without_panicking() {
    let state = arrow(Vec3::new(600.0, 0.0, 0.0), 0);
    let mut hits = (0..200)
        .map(|index| {
            let mut hit = accepted(index, f64::from(index) + 1.0, f64::from(index) + 1.0);
            hit.resolution = ArrowEntityResolution::Damage(ArrowDamageResolution::Accepted {
                enderman: true,
                living: true,
                killed: false,
            });
            hit
        })
        .collect::<Vec<_>>();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        prepare_arrow_tick(
            &state,
            input(OwnerCollisionInput::missing(), None, &mut hits),
        )
    }));
    assert_eq!(
        result.unwrap(),
        Err(ArrowPrepareError::PublicationCapacityExceeded {
            capacity: MAX_PUBLICATIONS,
        })
    );
}

#[test]
fn equal_entity_center_distances_keep_caller_order() {
    let mut state = arrow(Vec3::new(4.0, 0.0, 0.0), 2);
    let mut hits = [accepted(12, 2.0, 1.0), accepted(11, 1.0, -1.0)];
    let outcome = tick(
        &mut state,
        input(OwnerCollisionInput::missing(), None, &mut hits),
    )
    .unwrap();
    let impacts = outcome
        .publications
        .iter()
        .filter_map(|event| match event {
            ProjectilePublication::EntityImpact { entity, .. } => Some(entity),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(impacts, vec![EntityId::new(12), EntityId::new(11)]);
}

#[test]
fn next_entity_beyond_piercing_limit_discards_before_damage_and_skips_block() {
    let mut state = arrow(Vec3::new(10.0, 0.0, 0.0), 1);
    let mut hits = [
        accepted(1, 1.0, 1.0),
        accepted(2, 2.0, 2.0),
        accepted(3, 3.0, 3.0),
    ];
    let outcome = tick(
        &mut state,
        input(
            OwnerCollisionInput::missing(),
            Some(ArrowBlockHit::block(
                BLOCK,
                BlockPosition::new(8, 0, 0),
                Vec3::new(8.0, 0.0, 0.0),
            )),
            &mut hits,
        ),
    )
    .unwrap();

    assert_eq!(state.projectile.lifecycle, ProjectileLifecycle::Discarded);
    assert_eq!(state.pierced_entities.len(), 2);
    assert!(!outcome.publications.iter().any(|event| matches!(
        event,
        ProjectilePublication::ArrowDamageAccepted { entity, .. }
            if entity == EntityId::new(3)
    )));
    assert!(
        !outcome
            .publications
            .iter()
            .any(|event| matches!(event, ProjectilePublication::BlockImpact { .. }))
    );
}

#[test]
fn maximum_positive_byte_piercing_tick_fits_fixed_hot_path_storage() {
    let mut state = arrow(Vec3::new(200.0, 0.0, 0.0), i8::MAX);
    let mut hits: [ArrowEntityHit; MAX_PIERCED_ENTITIES] = std::array::from_fn(|index| {
        accepted(index as i32, index as f64 + 0.25, index as f64 + 0.5)
    });
    let outcome = tick(
        &mut state,
        input(
            OwnerCollisionInput::missing(),
            Some(ArrowBlockHit::block(
                BLOCK,
                BlockPosition::new(150, 0, 0),
                Vec3::new(150.0, 0.0, 0.0),
            )),
            &mut hits,
        ),
    )
    .unwrap();
    assert_eq!(outcome.publications.len(), MAX_PIERCED_ENTITIES * 3 + 3);
    assert!(state.in_ground);
    assert!(state.pierced_entities.is_empty());
}

#[test]
fn ordinary_accepted_hit_discards_before_land_and_prevents_block_processing() {
    let mut state = arrow(Vec3::new(5.0, 0.0, 0.0), 0);
    let outcome = tick(
        &mut state,
        input(
            OwnerCollisionInput::missing(),
            Some(ArrowBlockHit::block(
                BLOCK,
                BlockPosition::new(4, 0, 0),
                Vec3::new(4.0, 0.0, 0.0),
            )),
            &mut [accepted(5, 1.0, 1.0)],
        ),
    )
    .unwrap();
    assert_eq!(state.projectile.lifecycle, ProjectileLifecycle::Discarded);
    let events = outcome.publications.iter().collect::<Vec<_>>();
    let discard = events
        .iter()
        .position(|event| matches!(event, ProjectilePublication::Discarded { .. }))
        .unwrap();
    let land = events
        .iter()
        .position(|event| matches!(event, ProjectilePublication::ProjectileLandedEntity { .. }))
        .unwrap();
    let shot = events
        .iter()
        .position(|event| matches!(event, ProjectilePublication::ProjectileShot { .. }))
        .unwrap();
    assert!(discard < land && land < shot);
}

#[test]
fn rejected_hit_uses_resolved_reverse_then_point_two_scale_and_low_speed_pickup() {
    let mut state = arrow(Vec3::new(1.0, 0.0, 0.0), 0);
    state.pickup = PickupMode::Allowed;
    let mut hit = accepted(8, 0.5, 0.5);
    hit.resolution = ArrowEntityResolution::Damage(ArrowDamageResolution::Rejected {
        reverse: ResolvedDeflection {
            velocity: Vec3::new(0.0001, 0.0, 0.0),
            yaw_delta: 177.0,
        },
    });
    let outcome = tick(
        &mut state,
        input(OwnerCollisionInput::missing(), None, &mut [hit]),
    )
    .unwrap();
    assert_eq!(
        state.projectile.velocity,
        Vec3::new(0.0001 * 0.2 * f64::from(0.99_f32), -0.05, 0.0)
    );
    assert_eq!(state.projectile.lifecycle, ProjectileLifecycle::Discarded);
    assert!(
        outcome
            .publications
            .iter()
            .any(|event| event == ProjectilePublication::PickupItemRequested)
    );
}

#[test]
fn owner_range_is_checked_before_arrow_candidates_and_world_border_deflects_without_landing() {
    let mut state = arrow(Vec3::new(2.0, 0.0, 0.0), 0);
    state.projectile.owner = Some(OWNER);
    let mut owner_hit = accepted(9, 1.0, 1.0);
    owner_hit.eligibility.shares_owner_vehicle = true;
    let outcome = tick(
        &mut state,
        input(
            OwnerCollisionInput::resolved(OWNER, &[]),
            None,
            &mut [owner_hit],
        ),
    )
    .unwrap();
    assert!(state.projectile.left_owner);
    assert!(outcome.publications.iter().any(|event| matches!(
        event,
        ProjectilePublication::EntityImpact { entity, .. }
            if entity == EntityId::new(9)
    )));

    let mut border = arrow(Vec3::new(2.0, 0.0, 0.0), 0);
    let hit = ArrowBlockHit::world_border(
        BLOCK,
        Vec3::new(1.0, 0.0, 0.0),
        ResolvedDeflection {
            velocity: Vec3::new(-1.0, 0.0, 0.0),
            yaw_delta: 170.0,
        },
    );
    let outcome = tick(
        &mut border,
        input(OwnerCollisionInput::missing(), Some(hit), &mut []),
    )
    .unwrap();
    assert!(!border.in_ground);
    assert!(!outcome.publications.iter().any(|event| matches!(
        event,
        ProjectilePublication::BlockImpact { .. }
            | ProjectilePublication::ProjectileLandedBlock { .. }
    )));
}

#[test]
fn arrow_owner_assignment_updates_pickup_mode_and_is_atomic() {
    let mut state = arrow(Vec3::ZERO, 0);
    assert_eq!(
        assign_arrow_owner(
            &mut state,
            1,
            Some(ArrowOwner::new(OWNER, ArrowOwnerKind::Player)),
        ),
        Err(ArrowMutationError::StaleState {
            expected: 1,
            actual: 0,
        })
    );
    assert_eq!(state.pickup, PickupMode::Disallowed);

    let outcome = assign_arrow_owner(
        &mut state,
        0,
        Some(ArrowOwner::new(OWNER, ArrowOwnerKind::Player)),
    )
    .unwrap();
    assert_eq!(state.pickup, PickupMode::Allowed);
    assert_eq!(outcome.pickup, PickupMode::Allowed);

    assign_arrow_owner(
        &mut state,
        1,
        Some(ArrowOwner::new(
            EntityIdentity::new(32),
            ArrowOwnerKind::OminousItemSpawner,
        )),
    )
    .unwrap();
    assert_eq!(state.pickup, PickupMode::Disallowed);

    state.projectile.revision = u64::MAX;
    let before = state;
    assert_eq!(
        assign_arrow_owner(&mut state, u64::MAX, None),
        Err(ArrowMutationError::RevisionExhausted)
    );
    assert_eq!(state, before);
}

#[test]
fn resolved_shoot_and_lerp_motion_reset_despawn_age_with_vanilla_in_ground_rule() {
    let mut state = arrow(Vec3::ZERO, 0);
    state.in_ground = true;
    state.despawn_age = 900;
    let shot = update_arrow_motion(
        &mut state,
        0,
        ArrowMotionUpdate::Shot {
            velocity: Vec3::new(1.0, 2.0, 3.0),
            rotation: Rotation::new(20.0, 30.0),
        },
    )
    .unwrap();
    assert_eq!(state.despawn_age, 0);
    assert!(state.in_ground);
    assert_eq!(shot.revision, RevisionTransition { from: 0, to: 1 });

    state.despawn_age = 33;
    update_arrow_motion(
        &mut state,
        1,
        ArrowMotionUpdate::Lerp {
            velocity: Vec3::ZERO,
        },
    )
    .unwrap();
    assert!(state.in_ground);
    update_arrow_motion(
        &mut state,
        2,
        ArrowMotionUpdate::Lerp {
            velocity: Vec3::new(0.0, f64::MIN_POSITIVE.sqrt(), 0.0),
        },
    )
    .unwrap();
    assert!(!state.in_ground);

    state.in_ground = true;
    state.despawn_age = 44;
    let old_velocity = state.projectile.velocity;
    let nan = update_arrow_motion(
        &mut state,
        3,
        ArrowMotionUpdate::Lerp {
            velocity: Vec3::new(f64::NAN, 0.0, 0.0),
        },
    )
    .unwrap();
    assert_eq!(state.projectile.velocity, old_velocity);
    assert_eq!(state.despawn_age, 0);
    assert!(state.in_ground);
    assert!(!nan.velocity_applied);
    assert!(!nan.cleared_in_ground);

    state.despawn_age = 55;
    let infinity = update_arrow_motion(
        &mut state,
        4,
        ArrowMotionUpdate::Lerp {
            velocity: Vec3::new(f64::INFINITY, 0.0, 0.0),
        },
    )
    .unwrap();
    assert_eq!(state.projectile.velocity, old_velocity);
    assert_eq!(state.despawn_age, 0);
    assert!(!state.in_ground);
    assert!(!infinity.velocity_applied);
    assert!(infinity.cleared_in_ground);

    let before = state;
    assert_eq!(
        update_arrow_motion(
            &mut state,
            1,
            ArrowMotionUpdate::Lerp {
                velocity: Vec3::new(1.0, 0.0, 0.0),
            },
        ),
        Err(ArrowMotionError::StaleState {
            expected: 1,
            actual: 5,
        })
    );
    assert_eq!(state, before);
    assert_eq!(
        update_arrow_motion(
            &mut state,
            5,
            ArrowMotionUpdate::Shot {
                velocity: Vec3::new(f64::NAN, 0.0, 0.0),
                rotation: Rotation::new(0.0, 0.0),
            },
        ),
        Err(ArrowMotionError::NonFiniteVelocity)
    );
    assert_eq!(state, before);

    assert_eq!(
        update_arrow_motion(
            &mut state,
            5,
            ArrowMotionUpdate::Shot {
                velocity: Vec3::new(1.0, 0.0, 0.0),
                rotation: Rotation::new(f32::INFINITY, 0.0),
            },
        ),
        Err(ArrowMotionError::NonFiniteRotation)
    );
    assert_eq!(state, before);

    state.projectile.revision = u64::MAX;
    let exhausted = state;
    assert_eq!(
        update_arrow_motion(
            &mut state,
            u64::MAX,
            ArrowMotionUpdate::Lerp {
                velocity: Vec3::ZERO,
            },
        ),
        Err(ArrowMotionError::RevisionExhausted)
    );
    assert_eq!(state, exhausted);

    let mut invalid = arrow(Vec3::ZERO, 0);
    invalid.projectile.position.x = f64::NAN;
    let before = invalid;
    assert_eq!(
        update_arrow_motion(
            &mut invalid,
            0,
            ArrowMotionUpdate::Lerp {
                velocity: Vec3::ZERO,
            },
        ),
        Err(ArrowMotionError::InvalidState(StateError::NonFinite(
            StateField::Position,
        )))
    );
    assert_eq!(
        invalid.projectile.position.x.to_bits(),
        before.projectile.position.x.to_bits()
    );
    invalid.projectile.position.x = 0.0;
    let mut sanitized_before = before;
    sanitized_before.projectile.position.x = 0.0;
    assert_eq!(invalid, sanitized_before);
}

#[test]
fn pickup_modes_return_typed_rejections_and_success_discards_atomically() {
    let mut state = arrow(Vec3::ZERO, 0);
    assert_eq!(
        pickup_arrow(&mut state, 0, PickupInput::default()).unwrap(),
        PickupOutcome::Rejected(PickupRejection::NotAccessible)
    );
    state.in_ground = true;
    state.shake_time = 1;
    assert_eq!(
        pickup_arrow(&mut state, 0, PickupInput::default()).unwrap(),
        PickupOutcome::Rejected(PickupRejection::Shaking)
    );
    state.shake_time = 0;
    assert_eq!(
        pickup_arrow(&mut state, 0, PickupInput::default()).unwrap(),
        PickupOutcome::Rejected(PickupRejection::Disallowed)
    );
    state.pickup = PickupMode::Allowed;
    assert_eq!(
        pickup_arrow(&mut state, 0, PickupInput::default()).unwrap(),
        PickupOutcome::Rejected(PickupRejection::InventoryRejected)
    );
    state.pickup = PickupMode::CreativeOnly;
    assert_eq!(
        pickup_arrow(&mut state, 0, PickupInput::default()).unwrap(),
        PickupOutcome::Rejected(PickupRejection::RequiresInfiniteMaterials)
    );

    let before = state;
    assert_eq!(
        pickup_arrow(&mut state, 2, PickupInput::default()),
        Err(ArrowMutationError::StaleState {
            expected: 2,
            actual: 0,
        })
    );
    assert_eq!(state, before);
    let result = pickup_arrow(
        &mut state,
        0,
        PickupInput {
            has_infinite_materials: true,
            inventory_inserted: false,
        },
    )
    .unwrap();
    let PickupOutcome::PickedUp { publication, .. } = result else {
        panic!("eligible pickup must succeed");
    };
    assert_eq!(
        publication,
        ProjectilePublication::Discarded {
            reason: DiscardReason::PickedUp,
        }
    );
    assert_eq!(state.projectile.lifecycle, ProjectileLifecycle::Discarded);
    assert_eq!(
        pickup_arrow(&mut state, 1, PickupInput::default()).unwrap(),
        PickupOutcome::Rejected(PickupRejection::Discarded)
    );

    let mut exhausted = arrow(Vec3::ZERO, 0);
    exhausted.in_ground = true;
    exhausted.pickup = PickupMode::CreativeOnly;
    exhausted.projectile.revision = u64::MAX;
    let before = exhausted;
    assert_eq!(
        pickup_arrow(
            &mut exhausted,
            u64::MAX,
            PickupInput {
                has_infinite_materials: true,
                inventory_inserted: false,
            },
        ),
        Err(ArrowMutationError::RevisionExhausted)
    );
    assert_eq!(exhausted, before);
}

#[test]
fn piercing_ledger_is_fixed_deduplicated_and_reports_capacity() {
    let mut ledger = PiercingLedger::new();
    assert_eq!(ledger.record(EntityId::new(1)), Ok(LedgerRecord::Inserted));
    assert_eq!(
        ledger.record(EntityId::new(1)),
        Ok(LedgerRecord::AlreadyPresent)
    );
    for raw in 2..=MAX_PIERCED_ENTITIES as i32 {
        ledger.record(EntityId::new(raw)).unwrap();
    }
    assert_eq!(ledger.len(), MAX_PIERCED_ENTITIES);
    assert_eq!(
        ledger.record(EntityId::new(10_000)),
        Err(PiercingLedgerError::CapacityExceeded {
            capacity: MAX_PIERCED_ENTITIES,
        })
    );
}

#[test]
fn arrow_prepare_rejects_reachable_input_failures_without_mutation() {
    let base = arrow(Vec3::new(1.0, 0.0, 0.0), 0);
    let mut invalid = base;
    invalid.projectile.rotation.yaw = f32::NAN;
    assert_eq!(
        prepare_arrow_tick(
            &invalid,
            input(OwnerCollisionInput::missing(), None, &mut []),
        ),
        Err(ArrowPrepareError::InvalidState(StateError::NonFinite(
            StateField::Rotation,
        )))
    );

    let mut owned = base;
    owned.projectile.owner = Some(OWNER);
    assert_eq!(
        prepare_arrow_tick(
            &owned,
            input(
                OwnerCollisionInput::resolved(EntityIdentity::new(999), &[]),
                None,
                &mut [],
            ),
        ),
        Err(ArrowPrepareError::Owner(OwnerInputError::OwnerMismatch {
            expected: OWNER,
            actual: EntityIdentity::new(999),
        }))
    );
    let mut discarded = base;
    discarded.projectile.lifecycle = ProjectileLifecycle::Discarded;
    assert_eq!(
        prepare_arrow_tick(
            &discarded,
            input(OwnerCollisionInput::missing(), None, &mut []),
        ),
        Err(ArrowPrepareError::Discarded)
    );
    let mut exhausted = base;
    exhausted.projectile.revision = u64::MAX;
    assert_eq!(
        prepare_arrow_tick(
            &exhausted,
            input(OwnerCollisionInput::missing(), None, &mut []),
        ),
        Err(ArrowPrepareError::RevisionExhausted)
    );

    let mut bad = accepted(1, 0.5, 0.5);
    bad.entity_position.x = f64::NAN;
    assert_eq!(
        prepare_arrow_tick(
            &base,
            input(OwnerCollisionInput::missing(), None, &mut [bad]),
        ),
        Err(ArrowPrepareError::NonFinite(
            ArrowInputField::EntityPosition,
        ))
    );
    let mut bad_location = accepted(2, 0.5, 0.5);
    bad_location.location.z = f64::INFINITY;
    assert_eq!(
        prepare_arrow_tick(
            &base,
            input(OwnerCollisionInput::missing(), None, &mut [bad_location]),
        ),
        Err(ArrowPrepareError::NonFinite(
            ArrowInputField::EntityHitLocation,
        ))
    );

    let bad_block = ArrowBlockHit::block(
        BLOCK,
        BlockPosition::new(0, 0, 0),
        Vec3::new(f64::NAN, 0.0, 0.0),
    );
    assert_eq!(
        prepare_arrow_tick(
            &base,
            input(OwnerCollisionInput::missing(), Some(bad_block), &mut []),
        ),
        Err(ArrowPrepareError::NonFinite(
            ArrowInputField::BlockHitLocation,
        ))
    );

    let bad_deflection = ArrowEntityHit {
        resolution: ArrowEntityResolution::Deflected(ResolvedDeflection {
            velocity: Vec3::new(f64::NAN, 0.0, 0.0),
            yaw_delta: 0.0,
        }),
        ..accepted(3, 0.5, 0.5)
    };
    assert_eq!(
        prepare_arrow_tick(
            &base,
            input(OwnerCollisionInput::missing(), None, &mut [bad_deflection]),
        ),
        Err(ArrowPrepareError::NonFinite(
            ArrowInputField::DeflectionVelocity,
        ))
    );
    let bad_yaw = ArrowEntityHit {
        resolution: ArrowEntityResolution::Deflected(ResolvedDeflection {
            velocity: Vec3::ZERO,
            yaw_delta: f32::NAN,
        }),
        ..accepted(4, 0.5, 0.5)
    };
    assert_eq!(
        prepare_arrow_tick(
            &base,
            input(OwnerCollisionInput::missing(), None, &mut [bad_yaw]),
        ),
        Err(ArrowPrepareError::NonFinite(ArrowInputField::DeflectionYaw,))
    );
    let mut overflow_rotation = base;
    overflow_rotation.projectile.rotation.yaw = f32::MAX;
    let overflowing_deflection = ArrowEntityHit {
        resolution: ArrowEntityResolution::Deflected(ResolvedDeflection {
            velocity: Vec3::ZERO,
            yaw_delta: f32::MAX,
        }),
        ..accepted(5, 0.5, 0.5)
    };
    assert_eq!(
        prepare_arrow_tick(
            &overflow_rotation,
            input(
                OwnerCollisionInput::missing(),
                None,
                &mut [overflowing_deflection],
            ),
        ),
        Err(ArrowPrepareError::NonFinite(
            ArrowInputField::ComputedRotation,
        ))
    );

    let bad_border = ArrowBlockHit::world_border(
        BLOCK,
        Vec3::new(0.5, 0.0, 0.0),
        ResolvedDeflection {
            velocity: Vec3::new(f64::INFINITY, 0.0, 0.0),
            yaw_delta: 0.0,
        },
    );
    assert_eq!(
        prepare_arrow_tick(
            &base,
            input(OwnerCollisionInput::missing(), Some(bad_border), &mut []),
        ),
        Err(ArrowPrepareError::NonFinite(
            ArrowInputField::DeflectionVelocity,
        ))
    );
    assert_eq!(
        prepare_arrow_tick(
            &base,
            input(
                OwnerCollisionInput::missing(),
                None,
                &mut [accepted(1, 0.5, 0.5), accepted(1, 0.7, 0.7)],
            ),
        ),
        Err(ArrowPrepareError::DuplicateCandidate(EntityId::new(1)))
    );

    let mut grounded = base;
    grounded.in_ground = true;
    grounded.last_block_state = Some(BlockStateId::new(90));
    let mut missing_scale = input(OwnerCollisionInput::missing(), None, &mut []);
    missing_scale.should_fall = true;
    assert_eq!(
        prepare_arrow_tick(&grounded, missing_scale),
        Err(ArrowPrepareError::MissingFallVelocityScale)
    );
    let mut invalid_scale = input(OwnerCollisionInput::missing(), None, &mut []);
    invalid_scale.should_fall = true;
    invalid_scale.fall_velocity_scale = Some(Vec3::new(0.2, 0.0, 0.0));
    assert_eq!(
        prepare_arrow_tick(&grounded, invalid_scale),
        Err(ArrowPrepareError::FallVelocityScaleOutOfRange)
    );
}

#[test]
fn arrow_commit_rejects_all_stale_preconditions_atomically() {
    let base = arrow(Vec3::new(1.0, 0.0, 0.0), 0);
    let plan =
        prepare_arrow_tick(&base, input(OwnerCollisionInput::missing(), None, &mut [])).unwrap();
    let mut stale = base;
    stale.projectile.revision = 1;
    let before = stale;
    assert_eq!(
        commit_arrow_tick(&mut stale, stamp(7), plan),
        Err(TickCommitError::StaleState {
            expected: 0,
            actual: 1,
        })
    );
    assert_eq!(stale, before);

    let mut changed = base;
    changed.shake_time = 1;
    assert_eq!(
        commit_arrow_tick(&mut changed, stamp(7), plan),
        Err(TickCommitError::StateChangedAtRevision { revision: 0 })
    );
    assert_eq!(changed.shake_time, 1);

    for (actual, error) in [
        (
            InputStamp {
                world_revision: 8,
                ..stamp(7)
            },
            TickCommitError::StaleWorld {
                expected: 7,
                actual: 8,
            },
        ),
        (
            InputStamp {
                collision_revision: 8,
                ..stamp(7)
            },
            TickCommitError::StaleCollisions {
                expected: 7,
                actual: 8,
            },
        ),
        (
            InputStamp {
                resolution_revision: 8,
                ..stamp(7)
            },
            TickCommitError::StaleResolutions {
                expected: 7,
                actual: 8,
            },
        ),
    ] {
        let mut current = base;
        assert_eq!(commit_arrow_tick(&mut current, actual, plan), Err(error));
        assert_eq!(current, base);
    }
}
