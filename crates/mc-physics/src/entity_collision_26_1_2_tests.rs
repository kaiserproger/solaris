use super::*;

#[test]
fn collision_geometry_scales_width_height_and_eye_height_with_java_float_math() {
    let base = EntityCollisionDimensions {
        width: 0.9,
        height: 1.4,
        eye_height: 1.33,
        fixed: false,
    };

    let default = scale_entity_collision_geometry(base, 1.0).unwrap();
    assert_eq!(default.aabb.half_width, f64::from(0.45_f32));
    assert_eq!(default.aabb.height, f64::from(1.4_f32));
    assert_eq!(default.eye_height, f64::from(1.33_f32));

    let doubled = scale_entity_collision_geometry(base, 2.0).unwrap();
    assert_eq!(doubled.aabb.half_width, f64::from(0.9_f32));
    assert_eq!(doubled.aabb.height, f64::from(2.8_f32));
    assert_eq!(doubled.eye_height, f64::from(2.66_f32));
}

#[test]
fn collision_geometry_accepts_exact_scale_bounds() {
    let base = EntityCollisionDimensions {
        width: 0.9,
        height: 1.4,
        eye_height: 1.33,
        fixed: false,
    };

    let minimum = scale_entity_collision_geometry(base, 0.0625).unwrap();
    assert_eq!(minimum.aabb.height, f64::from(1.4_f32 * 0.0625_f32));
    assert_eq!(minimum.eye_height, f64::from(1.33_f32 * 0.0625_f32));

    let maximum = scale_entity_collision_geometry(base, 16.0).unwrap();
    assert_eq!(maximum.aabb.half_width, f64::from(0.9_f32 * 16.0_f32 / 2.0));
    assert_eq!(maximum.aabb.height, f64::from(1.4_f32 * 16.0_f32));
    assert_eq!(maximum.eye_height, f64::from(1.33_f32 * 16.0_f32));
}

#[test]
fn fixed_and_zero_width_dimensions_follow_entity_dimensions_scale() {
    let fixed = EntityCollisionDimensions {
        width: 0.9,
        height: 1.4,
        eye_height: 1.33,
        fixed: true,
    };
    let unchanged = scale_entity_collision_geometry(fixed, 2.0).unwrap();
    assert_eq!(unchanged.aabb.half_width, f64::from(0.45_f32));
    assert_eq!(unchanged.aabb.height, f64::from(1.4_f32));
    assert_eq!(unchanged.eye_height, f64::from(1.33_f32));

    let zero_width = EntityCollisionDimensions {
        width: 0.0,
        fixed: false,
        ..fixed
    };
    let scaled = scale_entity_collision_geometry(zero_width, 2.0).unwrap();
    assert_eq!(scaled.aabb.half_width, 0.0);
    assert_eq!(scaled.aabb.height, f64::from(2.8_f32));
    assert_eq!(scaled.eye_height, f64::from(2.66_f32));
}

#[test]
fn collision_geometry_rejects_invalid_dimensions_and_scale() {
    let valid = EntityCollisionDimensions {
        width: 0.9,
        height: 1.4,
        eye_height: 1.33,
        fixed: false,
    };
    for scale in [
        0.0,
        0.0625_f32.next_down(),
        16.0_f32.next_up(),
        -1.0,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ] {
        assert_eq!(
            scale_entity_collision_geometry(valid, scale),
            Err(EntityContactError::InvalidScale)
        );
    }

    for dimensions in [
        EntityCollisionDimensions {
            width: f32::NAN,
            ..valid
        },
        EntityCollisionDimensions {
            height: f32::NAN,
            ..valid
        },
        EntityCollisionDimensions {
            eye_height: f32::INFINITY,
            ..valid
        },
    ] {
        assert_eq!(
            scale_entity_collision_geometry(dimensions, 1.0),
            Err(EntityContactError::InvalidDimensions)
        );
    }
}

fn push_input(
    caller_to_other_x: f64,
    caller_to_other_z: f64,
    recipient: PushRecipient,
) -> EntityPushInput {
    EntityPushInput {
        caller_to_other_x,
        caller_to_other_z,
        recipient,
        caller_physics_enabled: true,
        other_physics_enabled: true,
        passenger_of_same_vehicle: false,
        recipient_pushable: true,
        recipient_is_vehicle: false,
    }
}

fn contact(is_passenger: bool) -> CrammingContact {
    CrammingContact { is_passenger }
}

fn pair_input(caller_to_other_x: f64, caller_to_other_z: f64) -> EntityPushPairInput {
    EntityPushPairInput {
        caller_to_other_x,
        caller_to_other_z,
        caller_physics_enabled: true,
        other_physics_enabled: true,
        passenger_of_same_vehicle: false,
        caller_pushable: true,
        caller_is_vehicle: false,
        other_pushable: true,
        other_is_vehicle: false,
    }
}

