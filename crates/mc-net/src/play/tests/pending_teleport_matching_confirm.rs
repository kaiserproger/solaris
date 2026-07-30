use crate::play::movement::{PendingTeleport, TeleportConfirmResult, confirm_pending_teleport};

#[test]
fn pending_teleport_confirm_clears_only_matching_id() {
    let mut pending = Some(PendingTeleport::new(7, 0));

    assert_eq!(
        confirm_pending_teleport(&mut pending, 8),
        TeleportConfirmResult::Mismatched { expected: 7 }
    );
    assert!(pending.is_some());

    assert_eq!(
        confirm_pending_teleport(&mut pending, 7),
        TeleportConfirmResult::Confirmed
    );
    assert!(pending.is_none());
}
