use super::*;

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use bytes::BytesMut;
use mc_protocol::Packet;
use mc_protocol::packets::play::{BlockChangedAck, BlockUpdate, ClientboundContainerSetSlot};
use mc_script::precommit::{HookDecision, HookFailurePolicy, HookKind, HookRegistration};
use mc_script::{ScriptHostInput, script_boundary_pair};

use crate::play::simulation::simulation_channel;
use crate::play::tests::{interaction_state_for_items_and_blocks, register_survival_test_player};
use crate::play::{BlockEditPrecondition, SurvivalState, XpState};

#[tokio::test]
async fn cancelled_bucket_corrects_prediction_without_disconnect_or_inventory_debit() {
    let items = Arc::new(mc_data::items::solaris_required_items());
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .unwrap(),
    );
    let mut state = interaction_state_for_items_and_blocks(Arc::clone(&items), Arc::clone(&blocks));
    let water_bucket = items
        .id_of(&Identifier::parse("minecraft:water_bucket").unwrap())
        .unwrap();
    let empty_bucket = items
        .id_of(&Identifier::parse("minecraft:bucket").unwrap())
        .unwrap();
    let stone = blocks
        .block(&Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default;
    let water = blocks
        .block(&Identifier::parse("minecraft:water").unwrap())
        .unwrap()
        .default;
    let air = blocks
        .block(&Identifier::parse("minecraft:air").unwrap())
        .unwrap()
        .default;
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let token = {
        let mut storage = state.world.lock().await;
        let chunk = mc_world::ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                chunk,
                mc_world::Chunk::empty(chunk, air, Identifier::parse("minecraft:plains").unwrap()),
            )
            .unwrap();
        storage.set_block_at(pos, stone).unwrap();
        storage.block_mutation_token(pos).unwrap()
    };
    let held_slot = PlayerInventory::HOTBAR_BASE;
    let held = ItemStack::new(water_bucket, 1);
    state.inventory.slots[held_slot] = held.clone();
    let (session_id, _) = register_survival_test_player(
        &mut state,
        "CancelledBucket",
        SurvivalState::FULL,
        &XpState::default(),
    );
    let (simulation, mut owner) = simulation_channel();
    let (boundary, mut endpoint) =
        script_boundary_pair(NonZeroUsize::new(4).unwrap(), NonZeroUsize::new(4).unwrap());
    boundary
        .set_precommit_hooks(vec![HookRegistration::new(
            "judge",
            HookKind::Build,
            0,
            HookFailurePolicy::Deny,
        )])
        .unwrap();
    simulation.install_precommit_boundary(boundary);
    state.simulation = simulation.for_session(session_id);
    let sessions = Arc::clone(&state.sessions);
    sessions.mark_loaded(session_id, (0, 0));
    let world = Arc::clone(&state.world);
    let plan = BucketUsePlan {
        edit: BlockEdit {
            pos,
            new_state: water,
        },
        precondition: BlockEditPrecondition {
            pos,
            expected_state: stone,
            expected_token: token,
        },
        block_facts: Arc::clone(&state.block_facts),
        inventory: Some(BucketInventoryChange {
            held_slot,
            expected_held: held.clone(),
            replacement_item: empty_bucket,
            replacement_max_stack: 16,
        }),
        schedule_fluid_ticks: false,
        hook_approval: None,
        zone_fence: None,
    };
    let mut writer = Vec::new();
    let mut response = Box::pin(commit_bucket_use_and_respond(
        &mut state,
        &mut writer,
        31,
        plan,
    ));
    std::future::poll_fn(|context| {
        assert!(response.as_mut().poll(context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert!(owner.wait_for_command().await);
    owner.process_tick_with_world(&sessions, Some(&world), None, 1);
    let Some(ScriptHostInput::Precommit(request)) = endpoint.recv_input_blocking() else {
        panic!("expected native bucket hook");
    };
    request.answer(HookDecision::Cancel).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                result = &mut response => break result,
                ready = owner.wait_for_command() => {
                    assert!(ready);
                    owner.process_tick_with_world(&sessions, Some(&world), None, 1);
                }
            }
        }
    })
    .await
    .expect("bucket refusal and correction complete");
    assert!(result.expect("hook cancellation must not disconnect the player"));
    drop(response);
    assert_eq!(state.inventory.slots[held_slot], held);
    assert_eq!(world.lock().await.get_cached_block(pos), Some(stone));

    let mut bytes = BytesMut::from(writer.as_slice());
    let mut acknowledgements = Vec::new();
    let mut corrected_blocks = Vec::new();
    let mut corrected_items = Vec::new();
    while let Some(mut frame) =
        mc_protocol::frame::try_decode_frame(&mut bytes, state.compression).unwrap()
    {
        match frame.id {
            BlockChangedAck::ID => {
                acknowledgements.push(BlockChangedAck::decode(&mut frame.body).unwrap().sequence)
            }
            BlockUpdate::ID => {
                corrected_blocks.push(BlockUpdate::decode(&mut frame.body).unwrap().state_id)
            }
            ClientboundContainerSetSlot::ID => corrected_items.push(
                ClientboundContainerSetSlot::decode(&mut frame.body)
                    .unwrap()
                    .item_stack,
            ),
            _ => {}
        }
    }
    assert_eq!(acknowledgements, vec![31]);
    assert_eq!(corrected_blocks, vec![i32::try_from(stone.0).unwrap()]);
    assert_eq!(corrected_items, vec![held]);
}