#[test]
fn push_constants_are_java_f32_values_widened_to_f64() {
    assert_eq!(MIN_PUSH_DISTANCE.to_bits(), f64::from(0.01_f32).to_bits());
    assert_eq!(PUSH_STRENGTH.to_bits(), f64::from(0.05_f32).to_bits());
    assert_ne!(MIN_PUSH_DISTANCE.to_bits(), 0.01_f64.to_bits());
    assert_ne!(PUSH_STRENGTH.to_bits(), 0.05_f64.to_bits());
}

#[test]
fn exact_widened_threshold_is_inclusive() {
    let just_below = f64::from_bits(MIN_PUSH_DISTANCE.to_bits() - 1);
    assert_eq!(
        vanilla_push_impulse(push_input(just_below, 0.0, PushRecipient::Caller)),
        Ok(Vec3::ZERO)
    );

    let impulse =
        vanilla_push_impulse(push_input(MIN_PUSH_DISTANCE, 0.0, PushRecipient::Caller)).unwrap();
    assert!(impulse.x < 0.0);
    assert_eq!(impulse.y, 0.0);
    assert_eq!(impulse.z, 0.0);
}

#[test]
fn zero_and_near_overlap_emit_no_vanilla_impulse() {
    assert_eq!(
        vanilla_push_impulse(push_input(0.0, 0.0, PushRecipient::Caller)),
        Ok(Vec3::ZERO)
    );
    assert_eq!(
        vanilla_push_impulse(push_input(
            MIN_PUSH_DISTANCE / 2.0,
            -MIN_PUSH_DISTANCE / 2.0,
            PushRecipient::Other,
        )),
        Ok(Vec3::ZERO)
    );
}

#[test]
fn caller_selects_one_directional_recipient_impulse() {
    let caller = vanilla_push_impulse(push_input(0.25, -0.09, PushRecipient::Caller)).unwrap();
    let other = vanilla_push_impulse(push_input(0.25, -0.09, PushRecipient::Other)).unwrap();

    assert_eq!(caller, Vec3::new(-other.x, 0.0, -other.z));
    assert!(caller.x < 0.0);
    assert!(caller.z > 0.0);
}

#[test]
fn paired_push_applies_each_recipient_eligibility_independently() {
    let mut input = pair_input(0.25, -0.09);
    input.caller_pushable = false;

    let impulses = vanilla_push_impulses(input).unwrap();

    assert_eq!(impulses.caller, Vec3::ZERO);
    assert!(impulses.other.x > 0.0);
    assert!(impulses.other.z < 0.0);
}

#[test]
fn paired_push_suppresses_both_recipients_for_shared_vehicle_or_disabled_physics() {
    let mut shared_vehicle = pair_input(0.25, 0.0);
    shared_vehicle.passenger_of_same_vehicle = true;
    assert_eq!(
        vanilla_push_impulses(shared_vehicle),
        Ok(EntityPushImpulses::ZERO)
    );

    let mut caller_disabled = pair_input(0.25, 0.0);
    caller_disabled.caller_physics_enabled = false;
    assert_eq!(
        vanilla_push_impulses(caller_disabled),
        Ok(EntityPushImpulses::ZERO)
    );

    let mut other_disabled = pair_input(0.25, 0.0);
    other_disabled.other_physics_enabled = false;
    assert_eq!(
        vanilla_push_impulses(other_disabled),
        Ok(EntityPushImpulses::ZERO)
    );
}

#[test]
fn pushable_by_applies_both_team_collision_rules_exactly() {
    use TeamCollisionRule::{Always, Never, PushOtherTeams, PushOwnTeam};
    use TeamRelationship::{Allied, NotAllied};

    assert!(!vanilla_pushable_by(Never, Always, NotAllied, true, false));
    assert!(!vanilla_pushable_by(Always, Never, NotAllied, true, false));
    assert!(!vanilla_pushable_by(
        PushOwnTeam,
        Always,
        Allied,
        true,
        false
    ));
    assert!(!vanilla_pushable_by(
        Always,
        PushOwnTeam,
        Allied,
        true,
        false
    ));
    assert!(!vanilla_pushable_by(
        PushOtherTeams,
        Always,
        NotAllied,
        true,
        false
    ));
    assert!(!vanilla_pushable_by(
        Always,
        PushOtherTeams,
        NotAllied,
        true,
        false
    ));
    assert!(vanilla_pushable_by(
        PushOtherTeams,
        PushOtherTeams,
        Allied,
        true,
        false
    ));
    assert!(vanilla_pushable_by(
        PushOwnTeam,
        PushOwnTeam,
        NotAllied,
        true,
        false
    ));
}

#[test]
fn pushable_by_rejects_spectators_and_non_pushable_contacts_before_team_rules() {
    assert!(!vanilla_pushable_by(
        TeamCollisionRule::Always,
        TeamCollisionRule::Always,
        TeamRelationship::NotAllied,
        false,
        false,
    ));
    assert!(!vanilla_pushable_by(
        TeamCollisionRule::Always,
        TeamCollisionRule::Always,
        TeamRelationship::NotAllied,
        true,
        true,
    ));
}

