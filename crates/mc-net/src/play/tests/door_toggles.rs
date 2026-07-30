use super::{
    BlockDelta, BlockDeltaPacket, BlockEdit, BlockReport, BlockStateId, BlockUpdate, Compression,
    Identifier, LoggedInProfile, OutboundCommand, PlayerPose, SectionBlockChange,
    SectionBlocksUpdate, SessionRegistry, block_state_property, dispatch_and_clear_setup_packets,
    hand_toggle_test_registry, in_memory_button_world, pack_block_pos, plan_block_delta_packets,
    plan_toggle_block_interaction, prop_schema, send_block_deltas, simple_block,
    simulation_channel, state, toggled_bool_state,
};
use mc_protocol::Packet;
use std::collections::HashSet;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::mpsc;

#[test]
fn interactive_toggle_helpers_preserve_other_properties() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:oak_trapdoor").unwrap(),
            properties: prop_schema(&[
                ("facing", &["north"]),
                ("open", &["false", "true"]),
                ("waterlogged", &["false"]),
            ]),
            states: vec![
                state(
                    1,
                    true,
                    &[
                        ("facing", "north"),
                        ("open", "false"),
                        ("waterlogged", "false"),
                    ],
                ),
                state(
                    2,
                    false,
                    &[
                        ("facing", "north"),
                        ("open", "true"),
                        ("waterlogged", "false"),
                    ],
                ),
            ],
        },
        BlockReport {
            id: Identifier::parse("minecraft:lever").unwrap(),
            properties: prop_schema(&[("facing", &["north"]), ("powered", &["false", "true"])]),
            states: vec![
                state(3, true, &[("facing", "north"), ("powered", "false")]),
                state(4, false, &[("facing", "north"), ("powered", "true")]),
            ],
        },
    ])
    .unwrap();

    assert_eq!(
        toggled_bool_state(
            &blocks,
            blocks.by_id(mc_world::BlockStateId(1)).unwrap(),
            "open"
        ),
        Some(mc_world::BlockStateId(2))
    );
    assert_eq!(
        toggled_bool_state(
            &blocks,
            blocks.by_id(mc_world::BlockStateId(3)).unwrap(),
            "powered"
        ),
        Some(mc_world::BlockStateId(4))
    );
}

