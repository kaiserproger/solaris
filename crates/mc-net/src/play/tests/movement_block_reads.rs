use super::{
    PlayerPose, fluid_test_facts, fluid_test_registry, insert_fluid_test_chunk,
    interaction_state_for_blocks, player_pose_collides_with_solid, player_water_overlap,
};
use mc_world::BlockStateId;
use std::sync::Arc;
use std::task::Poll;

#[tokio::test]
async fn movement_block_reads_do_not_wait_for_world_writer() {
    let mut state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    state.block_facts = Arc::new(fluid_test_facts());
    insert_fluid_test_chunk(&state).await;
    {
        let mut storage = state.world.lock().await;
        storage
            .set_block_at(mc_world::BlockPos { x: 0, y: 64, z: 0 }, BlockStateId(1))
            .unwrap();
        storage
            .set_block_at(mc_world::BlockPos { x: 2, y: 64, z: 0 }, BlockStateId(2))
            .unwrap();
    }

    let world = Arc::clone(&state.world);
    let world_writer = world.lock().await;

    let mut collision = Box::pin(player_pose_collides_with_solid(
        Some(&state),
        PlayerPose::new(0.5, 64.0, 0.5),
    ));
    std::future::poll_fn(|cx| {
        assert!(
            matches!(
                std::future::Future::poll(collision.as_mut(), cx),
                Poll::Ready(true)
            ),
            "collision over a loaded chunk must not wait for the world writer"
        );
        Poll::Ready(())
    })
    .await;

    let mut water = Box::pin(player_water_overlap(
        &state,
        PlayerPose::new(2.5, 64.0, 0.5),
    ));
    std::future::poll_fn(|cx| {
        assert!(
            matches!(
                std::future::Future::poll(water.as_mut(), cx),
                Poll::Ready((true, false))
            ),
            "water overlap over a loaded chunk must not wait for the world writer"
        );
        Poll::Ready(())
    })
    .await;

    drop(world_writer);
}
