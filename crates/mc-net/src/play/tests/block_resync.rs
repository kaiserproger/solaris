use super::{
    BlockChangedAck, BlockEdit, BlockStateId, BlockUpdate, Chunk, ChunkPos, Compression,
    Identifier, InteractionHand, ItemRegistry, UseItemOnNoOpReason, UseItemOnResyncOptions,
    interaction_state_for_items, pack_block_pos, reject_use_item_on_with_resync,
    send_loaded_block_edit_resyncs,
};
use mc_protocol::Packet;
use std::sync::Arc;

#[tokio::test]
async fn rejected_visible_block_edit_resyncs_authoritative_cached_state() {
    let state = interaction_state_for_items(Arc::new(ItemRegistry::from_report(&[])));
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = state.world.lock().await;
        let cpos = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage.set_block_at(pos, BlockStateId(1)).unwrap();
    }
    let mut writer = Vec::new();
    let world = Arc::clone(&state.world);
    let world_writer = world.lock().await;
    let edits = [BlockEdit {
        pos,
        new_state: BlockStateId(0),
    }];

    let mut resync = Box::pin(send_loaded_block_edit_resyncs(&state, &mut writer, &edits));
    std::future::poll_fn(|cx| match std::future::Future::poll(resync.as_mut(), cx) {
        std::task::Poll::Ready(result) => std::task::Poll::Ready(result),
        std::task::Poll::Pending => {
            panic!("loaded block resync must not wait for the world writer")
        }
    })
    .await
    .unwrap();
    drop(resync);
    drop(world_writer);

    let mut buf = bytes::BytesMut::from(writer.as_slice());
    let mut frame = mc_protocol::frame::try_decode_frame(&mut buf, Compression::Disabled)
        .unwrap()
        .expect("authoritative block update");
    assert_eq!(frame.id, BlockUpdate::ID);
    let update = BlockUpdate::decode(&mut frame.body).unwrap();
    assert_eq!(update.position, pack_block_pos(pos.x, pos.y, pos.z));
    assert_eq!(update.state_id, 1);
    assert!(buf.is_empty());
}

#[tokio::test]
async fn rejected_use_item_on_resync_does_not_wait_for_world_writer() {
    let mut state = interaction_state_for_items(Arc::new(ItemRegistry::from_report(&[])));
    let clicked = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let target = mc_world::BlockPos { x: 1, y: 65, z: 1 };
    {
        let mut storage = state.world.lock().await;
        let cpos = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage.set_block_at(clicked, BlockStateId(1)).unwrap();
        storage.set_block_at(target, BlockStateId(2)).unwrap();
    }
    let world = Arc::clone(&state.world);
    let world_writer = world.lock().await;
    let mut writer = Vec::new();
    let mut resync = Box::pin(reject_use_item_on_with_resync(
        &mut state,
        &mut writer,
        InteractionHand::MainHand,
        17,
        clicked,
        target,
        UseItemOnNoOpReason::ConcurrentMutation,
        UseItemOnResyncOptions::WITH_HELD_ITEM,
    ));

    std::future::poll_fn(|cx| match std::future::Future::poll(resync.as_mut(), cx) {
        std::task::Poll::Ready(result) => std::task::Poll::Ready(result),
        std::task::Poll::Pending => {
            panic!("UseItemOn resync must not wait for the world writer")
        }
    })
    .await
    .unwrap();
    drop(resync);
    drop(world_writer);

    let mut buf = bytes::BytesMut::from(writer.as_slice());
    let mut first = mc_protocol::frame::try_decode_frame(&mut buf, Compression::Disabled)
        .unwrap()
        .expect("clicked block update");
    assert_eq!(first.id, BlockUpdate::ID);
    assert_eq!(BlockUpdate::decode(&mut first.body).unwrap().state_id, 1);
    let mut second = mc_protocol::frame::try_decode_frame(&mut buf, Compression::Disabled)
        .unwrap()
        .expect("target block update");
    assert_eq!(second.id, BlockUpdate::ID);
    assert_eq!(BlockUpdate::decode(&mut second.body).unwrap().state_id, 2);
    let ack = mc_protocol::frame::try_decode_frame(&mut buf, Compression::Disabled)
        .unwrap()
        .expect("block changed acknowledgement");
    assert_eq!(ack.id, BlockChangedAck::ID);
    assert!(buf.is_empty());
}
