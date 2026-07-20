use super::*;

fn stamp(revision: u64) -> InputStamp {
    InputStamp {
        entity_revision: revision,
        facts_revision: revision,
    }
}

fn ground_facts(position: Vec3, yaw: f32, movement_speed: f64) -> MoveFacts {
    MoveFacts {
        position,
        yaw,
        pitch: 0.0,
        movement_speed,
        flying_speed: None,
        max_up_step: 0.6,
        body_width: 0.6,
        on_ground: true,
        in_liquid: false,
        affected_by_fluids: true,
        in_water: false,
        navigation_done: Some(true),
        strafe_navigation_present: None,
        strafe_evaluator_present: None,
        walkability: None,
        current_vertical_velocity: None,
        collision: Some(None),
    }
}

#[test]
fn wrap_and_rotlerp_match_vanilla_boundaries() {
    assert_eq!(wrap_degrees(180.0), -180.0);
    assert_eq!(wrap_degrees(-180.0), -180.0);
    assert_eq!(rotlerp(350.0, 10.0, 90.0), 370.0 - 360.0);
    assert_eq!(rotlerp(270.0, 360.0, 90.0), 360.0);
    assert_eq!(rotlerp(0.0, 270.0, 45.0), 315.0);
    assert_eq!(rotlerp(0.0, 10.0, -5.0), 5.0);
    assert_eq!(rotate_towards(0.0, 10.0, -5.0), -5.0);
}

#[test]
fn target_angles_match_mth_atan2_oracle_bits() {
    assert_eq!(target_yaw(2.0, 1.0).to_bits(), 0xc27d_bd68);
    assert_eq!(target_yaw(1.0, 2.0).to_bits(), 0xc1d4_8530);
    assert_eq!(target_pitch(1.0, 2.0).to_bits(), 0xc1d4_852f);
}

#[test]
fn strafe_normalizes_only_vectors_longer_than_one_and_uses_walkability() {
    let mut control = MoveControlState::default();
    control.strafe(3.0, 4.0);
    let mut facts = ground_facts(Vec3::ZERO, 90.0, 0.2);
    facts.strafe_navigation_present = Some(true);
    facts.strafe_evaluator_present = Some(true);
    let probe = strafe_walkability_probe(&control, facts).unwrap();
    assert_eq!(probe.x.to_bits(), 0xbd23_d70a);
    assert_eq!(probe.z.to_bits(), 0x3cf5_c28f);
    facts.walkability = Some(WalkabilityFact::new(probe, false));
    let plan = prepare_move(&control, stamp(1), facts).unwrap();
    assert_eq!(plan.output.speed, Some(0.05));
    assert_eq!(plan.output.forward, Some(1.0));
    assert_eq!(plan.output.strafe, Some(0.0));
    assert_eq!(plan.next.operation, MoveOperation::Wait);

    let mut short = MoveControlState::default();
    short.strafe(0.3, 0.4);
    let mut short_facts = ground_facts(Vec3::ZERO, 0.0, 0.2);
    short_facts.strafe_navigation_present = Some(true);
    short_facts.strafe_evaluator_present = Some(true);
    let short_probe = strafe_walkability_probe(&short, short_facts).unwrap();
    assert_eq!(short_probe.x.to_bits(), 0x3c75_c290);
    assert_eq!(short_probe.z.to_bits(), 0x3ca3_d70b);
    short_facts.walkability = Some(WalkabilityFact::new(short_probe, true));
    let short_plan = prepare_move(&short, stamp(1), short_facts).unwrap();
    assert_eq!(short_plan.output.forward, Some(0.3));
    assert_eq!(short_plan.output.strafe, Some(0.4));
}

