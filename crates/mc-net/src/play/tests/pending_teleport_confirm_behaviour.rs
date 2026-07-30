use crate::play::movement::{
    PendingTeleport, TeleportConfirmResult, confirm_pending_teleport,
    guard_pending_teleport_movement,
};

#[test]
fn pending_teleport_confirm_behaviour_after_unconfirmed_movement() {
    let mut pending = Some(PendingTeleport::new(7, 0));

    assert!(guard_pending_teleport_movement(
        &pending,
        "ServerboundMovePlayerPos"
    ));

    assert_eq!(
        confirm_pending_teleport(&mut pending, 8),
        TeleportConfirmResult::Mismatched { expected: 7 }
    );
    assert_eq!(pending.unwrap().id, 7);

    assert_eq!(
        confirm_pending_teleport(&mut pending, 7),
        TeleportConfirmResult::Confirmed
    );
    assert!(pending.is_none());
    assert!(!guard_pending_teleport_movement(
        &pending,
        "ServerboundMovePlayerPos"
    ));
}
