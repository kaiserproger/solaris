use super::{
    BlockEdit, BlockStateId, Direction, PlayerPose, fluid_test_registry, insert_fluid_test_chunk,
    interaction_state_for_blocks, plan_place_block_edits,
};
use std::sync::Arc;
use std::task::Poll;

#[tokio::test]
async fn block_placement_planning_does_not_wait_for_world_writer() {
    let state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    insert_fluid_test_chunk(&state).await;
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };

    let world = Arc::clone(&state.world);
    let world_writer = world.lock().await;
    let mut planning = Box::pin(plan_place_block_edits(
        &state,
        position,
        BlockStateId(1),
        PlayerPose::new(1.5, 64.0, 1.5),
        Direction::Up,
        0.5,
    ));
    std::future::poll_fn(
        |cx| match std::future::Future::poll(planning.as_mut(), cx) {
            Poll::Ready(Some(plan)) => {
                assert_eq!(
                    plan.edits,
                    vec![BlockEdit {
                        pos: position,
                        new_state: BlockStateId(1),
                    }]
                );
                Poll::Ready(())
            }
            Poll::Ready(None) => panic!("valid loaded stone placement was rejected"),
            Poll::Pending => panic!("placement planning waited for the world writer"),
        },
    )
    .await;

    drop(world_writer);
}
