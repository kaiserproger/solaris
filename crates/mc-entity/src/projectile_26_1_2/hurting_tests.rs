use super::*;

const OWNER: EntityIdentity = EntityIdentity::new(9);
const BLOCK: BlockStateId = BlockStateId::new(17);

fn stamp(value: u64) -> InputStamp {
    InputStamp {
        world_revision: value,
        collision_revision: value,
        resolution_revision: value,
    }
}

fn impact(entity: i32, location: Vec3) -> ThrowableEntityHit {
    ThrowableEntityHit {
        entity: EntityId::new(entity),
        location,
        eligibility: HitEligibility {
            can_be_hit_by_projectile: true,
            arrow_pvp_permitted: true,
            shares_owner_vehicle: false,
        },
        resolution: EntityHitResolution::Impact,
        input_order: 0,
    }
}

fn state(direction: Vec3) -> HurtingProjectileState {
    HurtingProjectileState::new(
        Some(OWNER),
        Vec3::new(1.0, 2.0, 3.0),
        Aabb::new(0.8, 1.8, 2.8, 1.2, 2.2, 3.2).unwrap(),
        direction,
        Rotation::new(0.0, 0.0),
        HURTING_PROJECTILE_DEFAULT_ACCELERATION_POWER,
    )
    .unwrap()
}

#[test]
fn hurting_projectile_initial_direction_is_normalized_to_point_one() {
    let state = state(Vec3::new(3.0, 4.0, 0.0));

    assert_eq!(
        state.projectile.velocity.x.to_bits(),
        (3.0_f64 * 25.0_f64.sqrt().recip() * 0.1).to_bits()
    );
    assert_eq!(
        state.projectile.velocity.y.to_bits(),
        (4.0_f64 * 25.0_f64.sqrt().recip() * 0.1).to_bits()
    );
    assert_eq!(state.projectile.velocity.z.to_bits(), 0.0_f64.to_bits());
    assert_eq!(state.projectile.owner, Some(OWNER));
    assert_eq!(state.projectile.revision, 0);
}

#[test]
fn hurting_projectile_air_inertia_matches_26_1_2_ordering() {
    let state = state(Vec3::new(1.0, 0.0, 0.0));

    let velocity = state.next_velocity(false).unwrap();

    assert_eq!(
        velocity.x.to_bits(),
        ((0.1_f64 + 0.1_f64) * (0.95_f32 as f64)).to_bits()
    );
    assert_eq!(velocity.y.to_bits(), 0.0_f64.to_bits());
    assert_eq!(velocity.z.to_bits(), 0.0_f64.to_bits());
}

#[test]
fn hurting_projectile_water_inertia_uses_float_26_1_2_constant() {
    let state = state(Vec3::new(0.0, 0.0, 1.0));

    let velocity = state.next_velocity(true).unwrap();

    assert_eq!(
        velocity.z.to_bits(),
        ((0.1_f64 + 0.1_f64) * (0.8_f32 as f64)).to_bits()
    );
}

#[test]
fn hurting_projectile_rotation_converges_towards_updated_velocity() {
    let state = state(Vec3::new(1.0, 0.0, 0.0));
    let velocity = Vec3::new(0.0, 1.0, 1.0);

    let rotation = state.next_rotation(velocity).unwrap();
    let target = target_rotation(velocity, false);

    assert_eq!(
        rotation.yaw.to_bits(),
        lerp_rotation(0.0, target.yaw).unwrap().to_bits()
    );
    assert_eq!(
        rotation.pitch.to_bits(),
        lerp_rotation(0.0, target.pitch).unwrap().to_bits()
    );
}

#[test]
fn hurting_projectile_advance_increments_revision_and_can_discard() {
    let state = state(Vec3::new(1.0, 0.0, 0.0));
    let velocity = state.next_velocity(false).unwrap();
    let rotation = state.next_rotation(velocity).unwrap();
    let position = state.projectile.position.plus(velocity);

    let advanced = state.advance(position, velocity, rotation, true).unwrap();

    assert_eq!(advanced.projectile.revision, 1);
    assert_eq!(advanced.projectile.position, position);
    assert_eq!(advanced.projectile.velocity, velocity);
    assert_eq!(advanced.projectile.rotation, rotation);
    assert!(advanced.projectile.has_been_shot);
    assert_eq!(
        advanced.projectile.lifecycle,
        ProjectileLifecycle::Discarded
    );
}