#[test]
fn recipient_eligibility_is_asymmetric() {
    let mut caller = push_input(0.25, 0.0, PushRecipient::Caller);
    caller.recipient_pushable = false;
    assert_eq!(vanilla_push_impulse(caller), Ok(Vec3::ZERO));

    let other = push_input(0.25, 0.0, PushRecipient::Other);
    assert!(vanilla_push_impulse(other).unwrap().x > 0.0);
}

#[test]
fn same_vehicle_and_vehicle_recipient_suppress_push() {
    let mut same_vehicle = push_input(0.25, 0.0, PushRecipient::Caller);
    same_vehicle.passenger_of_same_vehicle = true;
    assert_eq!(vanilla_push_impulse(same_vehicle), Ok(Vec3::ZERO));

    let mut vehicle = push_input(0.25, 0.0, PushRecipient::Other);
    vehicle.recipient_is_vehicle = true;
    assert_eq!(vanilla_push_impulse(vehicle), Ok(Vec3::ZERO));
}

#[test]
fn disabled_physics_suppresses_push_for_either_participant() {
    let mut caller_disabled = push_input(0.25, 0.0, PushRecipient::Caller);
    caller_disabled.caller_physics_enabled = false;
    assert_eq!(vanilla_push_impulse(caller_disabled), Ok(Vec3::ZERO));

    let mut other_disabled = push_input(0.25, 0.0, PushRecipient::Other);
    other_disabled.other_physics_enabled = false;
    assert_eq!(vanilla_push_impulse(other_disabled), Ok(Vec3::ZERO));
}

#[test]
fn nonfinite_push_deltas_are_rejected() {
    assert_eq!(
        vanilla_push_impulse(push_input(f64::NAN, 0.0, PushRecipient::Caller)),
        Err(EntityContactError::NonFinitePushDelta)
    );
    assert_eq!(
        vanilla_push_impulse(push_input(0.0, f64::NEG_INFINITY, PushRecipient::Other,)),
        Err(EntityContactError::NonFinitePushDelta)
    );
}

#[test]
fn cramming_gate_requests_no_roll_when_cap_is_zero_or_below_threshold() {
    let contacts = [contact(false), contact(false), contact(false)];

    let disabled = vanilla_cramming_gate(&contacts, 0);
    assert_eq!(disabled.pushable_contacts, 3);
    assert_eq!(disabled.non_passenger_contacts, 3);
    assert!(!disabled.roll_required);

    let below = vanilla_cramming_gate(&contacts, 4);
    assert_eq!(below.pushable_contacts, 3);
    assert_eq!(below.non_passenger_contacts, 3);
    assert!(!below.roll_required);
}

#[test]
fn cramming_gate_requests_a_roll_at_and_above_the_cap() {
    let contacts = [
        contact(false),
        contact(false),
        contact(false),
        contact(false),
    ];

    assert!(vanilla_cramming_gate(&contacts[..3], 3).roll_required);
    assert!(vanilla_cramming_gate(&contacts, 3).roll_required);
}

#[test]
fn passengers_trigger_the_coarse_roll_gate_but_not_damage_eligibility() {
    let contacts = [contact(false), contact(true), contact(false)];
    let gate = vanilla_cramming_gate(&contacts, 3);

    assert_eq!(gate.pushable_contacts, 3);
    assert_eq!(gate.non_passenger_contacts, 2);
    assert!(gate.roll_required);
    assert!(!evaluate_cramming_roll(gate, 0).unwrap());
}

#[test]
fn second_stage_accepts_exact_rolls_zero_through_three_after_gate() {
    let contacts = [contact(false), contact(false)];
    let gate = vanilla_cramming_gate(&contacts, 2);
    assert!(gate.roll_required);

    for roll in 0..CRAMMING_ROLL_DENOMINATOR {
        assert_eq!(evaluate_cramming_roll(gate, roll).unwrap(), roll == 0);
    }
}

#[test]
fn invalid_cramming_roll_is_rejected_only_by_the_second_stage() {
    let gate = vanilla_cramming_gate(&[contact(false)], 1);
    assert!(gate.roll_required);
    assert_eq!(
        evaluate_cramming_roll(gate, CRAMMING_ROLL_DENOMINATOR),
        Err(EntityContactError::InvalidCrammingRoll {
            roll: CRAMMING_ROLL_DENOMINATOR,
        })
    );
}

#[test]
fn cramming_roll_request_is_explicit_and_applies_exact_damage() {
    let contacts = [contact(false), contact(false), contact(true)];
    let request = vanilla_cramming_roll_request(&contacts, 3).expect("roll request at cap");

    assert_eq!(apply_cramming_roll(request, 1).unwrap(), None);
    assert_eq!(apply_cramming_roll(request, 0).unwrap(), None);

    let damaging = vanilla_cramming_roll_request(&contacts[..2], 2).expect("damage-cap roll");
    assert_eq!(
        apply_cramming_roll(damaging, 0).unwrap(),
        Some(CRAMMING_DAMAGE)
    );
}
