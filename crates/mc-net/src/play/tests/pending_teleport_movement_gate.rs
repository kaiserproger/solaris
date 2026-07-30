use crate::play::movement::{PendingTeleport, guard_pending_teleport_movement};

#[test]
fn pending_teleport_movement_gate_waits_without_duplicate_sync_packets() {
    let pending = Some(PendingTeleport::new(12, 0));

    for _ in 0..4 {
        assert!(guard_pending_teleport_movement(
            &pending,
            "ServerboundMovePlayerPos"
        ));
    }

    assert!(matches!(pending, Some(PendingTeleport { id: 12, .. })));
}