#[test]
fn hand_toggle_respects_door_and_trapdoor_material() {
    let blocks = Arc::new(hand_toggle_test_registry());
    let mut world = in_memory_button_world(Arc::clone(&blocks));
    let oak_lower = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let oak_upper = mc_world::BlockPos { y: 65, ..oak_lower };
    let iron_lower = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let iron_upper = mc_world::BlockPos {
        y: 65,
        ..iron_lower
    };
    let copper_lower = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    let copper_upper = mc_world::BlockPos {
        y: 65,
        ..copper_lower
    };
    let oak_trapdoor = mc_world::BlockPos { x: 4, y: 64, z: 1 };
    let iron_trapdoor = mc_world::BlockPos { x: 5, y: 64, z: 1 };
    let copper_trapdoor = mc_world::BlockPos { x: 6, y: 64, z: 1 };
    let oak_fence_gate = mc_world::BlockPos { x: 7, y: 64, z: 1 };

    for (pos, state_id) in [
        (oak_lower, 1),
        (oak_upper, 2),
        (iron_lower, 5),
        (iron_upper, 6),
        (copper_lower, 9),
        (copper_upper, 10),
        (oak_trapdoor, 13),
        (iron_trapdoor, 15),
        (copper_trapdoor, 17),
        (oak_fence_gate, 19),
    ] {
        world
            .set_block_at(pos, mc_world::BlockStateId(state_id))
            .expect("place toggle test block");
    }

    let oak_plan =
        plan_toggle_block_interaction(&blocks, &world, oak_lower, mc_world::BlockStateId(1), 0)
            .expect("oak door should open by hand");
    assert_eq!(
        oak_plan.edits,
        vec![
            BlockEdit {
                pos: oak_lower,
                new_state: mc_world::BlockStateId(3),
            },
            BlockEdit {
                pos: oak_upper,
                new_state: mc_world::BlockStateId(4),
            },
        ]
    );

    let copper_plan =
        plan_toggle_block_interaction(&blocks, &world, copper_lower, mc_world::BlockStateId(9), 0)
            .expect("copper door should open by hand");
    assert_eq!(
        copper_plan.edits,
        vec![
            BlockEdit {
                pos: copper_lower,
                new_state: mc_world::BlockStateId(11),
            },
            BlockEdit {
                pos: copper_upper,
                new_state: mc_world::BlockStateId(12),
            },
        ]
    );

    assert!(
        plan_toggle_block_interaction(&blocks, &world, iron_lower, mc_world::BlockStateId(5), 0,)
            .is_none(),
        "iron door must not open by hand"
    );

    let oak_trapdoor_plan =
        plan_toggle_block_interaction(&blocks, &world, oak_trapdoor, mc_world::BlockStateId(13), 0)
            .expect("oak trapdoor should open by hand");
    assert_eq!(
        oak_trapdoor_plan.edits,
        vec![BlockEdit {
            pos: oak_trapdoor,
            new_state: mc_world::BlockStateId(14),
        }]
    );

    let copper_trapdoor_plan = plan_toggle_block_interaction(
        &blocks,
        &world,
        copper_trapdoor,
        mc_world::BlockStateId(17),
        0,
    )
    .expect("copper trapdoor should open by hand");
    assert_eq!(
        copper_trapdoor_plan.edits,
        vec![BlockEdit {
            pos: copper_trapdoor,
            new_state: mc_world::BlockStateId(18),
        }]
    );

    assert!(
        plan_toggle_block_interaction(
            &blocks,
            &world,
            iron_trapdoor,
            mc_world::BlockStateId(15),
            0,
        )
        .is_none(),
        "iron trapdoor must not open by hand"
    );

    let fence_gate_plan = plan_toggle_block_interaction(
        &blocks,
        &world,
        oak_fence_gate,
        mc_world::BlockStateId(19),
        0,
    )
    .expect("oak fence gate should open by hand");
    assert_eq!(
        fence_gate_plan.edits,
        vec![BlockEdit {
            pos: oak_fence_gate,
            new_state: mc_world::BlockStateId(20),
        }]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn two_client_door_and_trapdoor_toggles_converge_and_reject_stale_retry() {
    let blocks = Arc::new(hand_toggle_test_registry());
    let mut storage = in_memory_button_world(Arc::clone(&blocks));
    let door_lower = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let door_upper = mc_world::BlockPos {
        y: 65,
        ..door_lower
    };
    let trapdoor = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    for (pos, state) in [
        (door_lower, BlockStateId(1)),
        (door_upper, BlockStateId(2)),
        (trapdoor, BlockStateId(13)),
    ] {
        storage
            .set_block_at(pos, state)
            .expect("seed hand-toggle state");
    }

    let door_plan =
        plan_toggle_block_interaction(&blocks, &storage, door_lower, BlockStateId(1), 0)
            .expect("closed oak door plans one atomic two-half toggle");
    let trapdoor_plan =
        plan_toggle_block_interaction(&blocks, &storage, trapdoor, BlockStateId(13), 0)
            .expect("closed oak trapdoor plans one toggle");
    assert_eq!(door_plan.preconditions.len(), 2);
    assert_eq!(trapdoor_plan.preconditions.len(), 1);

    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = SessionRegistry::new();
    let (actor_tx, mut actor_rx) = mpsc::channel(16);
    let (observer_tx, mut observer_rx) = mpsc::channel(16);
    let actor_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(1001),
        name: "DoorActor".to_owned(),
    };
    let observer_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(1002),
        name: "DoorObserver".to_owned(),
    };
    let (actor, _) = sessions.register(
        &actor_profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        actor_tx,
        PlayerPose::new(1.5, 64.0, 3.5),
    );
    let (observer, _) = sessions.register(
        &observer_profile,
        (0, 0),
        0,
        HashSet::from([(0, 0)]),
        observer_tx,
        PlayerPose::new(4.5, 64.0, 3.5),
    );
    let mut setup = sessions.mark_loaded(actor, (0, 0));
    setup.extend(sessions.mark_loaded(observer, (0, 0)));
    dispatch_and_clear_setup_packets(setup, &mut [&mut actor_rx, &mut observer_rx]);

    let (handle, mut owner) = simulation_channel();
    let actor_handle = handle.for_session(actor);
    let mut door_request = Box::pin(
        actor_handle.apply_block_edits(door_plan.edits.clone(), door_plan.preconditions.clone()),
    );
    let waker = std::task::Waker::noop();
    let mut context = Context::from_waker(waker);
    assert!(matches!(
        std::future::Future::poll(door_request.as_mut(), &mut context),
        Poll::Pending
    ));
    owner
        .process_commands_with_world(&sessions, Some(&world), None, 1)
        .await;
    assert!(door_request.await.unwrap().is_some());
    let door_deltas = match observer_rx.try_recv().expect("observer door publication") {
        OutboundCommand::BlockDeltas(deltas) => deltas,
        other => panic!("expected observer door BlockDeltas, got {other:?}"),
    };
    let expected_door_deltas = vec![
        BlockDelta {
            x: door_lower.x,
            y: door_lower.y,
            z: door_lower.z,
            state_id: BlockStateId(3),
        },
        BlockDelta {
            x: door_upper.x,
            y: door_upper.y,
            z: door_upper.z,
            state_id: BlockStateId(4),
        },
    ];
    assert_eq!(door_deltas, expected_door_deltas);
    assert_eq!(
        plan_block_delta_packets(&door_deltas),
        vec![BlockDeltaPacket::Section {
            section_x: 0,
            section_y: 4,
            section_z: 0,
            changes: door_deltas.clone(),
        }]
    );
    let mut wire = Vec::new();
    send_block_deltas(&mut wire, Compression::Disabled, &door_deltas, None)
        .await
        .expect("encode observer door deltas");
    let mut frames = bytes::BytesMut::from(wire.as_slice());
    let mut frame = mc_protocol::frame::try_decode_frame(&mut frames, Compression::Disabled)
        .expect("decode observer door frame")
        .expect("observer door frame");
    assert_eq!(frame.id, SectionBlocksUpdate::ID);
    assert_eq!(
        SectionBlocksUpdate::decode(&mut frame.body).expect("decode section door update"),
        SectionBlocksUpdate {
            section_pos: mc_protocol::packets::play::pack_section_pos(0, 4, 0),
            changes: vec![
                SectionBlockChange {
                    relative_pos: mc_protocol::packets::play::pack_section_relative_pos(
                        door_lower.x,
                        door_lower.y,
                        door_lower.z,
                    ),
                    state_id: 3,
                },
                SectionBlockChange {
                    relative_pos: mc_protocol::packets::play::pack_section_relative_pos(
                        door_upper.x,
                        door_upper.y,
                        door_upper.z,
                    ),
                    state_id: 4,
                },
            ],
        }
    );
    assert!(frames.is_empty());

    let mut trapdoor_request = Box::pin(actor_handle.apply_block_edits(
        trapdoor_plan.edits.clone(),
        trapdoor_plan.preconditions.clone(),
    ));
    assert!(matches!(
        std::future::Future::poll(trapdoor_request.as_mut(), &mut context),
        Poll::Pending
    ));
    owner
        .process_commands_with_world(&sessions, Some(&world), None, 1)
        .await;
    assert!(trapdoor_request.await.unwrap().is_some());
    let trapdoor_deltas = match observer_rx
        .try_recv()
        .expect("observer trapdoor publication")
    {
        OutboundCommand::BlockDeltas(deltas) => deltas,
        other => panic!("expected observer trapdoor BlockDeltas, got {other:?}"),
    };
    assert_eq!(
        trapdoor_deltas,
        vec![BlockDelta {
            x: trapdoor.x,
            y: trapdoor.y,
            z: trapdoor.z,
            state_id: BlockStateId(14),
        }]
    );
    let mut wire = Vec::new();
    send_block_deltas(&mut wire, Compression::Disabled, &trapdoor_deltas, None)
        .await
        .expect("encode actor trapdoor delta");
    let mut frames = bytes::BytesMut::from(wire.as_slice());
    let mut frame = mc_protocol::frame::try_decode_frame(&mut frames, Compression::Disabled)
        .expect("decode actor trapdoor frame")
        .expect("actor trapdoor frame");
    assert_eq!(frame.id, BlockUpdate::ID);
    assert_eq!(
        BlockUpdate::decode(&mut frame.body).expect("decode actor trapdoor update"),
        BlockUpdate {
            position: pack_block_pos(trapdoor.x, trapdoor.y, trapdoor.z),
            state_id: 14,
        }
    );
    assert!(frames.is_empty());

    assert!(actor_rx.try_recv().is_err());
    assert!(observer_rx.try_recv().is_err());

    let mut stale_retry =
        Box::pin(actor_handle.apply_block_edits(door_plan.edits, door_plan.preconditions));
    assert!(matches!(
        std::future::Future::poll(stale_retry.as_mut(), &mut context),
        Poll::Pending
    ));
    owner
        .process_commands_with_world(&sessions, Some(&world), None, 1)
        .await;
    assert!(stale_retry.await.unwrap().is_none());
    assert!(observer_rx.try_recv().is_err());
    assert!(actor_rx.try_recv().is_err());
    owner
        .process_commands_with_world(&sessions, Some(&world), None, 2)
        .await;
    assert!(observer_rx.try_recv().is_err());
    assert!(actor_rx.try_recv().is_err());

    let storage = world.lock().await;
    assert_eq!(storage.get_cached_block(door_lower), Some(BlockStateId(3)));
    assert_eq!(storage.get_cached_block(door_upper), Some(BlockStateId(4)));
    assert_eq!(storage.get_cached_block(trapdoor), Some(BlockStateId(14)));
    for (pos, expected_half) in [(door_lower, "lower"), (door_upper, "upper")] {
        let state = blocks
            .by_id(
                storage
                    .get_cached_block(pos)
                    .expect("door half remains loaded"),
            )
            .expect("door half state remains registered");
        assert_eq!(block_state_property(state, "facing"), Some("north"));
        assert_eq!(block_state_property(state, "half"), Some(expected_half));
        assert_eq!(block_state_property(state, "open"), Some("true"));
    }
    let state = blocks
        .by_id(
            storage
                .get_cached_block(trapdoor)
                .expect("trapdoor remains loaded"),
        )
        .expect("trapdoor state remains registered");
    assert_eq!(block_state_property(state, "facing"), Some("north"));
    assert_eq!(block_state_property(state, "half"), Some("bottom"));
    assert_eq!(block_state_property(state, "open"), Some("true"));
}
