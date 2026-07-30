use mc_protocol::frame::Compression;

use super::{PlayerPose, decode_player_position_sync_packets, resend_pending_teleport_if_due};
use crate::play::movement::PendingTeleport;

#[tokio::test]
async fn pending_teleport_resends_after_vanilla_tick_window() {
    let pose = PlayerPose::new(12.5, 70.0, -3.25);
    let mut writer = Vec::new();
    let mut next_teleport_id = 8;
    let mut pending = Some(PendingTeleport::new(7, 100));

    assert!(
        !resend_pending_teleport_if_due(
            &mut writer,
            Compression::Disabled,
            &mut pending,
            &mut next_teleport_id,
            pose,
            120,
        )
        .await
        .unwrap()
    );
    assert!(writer.is_empty());

    assert!(
        resend_pending_teleport_if_due(
            &mut writer,
            Compression::Disabled,
            &mut pending,
            &mut next_teleport_id,
            pose,
            121,
        )
        .await
        .unwrap()
    );
    let packets = decode_player_position_sync_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].teleport_id, 8);
    assert_eq!(packets[0].x, pose.x);
    assert_eq!(packets[0].y, pose.y);
    assert_eq!(packets[0].z, pose.z);
    assert!(matches!(
        pending,
        Some(PendingTeleport {
            id: 8,
            sent_tick: 121
        })
    ));
    assert_eq!(next_teleport_id, 9);
}
