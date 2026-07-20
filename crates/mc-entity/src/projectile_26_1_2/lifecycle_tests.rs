use super::*;

const OWNER: EntityIdentity = EntityIdentity::new(0x1234);
const OTHER_OWNER: EntityIdentity = EntityIdentity::new(0x5678);

fn bounds_at(x: f64, y: f64, z: f64) -> Aabb {
    Aabb::new(x - 0.25, y, z - 0.25, x + 0.25, y + 0.5, z + 0.25).unwrap()
}

fn projectile(owner: Option<EntityIdentity>) -> ProjectileState {
    ProjectileState::new(
        owner,
        Vec3::new(0.0, 1.0, 0.0),
        bounds_at(0.0, 1.0, 0.0),
        Vec3::new(2.0, 0.0, 0.0),
        Rotation::new(0.0, 0.0),
    )
    .unwrap()
}

#[test]
fn geometry_rejects_each_nonfinite_boundary_and_canonicalizes_inverted_axes() {
    for (field, index) in [
        (GeometryField::MinX, 0),
        (GeometryField::MinY, 1),
        (GeometryField::MinZ, 2),
        (GeometryField::MaxX, 3),
        (GeometryField::MaxY, 4),
        (GeometryField::MaxZ, 5),
    ] {
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut coordinates = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
            coordinates[index] = invalid;
            let expected = if invalid.is_nan() {
                GeometryError::NaN(field)
            } else {
                GeometryError::Infinite(field)
            };
            assert_eq!(
                Aabb::new(
                    coordinates[0],
                    coordinates[1],
                    coordinates[2],
                    coordinates[3],
                    coordinates[4],
                    coordinates[5],
                ),
                Err(expected)
            );
        }
    }

    assert_eq!(
        Aabb::new(2.0, 3.0, 4.0, 1.0, 0.0, -1.0).unwrap(),
        Aabb {
            min_x: 1.0,
            min_y: 0.0,
            min_z: -1.0,
            max_x: 2.0,
            max_y: 3.0,
            max_z: 4.0,
        }
    );
}

#[test]
fn owner_range_uses_expand_towards_then_one_block_inflation_and_strict_intersection() {
    let state = projectile(Some(OWNER));
    let overlapping_ahead = OwnerVehicleMember {
        pickable: true,
        bounds: Aabb::new(2.9, 1.0, -0.1, 3.1, 1.2, 0.1).unwrap(),
    };
    let unpickable_overlap = OwnerVehicleMember {
        pickable: false,
        bounds: overlapping_ahead.bounds,
    };
    let exactly_touching = OwnerVehicleMember {
        pickable: true,
        bounds: Aabb::new(3.25, 1.0, -0.1, 3.5, 1.2, 0.1).unwrap(),
    };

    assert!(
        !outside_owner_collision_range(
            &state,
            OwnerCollisionInput::resolved(OWNER, &[overlapping_ahead]),
        )
        .unwrap()
    );
    assert!(
        outside_owner_collision_range(
            &state,
            OwnerCollisionInput::resolved(OWNER, &[unpickable_overlap]),
        )
        .unwrap()
    );
    assert!(
        outside_owner_collision_range(
            &state,
            OwnerCollisionInput::resolved(OWNER, &[exactly_touching]),
        )
        .unwrap()
    );
}

#[test]
fn missing_owner_resolution_marks_the_projectile_as_outside() {
    assert!(
        outside_owner_collision_range(&projectile(None), OwnerCollisionInput::missing()).unwrap()
    );
    assert!(
        outside_owner_collision_range(&projectile(Some(OWNER)), OwnerCollisionInput::missing())
            .unwrap()
    );
}

#[test]
fn owner_range_rejects_mismatch_and_members_without_owner() {
    let state = projectile(Some(OWNER));
    assert_eq!(
        outside_owner_collision_range(&state, OwnerCollisionInput::resolved(OTHER_OWNER, &[]),),
        Err(OwnerInputError::OwnerMismatch {
            expected: OWNER,
            actual: OTHER_OWNER,
        })
    );
    assert_eq!(
        outside_owner_collision_range(
            &state,
            OwnerCollisionInput {
                resolved_owner: None,
                vehicle_members: &[OwnerVehicleMember {
                    pickable: true,
                    bounds: bounds_at(0.0, 0.0, 0.0),
                }],
            },
        ),
        Err(OwnerInputError::MembersWithoutResolvedOwner)
    );

    let ownerless = projectile(None);
    assert_eq!(
        validate_owner_collision_input(&ownerless, OwnerCollisionInput::resolved(OWNER, &[]),),
        Err(OwnerInputError::UnexpectedResolvedOwner(OWNER))
    );
}

#[test]
fn owner_range_streams_complete_dense_vehicle_members() {
    let state = projectile(Some(OWNER));
    let mut members = vec![
        OwnerVehicleMember {
            pickable: false,
            bounds: bounds_at(100.0, 100.0, 100.0),
        };
        256
    ];
    members[255] = OwnerVehicleMember {
        pickable: true,
        bounds: bounds_at(2.0, 1.0, 0.0),
    };

    assert!(
        !outside_owner_collision_range(&state, OwnerCollisionInput::resolved(OWNER, &members),)
            .unwrap()
    );
}

