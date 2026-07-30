use std::sync::Arc;

use mc_world::BlockStateId;

use super::{button_test_registry, insert_fluid_test_chunk, interaction_state_for_blocks};
use crate::play::{BlockEdit, plan_loaded_toggle_block_interaction};

#[tokio::test]
async fn toggle_planning_does_not_wait_for_world_writer() {
    let state = interaction_state_for_blocks(Arc::new(button_test_registry()));
    insert_fluid_test_chunk(&state).await;
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let expected_token = {
        let mut storage = state.world.lock().await;
        storage
            .set_block_at(position, BlockStateId(1))
            .expect("place unpowered button");
        storage
            .block_mutation_token(position)
            .expect("button mutation token")
    };

    let world = Arc::clone(&state.world);
    let world_writer = world.lock().await;
    let plan = plan_loaded_toggle_block_interaction(&state, position, 100)
        .expect("published button should be interactive");

    assert_eq!(
        plan.edits,
        vec![BlockEdit {
            pos: position,
            new_state: BlockStateId(2),
        }]
    );
    assert_eq!(plan.preconditions.len(), 1);
    assert_eq!(plan.preconditions[0].expected_token, expected_token);
    assert_eq!(plan.scheduled_block_ticks[0].trigger_tick, 120);
    drop(world_writer);
}