#[test]
fn move_to_uses_step_and_collision_jump_triggers() {
    let mut control = MoveControlState::default();
    control.move_to(Vec3::new(0.25, 1.0, 0.25), 2.0);
    let facts = ground_facts(Vec3::ZERO, 0.0, 0.3).with_body(0.6, 0.6);
    let plan = prepare_move(&control, stamp(2), facts).unwrap();
    assert_eq!(plan.next.operation, MoveOperation::Jumping);
    assert_eq!(plan.output.jump_requested, Some(true));

    let mut collision = MoveControlState::default();
    collision.move_to(Vec3::new(3.0, 0.0, 0.0), 1.0);
    let facts =
        ground_facts(Vec3::new(0.0, 0.25, 0.0), 0.0, 0.2).with_collision(Some(CollisionFact {
            top_y: 0.5,
            is_door: false,
            is_fence: false,
        }));
    assert_eq!(
        prepare_move(&collision, stamp(3), facts)
            .unwrap()
            .output
            .jump_requested,
        Some(true)
    );
}

#[test]
fn jumping_waits_for_ground_or_affected_liquid() {
    let control = MoveControlState {
        operation: MoveOperation::Jumping,
        speed_modifier: 2.0,
        ..Default::default()
    };
    let mut facts = ground_facts(Vec3::ZERO, 0.0, 0.25);
    facts.on_ground = false;
    facts.in_liquid = true;
    facts.affected_by_fluids = false;
    assert_eq!(
        prepare_move(&control, stamp(4), facts)
            .unwrap()
            .next
            .operation,
        MoveOperation::Jumping
    );
    facts.affected_by_fluids = true;
    assert_eq!(
        prepare_move(&control, stamp(4), facts)
            .unwrap()
            .next
            .operation,
        MoveOperation::Wait
    );
}

#[test]
fn flying_and_swimming_branches_follow_oracle_facts() {
    let mut control = MoveControlState::default();
    control.move_to(Vec3::new(0.0, 2.0, 2.0), 2.0);
    let mut facts = ground_facts(Vec3::ZERO, 0.0, 0.2);
    facts.on_ground = false;
    facts.flying_speed = Some(0.4);
    let flying = prepare_flying_move(
        &control,
        stamp(5),
        facts,
        FlyingConfig {
            max_pitch_turn: 20,
            hovers_in_place: false,
        },
    )
    .unwrap();
    assert_eq!(flying.output.no_gravity, Some(true));
    assert_eq!(flying.output.speed, Some(0.8));
    assert_eq!(flying.output.vertical, Some(0.8));

    let mut water = facts;
    water.in_water = true;
    water.navigation_done = Some(false);
    water.current_vertical_velocity = Some(CurrentVerticalVelocityFact::new(stamp(5), -0.002));
    let swimming = prepare_smooth_swimming_move(
        &control,
        stamp(5),
        water,
        SwimmingConfig {
            max_pitch: 30,
            max_yaw_turn: 10,
            in_water_speed_modifier: 0.5,
            outside_water_speed_modifier: 0.8,
            apply_gravity: true,
        },
    )
    .unwrap();
    assert_eq!(
        swimming.output.vertical_velocity_change,
        Some(VerticalVelocityChange::Additive {
            expected_current: -0.002,
            delta: 0.005,
            result: f64::from_bits(0x3f68_9374_bc6a_7efa),
        })
    );
    assert_eq!(swimming.output.forward.unwrap().to_bits(), 0x3ecc_04f4);
    assert_eq!(swimming.output.vertical.unwrap().to_bits(), 0x3d0e_c2de);
}

#[test]
fn swimming_outside_water_scales_speed_across_turn_thresholds() {
    let mut control = MoveControlState::default();
    control.move_to(Vec3::new(0.0, 0.0, 10.0), 1.0);
    let mut facts = ground_facts(Vec3::ZERO, 60.0, 0.2);
    facts.navigation_done = Some(false);
    let config = SwimmingConfig {
        max_pitch: 30,
        max_yaw_turn: 10,
        in_water_speed_modifier: 0.5,
        outside_water_speed_modifier: 0.8,
        apply_gravity: true,
    };
    let plan = prepare_smooth_swimming_move(&control, stamp(17), facts, config).unwrap();
    assert_eq!(plan.output.yaw, Some(50.0));
    assert_eq!(plan.output.speed.unwrap().to_bits(), 0x3d03_126f);

    facts.yaw = 90.0;
    let stopped = prepare_smooth_swimming_move(
        &control,
        stamp(18),
        facts,
        SwimmingConfig {
            max_yaw_turn: 30,
            ..config
        },
    )
    .unwrap();
    assert_eq!(stopped.output.yaw, Some(60.0));
    assert_eq!(stopped.output.speed, Some(0.0));
}