#[test]
fn hurting_projectile_rejects_invalid_acceleration_and_velocity() {
    let bounds = Aabb::new(0.0, 0.0, 0.0, 1.0, 1.0, 1.0).unwrap();
    assert_eq!(
        HurtingProjectileState::new(
            None,
            Vec3::ZERO,
            bounds,
            Vec3::new(1.0, 0.0, 0.0),
            Rotation::new(0.0, 0.0),
            f64::NAN,
        ),
        Err(HurtingProjectileError::NonFiniteAcceleration)
    );
    assert_eq!(
        HurtingProjectileState::new(
            None,
            Vec3::ZERO,
            bounds,
            Vec3::new(1.0, 0.0, 0.0),
            Rotation::new(0.0, 0.0),
            -0.1,
        ),
        Err(HurtingProjectileError::NegativeAcceleration)
    );

    let mut invalid = state(Vec3::new(1.0, 0.0, 0.0));
    invalid.projectile.velocity.x = f64::INFINITY;
    assert_eq!(
        invalid.next_velocity(false),
        Err(HurtingProjectileError::NonFiniteVelocity)
    );
}

#[test]
fn hurting_projectile_miss_moves_by_post_inertia_velocity() {
    let mut state = state(Vec3::new(1.0, 0.0, 0.0));
    let start = state.projectile.position;
    let expected_velocity = state.next_velocity(false).unwrap();
    let tick_stamp = stamp(10);
    let plan = prepare_hurting_projectile_tick(
        &state,
        HurtingProjectileTickInput {
            stamp: tick_stamp,
            in_water: false,
            owner_collision: OwnerCollisionInput::resolved(OWNER, &[]),
            block_hit: None,
            entity_hits: &mut [],
        },
    )
    .unwrap();

    let outcome = commit_hurting_projectile_tick(&mut state, tick_stamp, plan).unwrap();

    assert_eq!(outcome.hit, HitTarget::Miss);
    assert_eq!(state.projectile.velocity, expected_velocity);
    assert_eq!(state.projectile.position, start.plus(expected_velocity));
    assert_eq!(state.projectile.revision, 1);
}

#[test]
fn hurting_projectile_tick_orders_entity_before_block_and_discards_on_impact() {
    let mut state = state(Vec3::new(1.0, 0.0, 0.0));
    let start = state.projectile.position;
    let original_bounds = state.projectile.bounds;
    let entity_location = Vec3::new(start.x + 0.04, start.y, start.z);
    let block_location = Vec3::new(start.x + 0.08, start.y, start.z);
    let mut hits = [impact(12, entity_location)];
    let tick_stamp = stamp(11);
    let plan = prepare_hurting_projectile_tick(
        &state,
        HurtingProjectileTickInput {
            stamp: tick_stamp,
            in_water: false,
            owner_collision: OwnerCollisionInput::resolved(OWNER, &[]),
            block_hit: Some(BlockHit {
                block_state: BLOCK,
                location: block_location,
            }),
            entity_hits: &mut hits,
        },
    )
    .unwrap();

    let outcome = commit_hurting_projectile_tick(&mut state, tick_stamp, plan).unwrap();

    assert_eq!(
        outcome.hit,
        HitTarget::Entity {
            entity: EntityId::new(12),
            location: entity_location,
        }
    );
    assert_eq!(state.projectile.position, entity_location);
    assert_eq!(state.projectile.lifecycle, ProjectileLifecycle::Discarded);
    assert_eq!(state.projectile.revision, 1);
    assert_eq!(
        state.projectile.bounds.min_x - original_bounds.min_x,
        entity_location.x - start.x
    );
}

#[test]
fn hurting_projectile_tick_blocks_entity_tied_at_or_behind_block_endpoint() {
    let mut state = state(Vec3::new(1.0, 0.0, 0.0));
    let start = state.projectile.position;
    let block_location = Vec3::new(start.x + 0.08, start.y, start.z);
    let mut hits = [impact(12, block_location)];
    let tick_stamp = stamp(12);
    let plan = prepare_hurting_projectile_tick(
        &state,
        HurtingProjectileTickInput {
            stamp: tick_stamp,
            in_water: false,
            owner_collision: OwnerCollisionInput::resolved(OWNER, &[]),
            block_hit: Some(BlockHit {
                block_state: BLOCK,
                location: block_location,
            }),
            entity_hits: &mut hits,
        },
    )
    .unwrap();

    let outcome = commit_hurting_projectile_tick(&mut state, tick_stamp, plan).unwrap();

    assert_eq!(
        outcome.hit,
        HitTarget::Block {
            block_state: BLOCK,
            location: block_location,
        }
    );
    assert_eq!(state.projectile.lifecycle, ProjectileLifecycle::Discarded);
}

#[test]
fn hurting_projectile_commit_rejects_stale_world_facts_without_mutation() {
    let mut state = state(Vec3::new(1.0, 0.0, 0.0));
    let before = state;
    let plan = prepare_hurting_projectile_tick(
        &state,
        HurtingProjectileTickInput {
            stamp: stamp(3),
            in_water: false,
            owner_collision: OwnerCollisionInput::resolved(OWNER, &[]),
            block_hit: None,
            entity_hits: &mut [],
        },
    )
    .unwrap();

    assert!(matches!(
        commit_hurting_projectile_tick(&mut state, stamp(4), plan),
        Err(TickCommitError::StaleWorld { .. })
    ));
    assert_eq!(state, before);
}
