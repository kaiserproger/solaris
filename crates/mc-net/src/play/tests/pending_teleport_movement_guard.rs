use crate::play::movement::guard_pending_teleport_movement;

#[test]
fn pending_teleport_movement_guard_returns_false_without_pending_teleport() {
    let pending = None;

    assert!(!guard_pending_teleport_movement(
        &pending,
        "ServerboundMovePlayerPos"
    ));
}