#[test]
fn missing_caller_owned_facts_are_deferred() {
    let mut control = MoveControlState::default();
    control.strafe(1.0, 0.0);
    let mut facts = ground_facts(Vec3::ZERO, 0.0, 0.2);
    assert_eq!(
        prepare_move(&control, stamp(6), facts),
        Err(PrepareError::Deferred(MissingInput::NavigationPresence))
    );

    facts.strafe_navigation_present = Some(false);
    let no_navigation = prepare_move(&control, stamp(6), facts).unwrap();
    assert_eq!(no_navigation.output.forward, Some(1.0));
    assert_eq!(no_navigation.output.strafe, Some(0.0));

    facts.strafe_navigation_present = Some(true);
    assert_eq!(
        prepare_move(&control, stamp(6), facts),
        Err(PrepareError::Deferred(MissingInput::NodeEvaluatorPresence))
    );

    facts.strafe_evaluator_present = Some(false);
    let no_evaluator = prepare_move(&control, stamp(6), facts).unwrap();
    assert_eq!(no_evaluator.output.forward, Some(1.0));
    assert_eq!(no_evaluator.output.strafe, Some(0.0));

    facts.strafe_evaluator_present = Some(true);
    assert_eq!(
        prepare_move(&control, stamp(6), facts),
        Err(PrepareError::Deferred(MissingInput::Walkability))
    );

    let probe = strafe_walkability_probe(&control, facts).unwrap();
    facts.walkability = Some(WalkabilityFact::new(
        HorizontalDelta {
            x: probe.x + 1.0,
            z: probe.z,
        },
        true,
    ));
    assert_eq!(
        prepare_move(&control, stamp(6), facts),
        Err(PrepareError::StaleFact(MissingInput::Walkability))
    );

    control.move_to(Vec3::new(1.0, 0.0, 0.0), 1.0);
    facts.collision = None;
    assert_eq!(
        prepare_move(&control, stamp(6), facts),
        Err(PrepareError::Deferred(MissingInput::Collision))
    );
}

#[test]
fn swimming_gravity_requires_a_current_velocity_fact_with_matching_stamp() {
    let mut control = MoveControlState::default();
    control.move_to(Vec3::new(0.0, 1.0, 1.0), 1.0);
    let mut facts = ground_facts(Vec3::ZERO, 0.0, 0.2);
    facts.in_water = true;
    facts.navigation_done = Some(false);

    let config = SwimmingConfig {
        max_pitch: 30,
        max_yaw_turn: 10,
        in_water_speed_modifier: 0.5,
        outside_water_speed_modifier: 0.8,
        apply_gravity: true,
    };
    assert_eq!(
        prepare_smooth_swimming_move(&control, stamp(19), facts, config),
        Err(PrepareError::Deferred(
            MissingInput::CurrentVerticalVelocity
        ))
    );

    facts.current_vertical_velocity = Some(CurrentVerticalVelocityFact::new(stamp(18), 0.1));
    assert_eq!(
        prepare_smooth_swimming_move(&control, stamp(19), facts, config),
        Err(PrepareError::StaleFact(
            MissingInput::CurrentVerticalVelocity
        ))
    );

    facts.current_vertical_velocity = Some(CurrentVerticalVelocityFact::new(stamp(19), 0.1));
    let plan = prepare_smooth_swimming_move(&control, stamp(19), facts, config).unwrap();
    assert_eq!(
        plan.output.vertical_velocity_change,
        Some(VerticalVelocityChange::Additive {
            expected_current: 0.1,
            delta: 0.005,
            result: f64::from_bits(0x3fba_e147_ae14_7ae2),
        })
    );

    let mut applied = control;
    assert_eq!(
        apply_smooth_swimming_move(&mut applied, stamp(19), None, plan),
        Err(ApplyError::MissingFact(
            MissingInput::CurrentVerticalVelocity
        ))
    );
    assert_eq!(applied, control);
    assert_eq!(
        apply_smooth_swimming_move(&mut applied, stamp(19), Some(0.2), plan),
        Err(ApplyError::StaleVerticalVelocity {
            expected_bits: 0x3fb9_9999_9999_999a,
            actual_bits: 0x3fc9_9999_9999_999a,
        })
    );
    assert_eq!(applied, control);
    let output = apply_smooth_swimming_move(&mut applied, stamp(19), Some(0.1), plan).unwrap();
    assert_eq!(output, plan.output);
    assert_eq!(applied.revision, 1);
}

