use mc_entity::{EntityId, EntityMotionState, Rotation, Vec3};

use super::outbound::ServerEntitySnapshot;
use super::visibility::{
    EntityTrackerUpdate, LastSentEntityState, advance_entity_tracker_update,
    quantized_entity_delta, update_server_entity_motion,
};
use crate::play::wire_entities::ServerEntityWireMove;

fn last_sent(position: Vec3) -> LastSentEntityState {
    LastSentEntityState {
        position,
        velocity: Vec3::ZERO,
        rotation: Rotation::ZERO,
        on_ground: true,
        tracking_update_count: 1,
        teleport_delay: 0,
    }
}

fn advance(
    last_sent: &mut LastSentEntityState,
    position: Vec3,
    rotation: Rotation,
    on_ground: bool,
) -> EntityTrackerUpdate {
    advance_entity_tracker_update(last_sent, position, Vec3::ZERO, rotation, on_ground, false)
}

#[test]
fn entity_relative_delta_uses_quantized_absolute_endpoints() {
    let previous = Vec3::new(0.1, 64.0, -0.1);
    let current = Vec3::new(0.2, 64.0, -0.2);

    let delta = quantized_entity_delta(current, previous);

    assert_eq!(delta.x, 409.0 / 4096.0);
    assert_eq!(delta.y, 0.0);
    assert_eq!(delta.z, -409.0 / 4096.0);
}

#[test]
fn entity_relative_delta_matches_java_round_at_negative_half_step() {
    let previous = Vec3::new(-0.5 / 4096.0, 0.0, 0.0);
    let current = Vec3::ZERO;

    let delta = quantized_entity_delta(current, previous);

    assert_eq!(delta.x, 0.0);
}

#[test]
fn position_dirty_uses_unquantized_inclusive_threshold() {
    let previous = Vec3::new(0.000_12, 0.0, 0.0);
    let current = Vec3::new(0.000_13, 0.0, 0.0);
    assert_ne!(quantized_entity_delta(current, previous), Vec3::ZERO);
    let mut below = last_sent(previous);

    let below_update = advance(&mut below, current, Rotation::ZERO, true);

    assert_eq!(below_update.wire_move, None);

    let boundary_axis = 1.0 / 512.0;
    let mut boundary = last_sent(Vec3::ZERO);
    let boundary_position = Vec3::new(boundary_axis, boundary_axis, 0.0);

    let boundary_update = advance(&mut boundary, boundary_position, Rotation::ZERO, true);

    assert_eq!(
        boundary_update.wire_move,
        Some(ServerEntityWireMove::Position {
            delta: boundary_position,
        })
    );
}

#[test]
fn zero_count_forces_the_oracle_position_refresh() {
    let mut last = last_sent(Vec3::new(1.0, 64.0, 1.0));
    last.tracking_update_count = 0;
    let position = last.position;

    let update = advance(&mut last, position, Rotation::ZERO, true);

    assert_eq!(
        update.wire_move,
        Some(ServerEntityWireMove::Position { delta: Vec3::ZERO })
    );
    assert_eq!(last.tracking_update_count, 1);
    assert_eq!(last.teleport_delay, 1);
}

#[test]
fn movement_before_modulo_sixty_does_not_shift_global_refresh() {
    let mut last = last_sent(Vec3::ZERO);
    last.tracking_update_count = 58;

    let moved = advance(&mut last, Vec3::new(0.25, 0.0, 0.0), Rotation::ZERO, true);
    assert_eq!(
        moved.wire_move,
        Some(ServerEntityWireMove::Position {
            delta: Vec3::new(0.25, 0.0, 0.0),
        })
    );
    assert_eq!(last.tracking_update_count, 59);

    let position = last.position;
    let before_refresh = advance(&mut last, position, Rotation::ZERO, true);
    assert_eq!(before_refresh.wire_move, None);
    assert_eq!(last.tracking_update_count, 60);

    let position = last.position;
    let refresh = advance(&mut last, position, Rotation::ZERO, true);
    assert_eq!(
        refresh.wire_move,
        Some(ServerEntityWireMove::Position { delta: Vec3::ZERO })
    );
    assert_eq!(last.tracking_update_count, 61);
}

#[test]
fn teleport_delay_uses_strictly_greater_than_four_hundred() {
    let mut last = last_sent(Vec3::new(1.0, 64.0, 1.0));
    last.teleport_delay = 399;
    let position = last.position;

    let at_four_hundred = advance(&mut last, position, Rotation::ZERO, true);

    assert_eq!(at_four_hundred.wire_move, None);
    assert_eq!(last.teleport_delay, 400);

    let position = last.position;
    let at_four_hundred_one = advance(&mut last, position, Rotation::ZERO, true);

    assert_eq!(
        at_four_hundred_one.wire_move,
        Some(ServerEntityWireMove::Absolute { position })
    );
    assert_eq!(last.teleport_delay, 0);
}

