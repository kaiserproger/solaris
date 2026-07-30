use std::sync::Arc;

use super::{
    BlockStateId, Compression, PlayerPose, correct_player_collision, fluid_test_registry,
    insert_fluid_test_chunk, interaction_state_for_blocks,
};

#[tokio::test]
async fn collision_correction_does_not_teleport_back_into_existing_solid_overlap() {
    let state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    insert_fluid_test_chunk(&state).await;
    state
        .world
        .lock()
        .await
        .set_block_at(mc_world::BlockPos { x: 0, y: 64, z: 0 }, BlockStateId(1))
        .unwrap();
    let old_pose = PlayerPose::new(0.50, 64.0, 0.50);
    let new_pose = PlayerPose::new(0.55, 64.0, 0.50);
    let mut writer = Vec::new();
    let mut next_teleport_id = 2;
    let mut pending_teleport = None;

    let corrected = correct_player_collision(
        Some(&state),
        &mut writer,
        Compression::Disabled,
        old_pose,
        new_pose,
        0,
        &mut next_teleport_id,
        &mut pending_teleport,
    )
    .await
    .unwrap();

    assert!(
        !corrected,
        "an already-colliding authoritative pose must be allowed to escape"
    );
    assert!(writer.is_empty());
    assert!(pending_teleport.is_none());
    assert_eq!(next_teleport_id, 2);
}
