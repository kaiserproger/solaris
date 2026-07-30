use std::sync::Arc;
use std::task::Poll;

use mc_protocol::codec::Identifier;
use mc_world::{BlockStateId, Chunk, ChunkPos};

use super::{
    PlayerPose, interaction_state_for_blocks, open_crafting_table_container, simple_block,
};

#[tokio::test]
async fn crafting_table_open_does_not_wait_for_world_writer() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:crafting_table"),
        ])
        .unwrap(),
    );
    let mut state = interaction_state_for_blocks(blocks);
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = state.world.lock().await;
        let chunk = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage.set_block_at(position, BlockStateId(1)).unwrap();
    }

    let world = Arc::clone(&state.world);
    let world_writer = world.lock().await;
    let mut writer = Vec::new();
    let mut open = Box::pin(open_crafting_table_container(
        &mut state,
        &mut writer,
        PlayerPose::new(1.5, 64.0, 1.5),
        7,
        position.x,
        position.y,
        position.z,
    ));
    std::future::poll_fn(|cx| {
        assert!(
            matches!(
                std::future::Future::poll(open.as_mut(), cx),
                Poll::Ready(Ok(true))
            ),
            "opening a loaded crafting table must use the published world view"
        );
        Poll::Ready(())
    })
    .await;

    drop(world_writer);
}
