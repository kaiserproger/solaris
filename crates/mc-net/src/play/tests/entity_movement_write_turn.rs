use crate::play::{
    ENTITY_MOVEMENTS_PER_WRITE_TURN, ServerEntityMove, take_entity_movement_write_turn,
};
use mc_entity::{EntityId, Rotation, Vec3};

#[test]
fn entity_movement_write_turn_preserves_order_across_the_budget_boundary() {
    let movements = (0..=ENTITY_MOVEMENTS_PER_WRITE_TURN)
        .map(|index| ServerEntityMove {
            id: EntityId(index as i32),
            position: Vec3::new(index as f64, 64.0, 0.0),
            wire_move: Some(crate::play::wire_entities::ServerEntityWireMove::Absolute {
                position: Vec3::new(index as f64, 64.0, 0.0),
            }),
            velocity: Vec3::ZERO,
            rotation: Rotation::ZERO,
            on_ground: true,
            send_velocity: false,
            send_head_rotation: false,
        })
        .collect();

    let (current, remaining) = take_entity_movement_write_turn(movements);

    assert_eq!(current.len(), ENTITY_MOVEMENTS_PER_WRITE_TURN);
    assert_eq!(
        current.first().map(|movement| movement.id),
        Some(EntityId(0))
    );
    assert_eq!(
        current.last().map(|movement| movement.id),
        Some(EntityId(ENTITY_MOVEMENTS_PER_WRITE_TURN as i32 - 1))
    );
    let remaining = remaining.expect("one movement remains after the write-turn budget");
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining[0].id,
        EntityId(ENTITY_MOVEMENTS_PER_WRITE_TURN as i32)
    );
}