#[test]
fn look_observes_two_tick_cooldown_and_navigation_clamp() {
    let mut control = LookControlState::default();
    control.look_at(Vec3::new(10.0, 1.62, 0.0), 30.0, 20.0);
    let facts = LookFacts {
        position: Vec3::ZERO,
        eye_y: 1.62,
        pitch: 9.0,
        head_yaw: 90.0,
        body_yaw: 0.0,
        max_head_yaw: 45.0,
        navigation_done: Some(false),
        reset_pitch: true,
    };
    let plan = prepare_look(&control, stamp(7), facts).unwrap();
    assert_eq!(plan.next.cooldown, 1);
    assert_eq!(plan.output.pitch, Some(0.0));
    assert_eq!(plan.output.head_yaw, Some(45.0));
}

#[test]
fn smooth_swimming_look_uses_tilt_offsets_and_body_drift() {
    let mut control = LookControlState::default();
    control.look_at(Vec3::new(10.0, 1.62, 0.0), 30.0, 20.0);
    let active = LookFacts {
        position: Vec3::ZERO,
        eye_y: 1.62,
        pitch: 0.0,
        head_yaw: 0.0,
        body_yaw: 0.0,
        max_head_yaw: 45.0,
        navigation_done: None,
        reset_pitch: false,
    };
    let plan = prepare_smooth_swimming_look(
        &control,
        stamp(15),
        active,
        SwimmingLookConfig {
            max_yaw_from_center: 20,
        },
    )
    .unwrap();
    assert_eq!(plan.next.cooldown, 1);
    assert_eq!(plan.output.pitch, Some(10.0));
    assert_eq!(plan.output.head_yaw, Some(-30.0));
    assert_eq!(plan.output.body_yaw, Some(-4.0));

    let idle = LookControlState {
        max_yaw_speed: 10.0,
        ..Default::default()
    };
    let mut idle_facts = active;
    idle_facts.pitch = 12.0;
    idle_facts.head_yaw = 40.0;
    assert_eq!(
        prepare_smooth_swimming_look(
            &idle,
            stamp(16),
            idle_facts,
            SwimmingLookConfig {
                max_yaw_from_center: 20,
            },
        ),
        Err(PrepareError::Deferred(MissingInput::NavigationState))
    );
    idle_facts.navigation_done = Some(true);
    let idle_plan = prepare_smooth_swimming_look(
        &idle,
        stamp(16),
        idle_facts,
        SwimmingLookConfig {
            max_yaw_from_center: 20,
        },
    )
    .unwrap();
    assert_eq!(idle_plan.output.pitch, Some(7.0));
    assert_eq!(idle_plan.output.head_yaw, Some(30.0));
    assert_eq!(idle_plan.output.body_yaw, Some(4.0));
}

#[test]
fn jump_is_one_tick_and_apply_rejects_each_stale_precondition() {
    let mut state = JumpControlState::default();
    state.jump();
    let plan = prepare_jump(&state, stamp(8)).unwrap();
    assert_eq!(plan.output.jumping, Some(true));
    assert!(!plan.next.requested);

    assert_eq!(
        apply_jump(&mut state, stamp(9), plan),
        Err(ApplyError::StaleEntity {
            expected: 8,
            actual: 9
        })
    );
    assert_eq!(
        apply_jump(
            &mut state,
            InputStamp {
                entity_revision: 8,
                facts_revision: 9
            },
            plan
        ),
        Err(ApplyError::StaleFacts {
            expected: 8,
            actual: 9
        })
    );
    state.revision = 1;
    assert_eq!(
        apply_jump(&mut state, stamp(8), plan),
        Err(ApplyError::StaleControl {
            expected: 0,
            actual: 1
        })
    );
}