#[test]
fn signed_short_boundaries_are_relative_and_overflow_is_absolute() {
    let minimum = Vec3::new(f64::from(i16::MIN) / 4096.0, 0.0, 0.0);
    let maximum = Vec3::new(f64::from(i16::MAX) / 4096.0, 0.0, 0.0);

    for position in [minimum, maximum] {
        let mut last = last_sent(Vec3::ZERO);
        let update = advance(&mut last, position, Rotation::ZERO, true);
        assert_eq!(
            update.wire_move,
            Some(ServerEntityWireMove::Position { delta: position })
        );
    }

    let overflow = Vec3::new((f64::from(i16::MAX) + 1.0) / 4096.0, 0.0, 0.0);
    let mut last = last_sent(Vec3::ZERO);
    let update = advance(&mut last, overflow, Rotation::ZERO, true);
    assert_eq!(
        update.wire_move,
        Some(ServerEntityWireMove::Absolute { position: overflow })
    );
}

#[test]
fn on_ground_transition_is_absolute_and_resets_only_teleport_delay() {
    let mut last = last_sent(Vec3::ZERO);
    last.teleport_delay = 17;

    let update = advance(&mut last, Vec3::ZERO, Rotation::ZERO, false);

    assert_eq!(
        update.wire_move,
        Some(ServerEntityWireMove::Absolute {
            position: Vec3::ZERO,
        })
    );
    assert_eq!(last.teleport_delay, 0);
    assert_eq!(last.tracking_update_count, 2);
    assert!(!last.on_ground);
}

#[test]
fn tracker_selects_each_relative_wire_shape_explicitly() {
    let position = Vec3::new(0.25, 0.0, 0.0);
    let body_rotation = Rotation {
        yaw: 90.0,
        pitch: -15.0,
        head_yaw: 0.0,
    };

    let mut position_only = last_sent(Vec3::ZERO);
    assert_eq!(
        advance(&mut position_only, position, Rotation::ZERO, true).wire_move,
        Some(ServerEntityWireMove::Position { delta: position })
    );

    let mut rotation_only = last_sent(Vec3::ZERO);
    assert_eq!(
        advance(&mut rotation_only, Vec3::ZERO, body_rotation, true).wire_move,
        Some(ServerEntityWireMove::Rotation)
    );

    let mut both = last_sent(Vec3::ZERO);
    assert_eq!(
        advance(&mut both, position, body_rotation, true).wire_move,
        Some(ServerEntityWireMove::PositionRotation { delta: position })
    );
}

#[test]
fn packed_head_yaw_change_is_independent_and_not_redundant() {
    let mut last = last_sent(Vec3::ZERO);
    let head_rotation = Rotation {
        head_yaw: 90.0,
        ..Rotation::ZERO
    };

    let changed = advance(&mut last, Vec3::ZERO, head_rotation, true);

    assert_eq!(changed.wire_move, None);
    assert!(changed.send_head_rotation);

    let unchanged = advance(&mut last, Vec3::ZERO, head_rotation, true);

    assert_eq!(unchanged.wire_move, None);
    assert!(!unchanged.send_head_rotation);
}

#[test]
fn velocity_and_absolute_can_be_selected_together() {
    let mut last = last_sent(Vec3::ZERO);
    last.teleport_delay = 400;
    let velocity = Vec3::new(1.0, 0.0, -2.0);

    let update =
        advance_entity_tracker_update(&mut last, Vec3::ZERO, velocity, Rotation::ZERO, true, true);

    assert_eq!(
        update.wire_move,
        Some(ServerEntityWireMove::Absolute {
            position: Vec3::ZERO,
        })
    );
    assert!(update.send_velocity);
    assert_eq!(last.velocity, velocity);
}

#[test]
fn physics_motion_publication_preserves_non_kinematic_state() {
    let mut snapshot = ServerEntitySnapshot {
        id: EntityId(41),
        uuid: uuid::Uuid::nil(),
        type_id: 123,
        type_name: "minecraft:zombie".to_owned(),
        position: Vec3::new(0.5, 64.0, 0.5),
        rotation: Rotation::ZERO,
        velocity: Vec3::ZERO,
        on_ground: true,
        health: Some(7.0),
        item_stack: None,
        experience_value: Some(3),
        block_state: Some(9),
        animal: None,
        villager: None,
        villager_baby: false,
        main_hand_item: None,
        crossbow_charging: false,
        blaze_charged: false,
        guardian_attack_target_entity_id: 0,
    };
    let motion = EntityMotionState {
        id: snapshot.id,
        position: Vec3::new(1.5, 65.0, 2.5),
        rotation: Rotation {
            yaw: 90.0,
            pitch: 10.0,
            head_yaw: 75.0,
        },
        velocity: Vec3::new(0.1, 0.2, 0.3),
        on_ground: false,
        fall_distance: 0.0,
        goal_fence: mc_entity::EntityGoalFence::Idle,
        is_item: false,
        is_experience: false,
        is_arrow: false,
        arrow_revision: None,
        arrow_embedded_block: None,
        is_hurting_projectile: false,
        hurting_projectile_revision: None,
        is_throwable_projectile: false,
        throwable_projectile_revision: None,
        sends_velocity: true,
    };

    update_server_entity_motion(&mut snapshot, motion);

    assert_eq!(snapshot.position, motion.position);
    assert_eq!(snapshot.rotation, motion.rotation);
    assert_eq!(snapshot.velocity, motion.velocity);
    assert_eq!(snapshot.on_ground, motion.on_ground);
    assert_eq!(snapshot.health, Some(7.0));
    assert_eq!(snapshot.experience_value, Some(3));
    assert_eq!(snapshot.block_state, Some(9));
    assert_eq!(snapshot.type_name, "minecraft:zombie");
}
