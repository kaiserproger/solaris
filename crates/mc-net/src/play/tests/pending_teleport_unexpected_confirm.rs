use crate::play::movement::{TeleportConfirmResult, confirm_pending_teleport};

#[test]
fn pending_teleport_reports_unexpected_confirm_without_pending_state() {
    let mut pending = None;

    assert_eq!(
        confirm_pending_teleport(&mut pending, 1),
        TeleportConfirmResult::Unexpected
    );
    assert!(pending.is_none());
}
