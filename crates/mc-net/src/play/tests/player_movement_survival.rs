use super::*;

#[test]
fn player_pose_metadata_reports_swimming_and_shared_flags() {
    let mut pose = PlayerPose::new(0.5, 62.0, 0.5);
    pose.in_water = true;
    pose.swimming = true;
    pose.sprinting = true;

    assert_eq!(pose.entity_pose(), EntityPose::Swimming);
    assert_eq!(pose.shared_flags() & 0x08, 0x08);
    assert_eq!(pose.shared_flags() & 0x10, 0x10);
}

#[test]
fn survival_movement_exhaustion_tracks_sprint_and_sprint_jump_distance() {
    let mut old = PlayerPose::new(0.5, 72.0, 0.5);
    old.flags = MovePlayerFlags::new(true, false);
    let mut standing_jump = PlayerPose::new(0.5, 73.0, 0.5);
    standing_jump.flags = MovePlayerFlags::new(false, false);
    standing_jump.input.jump = true;

    assert_eq!(
        movement_exhaustion(old, standing_jump),
        mc_entity::player_survival_26_1_2::JUMP_EXHAUSTION
    );

    let mut walking = PlayerPose::new(4.5, 72.0, 0.5);

    assert_eq!(movement_exhaustion(old, walking), 0.0);

    walking.sprinting = true;
    let sprint_exhaustion = movement_exhaustion(old, walking);
    assert!(sprint_exhaustion > 0.0);

    let mut sprint_jump = walking;
    sprint_jump.y = 73.0;
    sprint_jump.flags = MovePlayerFlags::new(false, false);
    sprint_jump.input.jump = true;

    assert!(movement_exhaustion(old, sprint_jump) > sprint_exhaustion);
}

#[test]
fn player_movement_clamps_extreme_coordinates_and_rejects_non_finite_values() {
    let finite = AcceptedAbsoluteMovement {
        x: 1.0,
        y: 64.0,
        z: -2.0,
        yaw_pitch: Some((90.0, 15.0)),
        flags: MovePlayerFlags::new(true, false),
    };
    assert_eq!(
        normalize_absolute_player_movement(finite)
            .expect("finite movement is accepted")
            .x,
        1.0
    );

    let clamped = normalize_absolute_player_movement(AcceptedAbsoluteMovement {
        x: f64::MAX,
        y: -f64::MAX,
        z: -f64::MAX,
        ..finite
    })
    .expect("finite extreme movement is clamped");
    assert_eq!(clamped.x, 30_000_000.0);
    assert_eq!(clamped.y, -20_000_000.0);
    assert_eq!(clamped.z, -30_000_000.0);

    for movement in [
        AcceptedAbsoluteMovement {
            x: f64::NAN,
            ..finite
        },
        AcceptedAbsoluteMovement {
            y: f64::INFINITY,
            ..finite
        },
        AcceptedAbsoluteMovement {
            z: f64::NEG_INFINITY,
            ..finite
        },
        AcceptedAbsoluteMovement {
            yaw_pitch: Some((f32::NAN, 0.0)),
            ..finite
        },
        AcceptedAbsoluteMovement {
            yaw_pitch: Some((0.0, f32::INFINITY)),
            ..finite
        },
    ] {
        assert!(matches!(
            normalize_absolute_player_movement(movement),
            Err(ConnectionError::InvalidPlayerMovement)
        ));
    }

    assert!(matches!(
        validate_player_rotation(f32::NEG_INFINITY, 0.0),
        Err(ConnectionError::InvalidPlayerMovement)
    ));
}

#[test]
fn survival_food_update_saturates_extreme_input() {
    let mut state = SurvivalState::FULL;

    state.add_food(i32::MAX, f32::MAX);

    assert_eq!(state.food, mc_entity::player_survival_26_1_2::MAX_FOOD);
    assert_eq!(
        state.saturation,
        mc_entity::player_survival_26_1_2::MAX_FOOD as f32
    );
}

#[test]
fn survival_exhaustion_handles_extreme_input_in_bounded_work() {
    let mut state = SurvivalState::FULL;

    assert!(state.add_exhaustion(f32::MAX));

    assert_eq!(state.food, 0);
    assert_eq!(state.saturation, 0.0);
    assert!(state.exhaustion.is_finite());
    assert!((0.0..mc_entity::player_survival_26_1_2::EXHAUSTION_STEP).contains(&state.exhaustion));

    assert!(!state.add_exhaustion(f32::INFINITY));
    assert!(state.exhaustion.is_finite());
}