#[test]
fn apply_detects_same_revision_control_mutation() {
    let state = JumpControlState::default();
    let plan = prepare_jump(&state, stamp(14)).unwrap();
    let mut changed = state;
    changed.jump();
    assert_eq!(
        apply_jump(&mut changed, stamp(14), plan),
        Err(ApplyError::ControlChangedAtRevision { revision: 0 })
    );
}

#[test]
fn body_rotation_uses_movement_reset_and_delayed_convergence() {
    let state = BodyRotationState::default();
    let moving = BodyFacts {
        position: Vec3::new(1.0, 0.0, 0.0),
        previous_position: Vec3::ZERO,
        yaw: 80.0,
        head_yaw: 0.0,
        body_yaw: 0.0,
        max_head_yaw: 45.0,
        carrying_mob_passenger: false,
    };
    let plan = prepare_body_rotation(&state, stamp(10), moving).unwrap();
    assert_eq!(plan.output.body_yaw, Some(80.0));
    assert_eq!(plan.output.head_yaw, Some(35.0));
    assert_eq!(plan.next.head_stable_time, 0);

    let mut stable = BodyRotationState {
        last_stable_head_yaw: 30.0,
        ..Default::default()
    };
    let still = BodyFacts {
        position: Vec3::ZERO,
        previous_position: Vec3::ZERO,
        yaw: 0.0,
        head_yaw: 30.0,
        body_yaw: -30.0,
        max_head_yaw: 45.0,
        carrying_mob_passenger: false,
    };
    for revision in 0..11 {
        stable.revision = revision;
        stable = prepare_body_rotation(&stable, stamp(revision), still)
            .unwrap()
            .next;
    }
    assert_eq!(stable.head_stable_time, 11);
    let twelfth = prepare_body_rotation(&stable, stamp(11), still).unwrap();
    assert_eq!(twelfth.output.body_yaw, Some(-6.0));
}

#[test]
fn invalid_arithmetic_and_exhausted_revisions_fail_without_mutation() {
    let control = MoveControlState {
        revision: u64::MAX,
        operation: MoveOperation::Wait,
        ..Default::default()
    };
    assert_eq!(
        prepare_move(&control, stamp(12), ground_facts(Vec3::ZERO, 0.0, 0.2)),
        Err(PrepareError::RevisionExhausted)
    );

    let mut invalid = MoveControlState::default();
    invalid.move_to(Vec3::new(f64::NAN, 0.0, 0.0), 1.0);
    assert_eq!(
        prepare_move(&invalid, stamp(12), ground_facts(Vec3::ZERO, 0.0, 0.2)),
        Err(PrepareError::NonFinite(InputField::Target))
    );

    let invalid_body = BodyRotationState {
        last_stable_head_yaw: f32::NAN,
        ..Default::default()
    };
    let facts = BodyFacts {
        position: Vec3::ZERO,
        previous_position: Vec3::ZERO,
        yaw: 0.0,
        head_yaw: 0.0,
        body_yaw: 0.0,
        max_head_yaw: 45.0,
        carrying_mob_passenger: false,
    };
    assert_eq!(
        prepare_body_rotation(&invalid_body, stamp(12), facts),
        Err(PrepareError::NonFinite(InputField::Rotation))
    );
}

#[test]
fn carrying_a_mob_passenger_suppresses_stationary_body_updates() {
    let state = BodyRotationState {
        head_stable_time: 20,
        last_stable_head_yaw: 0.0,
        ..Default::default()
    };
    let facts = BodyFacts {
        position: Vec3::ZERO,
        previous_position: Vec3::ZERO,
        yaw: 90.0,
        head_yaw: 90.0,
        body_yaw: 0.0,
        max_head_yaw: 45.0,
        carrying_mob_passenger: true,
    };
    let plan = prepare_body_rotation(&state, stamp(13), facts).unwrap();
    assert_eq!(plan.next.head_stable_time, 20);
    assert_eq!(plan.output, ControlOutput::default());
}