#[test]
fn owner_range_rejects_invalid_member_bounds() {
    let state = projectile(Some(OWNER));
    let mut member = OwnerVehicleMember {
        pickable: true,
        bounds: bounds_at(0.0, 1.0, 0.0),
    };
    member.bounds.max_z = f64::INFINITY;

    assert_eq!(
        outside_owner_collision_range(&state, OwnerCollisionInput::resolved(OWNER, &[member]),),
        Err(OwnerInputError::InvalidVehicleMemberBounds {
            index: 0,
            error: GeometryError::Infinite(GeometryField::MaxZ),
        })
    );
}

#[test]
fn owner_range_rejects_invalid_state_and_swept_bounds_overflow() {
    let mut invalid = projectile(Some(OWNER));
    invalid.bounds.min_x = f64::NAN;
    assert_eq!(
        outside_owner_collision_range(&invalid, OwnerCollisionInput::resolved(OWNER, &[]),),
        Err(OwnerInputError::InvalidProjectileState(
            StateError::InvalidBounds(GeometryError::NaN(GeometryField::MinX)),
        ))
    );

    let huge = Aabb::new(0.0, 0.0, 0.0, f64::MAX, 1.0, 1.0).unwrap();
    let state = ProjectileState::new(
        Some(OWNER),
        Vec3::ZERO,
        huge,
        Vec3::new(f64::MAX, 0.0, 0.0),
        Rotation::default(),
    )
    .unwrap();
    assert_eq!(
        outside_owner_collision_range(&state, OwnerCollisionInput::resolved(OWNER, &[]),),
        Err(OwnerInputError::SweptBoundsOverflow)
    );
}

#[test]
fn assigning_owner_is_revisioned_atomic_and_does_not_reset_left_owner() {
    let mut state = projectile(Some(OWNER));
    state.left_owner = true;
    let before = state;

    assert_eq!(
        assign_owner(&mut state, 1, Some(OTHER_OWNER)),
        Err(MutationError::StaleState {
            expected: 1,
            actual: 0,
        })
    );
    assert_eq!(state, before);

    assert_eq!(
        assign_owner(&mut state, 0, Some(OTHER_OWNER)),
        Ok(OwnerMutation {
            previous: Some(OWNER),
            current: Some(OTHER_OWNER),
            revision: RevisionTransition { from: 0, to: 1 },
        })
    );
    assert!(state.left_owner);
    assert_eq!(state.owner, Some(OTHER_OWNER));
}

#[test]
fn assigning_owner_rejects_revision_exhaustion_without_mutation() {
    let mut state = projectile(Some(OWNER));
    state.revision = u64::MAX;
    let before = state;
    assert_eq!(
        assign_owner(&mut state, u64::MAX, None),
        Err(MutationError::RevisionExhausted)
    );
    assert_eq!(state, before);
}

#[test]
fn publication_batch_is_fixed_and_preserves_insertion_order() {
    let mut batch = PublicationBatch::new();
    for index in 0..MAX_PUBLICATIONS {
        batch
            .push(ProjectilePublication::EntityImpact {
                entity: EntityId::new(index as i32),
                location: Vec3::new(index as f64, 0.0, 0.0),
            })
            .unwrap();
    }
    assert_eq!(batch.len(), MAX_PUBLICATIONS);
    assert_eq!(
        batch.push(ProjectilePublication::ProjectileShot { owner: None }),
        Err(PublicationError::CapacityExceeded {
            capacity: MAX_PUBLICATIONS,
        })
    );
    assert_eq!(
        batch.get(0),
        Some(ProjectilePublication::EntityImpact {
            entity: EntityId::new(0),
            location: Vec3::new(0.0, 0.0, 0.0),
        })
    );
    assert_eq!(
        batch.get(MAX_PUBLICATIONS - 1),
        Some(ProjectilePublication::EntityImpact {
            entity: EntityId::new((MAX_PUBLICATIONS - 1) as i32),
            location: Vec3::new((MAX_PUBLICATIONS - 1) as f64, 0.0, 0.0),
        })
    );
    assert_eq!(batch.get(MAX_PUBLICATIONS), None);
}

#[test]
fn rotation_lerp_matches_vanilla_wrap_boundaries_without_unbounded_loops() {
    for (previous, target, expected) in [
        (-179.0, 179.0, 180.6),
        (179.0, -179.0, -180.6),
        (0.0, -180.0, -36.0),
        (0.0, 180.0, 324.0),
        (901.0, -179.0, -179.0),
    ] {
        assert_eq!(
            super::lifecycle::lerp_rotation(previous, target)
                .expect("ordinary vanilla rotation is representable")
                .to_bits(),
            (expected as f32).to_bits()
        );
    }
    assert_eq!(super::lifecycle::lerp_rotation(f32::MAX, 0.0), None);
}
