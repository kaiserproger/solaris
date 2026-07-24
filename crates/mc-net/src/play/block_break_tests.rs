use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};
use std::task::Poll;

use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_entity::{EntityItemStack, Vec3};
use mc_protocol::codec::Identifier;
use mc_protocol::frame::Compression;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    BlockChangedAck, Direction, GameMode, ItemStack, PlayerActionKind, ServerboundPlayerAction,
    pack_block_pos,
};
use tokio::sync::mpsc;

use super::block_break::{
    BlockBreakState, DelayedBreakOutcome, PendingBreak, StopBreakOutcome,
    handle_block_destroy_action, plan_break_block_edits, plan_break_edit_preconditions,
};
use super::inventory::PlayerInventory;
use super::persistence::{PlayerPersistedState, XpState};
use super::simulation::{SurvivalBreakDrop, SurvivalBreakHeldItem, SurvivalBreakPlan};
use super::survival::BlockMutationSnapshot;
use super::survival::SurvivalState;
use super::tests::{fluid_test_registry, insert_fluid_test_chunk, interaction_state_for_blocks};
use super::{BlockEdit, PlayerPose, simulation_channel};
use crate::login::{LoggedInProfile, offline_uuid};

fn target(state: u32) -> BlockMutationSnapshot {
    BlockMutationSnapshot {
        state: mc_world::BlockStateId(state),
        token: mc_world::BlockMutationToken {
            chunk_instance_id: 7,
            version: 11,
        },
    }
}

fn pending(position: i64, started_tick: u64) -> PendingBreak {
    PendingBreak {
        sequence: 1,
        position,
        direction: Direction::Up,
        started_tick,
        started_progress_per_tick: 0.1,
        held_hotbar_slot: 0,
        held_item: Some(ItemStack::new(10, 1)),
        expected_target: Some(target(1)),
        stop_received: false,
    }
}

fn stop(position: i64, sequence: i32) -> ServerboundPlayerAction {
    ServerboundPlayerAction {
        action: PlayerActionKind::StopDestroyBlock,
        position,
        direction: Direction::Up,
        sequence,
    }
}

fn stair_test_registry() -> Arc<mc_world::BlockRegistry> {
    let mut states = Vec::new();
    let mut id = 2;
    for facing in ["north", "south", "west", "east"] {
        for half in ["top", "bottom"] {
            for shape in [
                "straight",
                "inner_left",
                "inner_right",
                "outer_left",
                "outer_right",
            ] {
                for waterlogged in ["true", "false"] {
                    states.push(block_state_report(
                        id,
                        facing == "north"
                            && half == "bottom"
                            && shape == "straight"
                            && waterlogged == "false",
                        &[
                            ("facing", facing),
                            ("half", half),
                            ("shape", shape),
                            ("waterlogged", waterlogged),
                        ],
                    ));
                    id += 1;
                }
            }
        }
    }
    Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block_report(0, "minecraft:air"),
            simple_block_report(1, "minecraft:stone"),
            BlockReport {
                id: Identifier::parse("minecraft:oak_stairs").unwrap(),
                properties: property_schema(&[
                    ("facing", &["north", "south", "west", "east"]),
                    ("half", &["top", "bottom"]),
                    (
                        "shape",
                        &[
                            "straight",
                            "inner_left",
                            "inner_right",
                            "outer_left",
                            "outer_right",
                        ],
                    ),
                    ("waterlogged", &["true", "false"]),
                ]),
                states,
            },
            simple_block_report(id, "minecraft:malformed_stairs"),
        ])
        .unwrap(),
    )
}

fn simple_block_report(id: u32, name: &str) -> BlockReport {
    BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: BTreeMap::new(),
        states: vec![block_state_report(id, true, &[])],
    }
}

fn block_state_report(id: u32, default: bool, properties: &[(&str, &str)]) -> BlockStateReport {
    BlockStateReport {
        id,
        default,
        properties: properties
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
    }
}

fn property_schema(entries: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    entries
        .iter()
        .map(|(name, values)| {
            (
                (*name).to_owned(),
                values.iter().map(|value| (*value).to_owned()).collect(),
            )
        })
        .collect()
}

fn stair_state(
    blocks: &mc_world::BlockRegistry,
    facing: Direction,
    half: &str,
    shape: &str,
    waterlogged: &str,
) -> mc_world::BlockStateId {
    blocks
        .by_name_and_props(
            &Identifier::parse("minecraft:oak_stairs").unwrap(),
            &[
                ("facing".to_owned(), direction_name(facing).to_owned()),
                ("half".to_owned(), half.to_owned()),
                ("shape".to_owned(), shape.to_owned()),
                ("waterlogged".to_owned(), waterlogged.to_owned()),
            ],
        )
        .unwrap()
}

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::North => "north",
        Direction::South => "south",
        Direction::West => "west",
        Direction::East => "east",
        Direction::Down | Direction::Up => unreachable!("stair facing is horizontal"),
    }
}

fn opposite(direction: Direction) -> Direction {
    match direction {
        Direction::North => Direction::South,
        Direction::South => Direction::North,
        Direction::West => Direction::East,
        Direction::East => Direction::West,
        Direction::Down | Direction::Up => unreachable!("stair facing is horizontal"),
    }
}

fn counter_clockwise(direction: Direction) -> Direction {
    match direction {
        Direction::North => Direction::West,
        Direction::South => Direction::East,
        Direction::West => Direction::South,
        Direction::East => Direction::North,
        Direction::Down | Direction::Up => unreachable!("stair facing is horizontal"),
    }
}

fn clockwise(direction: Direction) -> Direction {
    opposite(counter_clockwise(direction))
}

fn relative(pos: mc_world::BlockPos, direction: Direction) -> mc_world::BlockPos {
    let (dx, dy, dz) = direction.normal();
    mc_world::BlockPos {
        x: pos.x + dx,
        y: pos.y + dy,
        z: pos.z + dz,
    }
}

fn stair_test_world(blocks: Arc<mc_world::BlockRegistry>) -> mc_world::WorldStorage {
    let mut world = mc_world::WorldStorage::in_memory(blocks);
    let chunk = mc_world::ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            chunk,
            mc_world::Chunk::empty(
                chunk,
                mc_world::BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    world
}

#[test]
fn removing_each_inner_and_outer_trigger_recomputes_the_surviving_stair() {
    let blocks = stair_test_registry();
    let center = mc_world::BlockPos { x: 8, y: 64, z: 8 };

    for facing in [
        Direction::North,
        Direction::South,
        Direction::West,
        Direction::East,
    ] {
        for half in ["top", "bottom"] {
            for (shape, trigger_direction, trigger_facing) in [
                ("outer_left", facing, counter_clockwise(facing)),
                ("outer_right", facing, clockwise(facing)),
                ("inner_left", opposite(facing), counter_clockwise(facing)),
                ("inner_right", opposite(facing), clockwise(facing)),
            ] {
                let trigger = relative(center, trigger_direction);
                let surviving = stair_state(&blocks, facing, half, shape, "true");
                let removed = stair_state(&blocks, trigger_facing, half, "straight", "false");
                let expected = stair_state(&blocks, facing, half, "straight", "true");
                let mut world = stair_test_world(Arc::clone(&blocks));
                world.set_block_at(center, surviving).unwrap();
                world.set_block_at(trigger, removed).unwrap();

                let edits = plan_break_block_edits(
                    &blocks,
                    &world,
                    trigger,
                    removed,
                    mc_world::BlockStateId(0),
                    mc_world::BlockStateId(0),
                );

                assert!(
                    edits.contains(&BlockEdit {
                        pos: center,
                        new_state: expected,
                    }),
                    "facing={facing:?}, half={half}, shape={shape}, edits={edits:?}"
                );
            }
        }
    }
}

#[test]
fn removing_outer_trigger_reveals_the_remaining_inner_shape() {
    let blocks = stair_test_registry();
    let center = mc_world::BlockPos { x: 8, y: 64, z: 8 };

    for facing in [
        Direction::North,
        Direction::South,
        Direction::West,
        Direction::East,
    ] {
        for half in ["top", "bottom"] {
            for (outer_shape, inner_shape, turn) in [
                ("outer_left", "inner_left", counter_clockwise(facing)),
                ("outer_right", "inner_right", clockwise(facing)),
            ] {
                let trigger = relative(center, facing);
                let remaining = relative(center, opposite(facing));
                let surviving = stair_state(&blocks, facing, half, outer_shape, "true");
                let perpendicular = stair_state(&blocks, turn, half, "straight", "false");
                let expected = stair_state(&blocks, facing, half, inner_shape, "true");
                let mut world = stair_test_world(Arc::clone(&blocks));
                world.set_block_at(center, surviving).unwrap();
                world.set_block_at(trigger, perpendicular).unwrap();
                world.set_block_at(remaining, perpendicular).unwrap();

                let edits = plan_break_block_edits(
                    &blocks,
                    &world,
                    trigger,
                    perpendicular,
                    mc_world::BlockStateId(0),
                    mc_world::BlockStateId(0),
                );

                assert!(
                    edits.contains(&BlockEdit {
                        pos: center,
                        new_state: expected,
                    }),
                    "facing={facing:?}, half={half}, outer={outer_shape}, edits={edits:?}"
                );
            }
        }
    }
}

#[test]
fn replacing_a_stair_trigger_uses_the_replacement_identity_half_and_facing() {
    let blocks = stair_test_registry();
    let center = mc_world::BlockPos { x: 8, y: 64, z: 8 };
    let trigger = relative(center, Direction::North);
    let surviving = stair_state(&blocks, Direction::North, "bottom", "outer_left", "true");
    let removed = stair_state(&blocks, Direction::West, "bottom", "straight", "false");
    let cases = [
        (mc_world::BlockStateId(1), "straight"),
        (
            stair_state(&blocks, Direction::West, "top", "straight", "false"),
            "straight",
        ),
        (
            stair_state(&blocks, Direction::North, "bottom", "straight", "false"),
            "straight",
        ),
        (
            stair_state(&blocks, Direction::East, "bottom", "straight", "false"),
            "outer_right",
        ),
    ];

    for (replacement, expected_shape) in cases {
        let expected = stair_state(&blocks, Direction::North, "bottom", expected_shape, "true");
        let mut world = stair_test_world(Arc::clone(&blocks));
        world.set_block_at(center, surviving).unwrap();
        world.set_block_at(trigger, removed).unwrap();

        let edits = plan_break_block_edits(
            &blocks,
            &world,
            trigger,
            removed,
            replacement,
            mc_world::BlockStateId(0),
        );

        assert!(
            edits.contains(&BlockEdit {
                pos: center,
                new_state: expected,
            }),
            "replacement={replacement:?}, expected_shape={expected_shape}, edits={edits:?}"
        );
    }
}

#[test]
fn stair_transition_updates_chained_neighbours_in_direction_order_without_duplicates() {
    let blocks = stair_test_registry();
    let root = mc_world::BlockPos { x: 8, y: 64, z: 8 };
    let north = relative(root, Direction::North);
    let south = relative(root, Direction::South);
    let west = relative(root, Direction::West);
    let east = relative(root, Direction::East);
    let removed = stair_state(&blocks, Direction::West, "bottom", "straight", "false");
    let north_before = stair_state(&blocks, Direction::North, "bottom", "inner_left", "true");
    let south_before = stair_state(&blocks, Direction::South, "bottom", "inner_right", "false");
    let west_before = stair_state(&blocks, Direction::West, "bottom", "outer_left", "true");
    let east_before = stair_state(&blocks, Direction::East, "bottom", "outer_right", "false");
    let mut world = stair_test_world(Arc::clone(&blocks));
    world.set_block_at(root, removed).unwrap();
    world.set_block_at(north, north_before).unwrap();
    world.set_block_at(south, south_before).unwrap();
    world.set_block_at(west, west_before).unwrap();
    world.set_block_at(east, east_before).unwrap();

    let edits = plan_break_block_edits(
        &blocks,
        &world,
        root,
        removed,
        mc_world::BlockStateId(0),
        mc_world::BlockStateId(0),
    );

    assert_eq!(
        edits,
        vec![
            BlockEdit {
                pos: root,
                new_state: mc_world::BlockStateId(0),
            },
            BlockEdit {
                pos: north,
                new_state: stair_state(&blocks, Direction::North, "bottom", "straight", "true",),
            },
            BlockEdit {
                pos: south,
                new_state: stair_state(&blocks, Direction::South, "bottom", "straight", "false",),
            },
            BlockEdit {
                pos: west,
                new_state: stair_state(&blocks, Direction::West, "bottom", "straight", "true",),
            },
            BlockEdit {
                pos: east,
                new_state: stair_state(&blocks, Direction::East, "bottom", "straight", "false",),
            },
        ]
    );
}

#[test]
fn unchanged_stair_transition_is_a_noop() {
    let blocks = stair_test_registry();
    let root = mc_world::BlockPos { x: 8, y: 64, z: 8 };
    let state = stair_state(&blocks, Direction::West, "bottom", "straight", "false");
    let mut world = stair_test_world(Arc::clone(&blocks));
    world.set_block_at(root, state).unwrap();

    assert!(
        plan_break_block_edits(
            &blocks,
            &world,
            root,
            state,
            state,
            mc_world::BlockStateId(0)
        )
        .is_empty()
    );
}

#[test]
fn malformed_or_unloaded_stair_transition_dependencies_fail_closed() {
    let blocks = stair_test_registry();
    let malformed = blocks
        .block(&Identifier::parse("minecraft:malformed_stairs").unwrap())
        .unwrap()
        .default;
    let root = mc_world::BlockPos { x: 8, y: 64, z: 8 };
    let removed = stair_state(&blocks, Direction::West, "bottom", "straight", "false");
    let mut malformed_world = stair_test_world(Arc::clone(&blocks));
    malformed_world.set_block_at(root, removed).unwrap();
    malformed_world
        .set_block_at(relative(root, Direction::North), malformed)
        .unwrap();
    assert!(
        plan_break_block_edits(
            &blocks,
            &malformed_world,
            root,
            removed,
            mc_world::BlockStateId(0),
            mc_world::BlockStateId(0),
        )
        .is_empty()
    );

    let boundary = mc_world::BlockPos { x: 15, ..root };
    let mut unloaded_world = stair_test_world(Arc::clone(&blocks));
    unloaded_world.set_block_at(boundary, removed).unwrap();
    assert!(
        plan_break_block_edits(
            &blocks,
            &unloaded_world,
            boundary,
            removed,
            mc_world::BlockStateId(0),
            mc_world::BlockStateId(0),
        )
        .is_empty()
    );
}

#[test]
fn breaking_non_stair_does_not_depend_on_neighbor_chunks() {
    let blocks = stair_test_registry();
    let root = mc_world::BlockPos { x: 15, y: 64, z: 8 };
    let mut world = stair_test_world(Arc::clone(&blocks));
    world.set_block_at(root, mc_world::BlockStateId(1)).unwrap();
    let expected_root = BlockMutationSnapshot {
        state: mc_world::BlockStateId(1),
        token: world.block_mutation_token(root).unwrap(),
    };
    let edits = plan_break_block_edits(
        &blocks,
        &world,
        root,
        expected_root.state,
        mc_world::BlockStateId(0),
        mc_world::BlockStateId(0),
    );
    let preconditions =
        plan_break_edit_preconditions(&blocks, &world, &edits, root, expected_root).unwrap();

    assert_eq!(
        preconditions
            .iter()
            .map(|precondition| precondition.pos)
            .collect::<Vec<_>>(),
        vec![root]
    );
}

#[tokio::test]
async fn stale_stair_dependency_rolls_back_break_tool_and_drop_publication() {
    let blocks = stair_test_registry();
    let state = interaction_state_for_blocks(Arc::clone(&blocks));
    insert_fluid_test_chunk(&state).await;
    let center = mc_world::BlockPos { x: 8, y: 64, z: 8 };
    let root = relative(center, Direction::North);
    let guard = relative(center, Direction::South);
    let surviving = stair_state(&blocks, Direction::North, "bottom", "outer_left", "true");
    let removed = stair_state(&blocks, Direction::West, "bottom", "straight", "false");
    let expected_root = {
        let mut world = state.world.lock().await;
        world.set_block_at(center, surviving).unwrap();
        world.set_block_at(root, removed).unwrap();
        BlockMutationSnapshot {
            state: removed,
            token: world.block_mutation_token(root).unwrap(),
        }
    };
    let (edits, preconditions) = {
        let world = state.world.lock().await;
        let edits = plan_break_block_edits(
            &blocks,
            &*world,
            root,
            removed,
            mc_world::BlockStateId(0),
            mc_world::BlockStateId(0),
        );
        let preconditions =
            plan_break_edit_preconditions(&blocks, &*world, &edits, root, expected_root).unwrap();
        (edits, preconditions)
    };
    assert!(
        preconditions
            .iter()
            .any(|precondition| precondition.pos == guard),
        "selector-only guard must be part of the atomic read footprint"
    );

    let pose = PlayerPose::new(8.5, 64.0, 8.5);
    let profile = LoggedInProfile {
        uuid: offline_uuid("StaleStairBreak"),
        name: "StaleStairBreak".to_owned(),
    };
    let (outbound, _outbound_rx) = mpsc::channel(8);
    let (session_id, _) =
        state
            .sessions
            .register(&profile, (0, 0), 0, HashSet::new(), outbound, pose);
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
    let mut persisted = PlayerPersistedState::new_default(pose);
    persisted.inventory = inventory;
    let persisted = Arc::new(Mutex::new(persisted));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&persisted));
    let (simulation, mut owner) = simulation_channel();
    let session_simulation = simulation.for_session(session_id);
    let mut request = Box::pin(session_simulation.commit_survival_break(SurvivalBreakPlan {
        edits,
        preconditions,
        blocks: Arc::clone(&blocks),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        falling_block_entity_type_id: None,
        held: SurvivalBreakHeldItem {
            hotbar_slot: 0,
            expected: ItemStack::new(42, 1),
            max_damage: Some(10),
        },
        drops: vec![SurvivalBreakDrop {
            entity_type_id: 7,
            position: Vec3::new(8.5, 64.5, 8.5),
            stack: EntityItemStack::new(9, 1),
        }],
    }));
    std::future::poll_fn(|context| {
        assert!(request.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    state
        .world
        .lock()
        .await
        .set_block_at(guard, mc_world::BlockStateId(1))
        .unwrap();

    assert_eq!(
        owner
            .process_tick_with_world(&state.sessions, Some(&state.world), None, 1)
            .processed,
        1
    );
    assert!(request.await.unwrap().is_none());
    let world = state.world.lock().await;
    assert_eq!(world.get_cached_block(root), Some(removed));
    assert_eq!(world.get_cached_block(center), Some(surviving));
    drop(world);
    assert_eq!(
        persisted.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 1)
    );
    assert!(
        state
            .sessions
            .persisted_entity_records()
            .into_iter()
            .all(|record| record.snapshot.item_stack.is_none())
    );
}

#[tokio::test]
async fn out_of_reach_destroy_is_ack_only_like_vanilla() {
    let mut state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    let target = mc_world::BlockPos { x: 0, y: 64, z: 0 };
    let pose = PlayerPose::new(20.5, 66.0, 0.5);
    let mut writer = Vec::new();
    let mut survival = SurvivalState::FULL;
    let mut xp = XpState::default();

    handle_block_destroy_action(
        &mut state,
        &mut writer,
        None,
        GameMode::Survival,
        &mut survival,
        &mut xp,
        pose,
        ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: pack_block_pos(target.x, target.y, target.z),
            direction: Direction::Up,
            sequence: 44,
        },
    )
    .await
    .unwrap();

    let mut bytes = bytes::BytesMut::from(writer.as_slice());
    let mut frame = mc_protocol::frame::try_decode_frame(&mut bytes, Compression::Disabled)
        .unwrap()
        .expect("out-of-reach destroy acknowledgement");
    assert_eq!(frame.id, BlockChangedAck::ID);
    assert_eq!(
        BlockChangedAck::decode(&mut frame.body).unwrap().sequence,
        44
    );
    assert!(
        bytes.is_empty(),
        "vanilla sends no target block resync for an out-of-reach START"
    );
    assert!(state.pending_break.is_none());
    assert!(state.pending_use.is_none());
}

#[tokio::test]
async fn start_tick_is_captured_before_owner_snapshot_queue_latency() {
    let mut state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    insert_fluid_test_chunk(&state).await;
    let target = mc_world::BlockPos { x: 0, y: 64, z: 0 };
    state
        .world
        .lock()
        .await
        .set_block_at(target, mc_world::BlockStateId(1))
        .unwrap();

    let (simulation, mut owner) = simulation_channel();
    let pose = PlayerPose::new(0.5, 66.0, 0.5);
    let profile = LoggedInProfile {
        uuid: offline_uuid("BreakStartTiming"),
        name: "BreakStartTiming".to_owned(),
    };
    let (outbound, _outbound_rx) = mpsc::channel(8);
    let (session_id, _) =
        state
            .sessions
            .register(&profile, (0, 0), 0, HashSet::new(), outbound, pose);
    state.session_id = session_id;
    state.simulation = simulation.for_session(session_id);

    let sessions = Arc::clone(&state.sessions);
    let world = Arc::clone(&state.world);
    let mut writer = Vec::new();
    let mut survival = SurvivalState::FULL;
    let mut xp = XpState::default();
    let mut request = Box::pin(handle_block_destroy_action(
        &mut state,
        &mut writer,
        None,
        GameMode::Survival,
        &mut survival,
        &mut xp,
        pose,
        ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: pack_block_pos(target.x, target.y, target.z),
            direction: Direction::Up,
            sequence: 4,
        },
    ));
    std::future::poll_fn(|cx| {
        assert!(
            request.as_mut().poll(cx).is_pending(),
            "START must await the queued owner snapshot"
        );
        Poll::Ready(())
    })
    .await;

    sessions.advance_world_time(5);
    assert_eq!(
        owner
            .process_commands_with_world(&sessions, Some(&world), None, 1)
            .await
            .processed,
        1
    );
    request.await.unwrap();

    assert_eq!(sessions.simulation_tick(), 5);
    assert_eq!(
        state
            .pending_break
            .as_ref()
            .expect("non-instant break remains active")
            .started_tick,
        0
    );
}

#[test]
fn stop_at_vanilla_threshold_completes_immediately() {
    let mut active = Some(pending(12, 40));
    let mut delayed = None;
    let outcome = BlockBreakState::new(&mut active, &mut delayed).stop(&stop(12, 5), 46, 0.1);

    let StopBreakOutcome::Complete(completion) = outcome else {
        panic!("expected immediate completion");
    };
    assert!(completion.acknowledgement.should_send());
    assert!(active.is_none());
    assert!(delayed.is_none());
}

#[test]
fn early_stop_acknowledges_and_transfers_to_delayed_progress() {
    let mut active = Some(pending(12, 40));
    let mut delayed = None;
    let outcome = BlockBreakState::new(&mut active, &mut delayed).stop(&stop(12, 5), 44, 0.1);

    assert_eq!(outcome, StopBreakOutcome::Acknowledge { delayed: true });
    assert!(active.is_none());
    assert_eq!(delayed.as_ref().map(|pending| pending.position), Some(12));
}

#[test]
fn new_start_does_not_overwrite_existing_delayed_break() {
    let mut active = None;
    let mut delayed = Some(pending(12, 40));
    BlockBreakState::new(&mut active, &mut delayed).start(pending(24, 50));

    assert_eq!(active.as_ref().map(|pending| pending.position), Some(24));
    assert_eq!(delayed.as_ref().map(|pending| pending.position), Some(12));
}

#[test]
fn second_early_stop_queues_active_behind_existing_delayed_break() {
    let mut active = Some(pending(24, 50));
    let mut delayed = Some(pending(12, 40));
    let outcome = BlockBreakState::new(&mut active, &mut delayed).stop(&stop(24, 8), 52, 0.1);

    assert_eq!(outcome, StopBreakOutcome::Acknowledge { delayed: true });
    assert_eq!(active.as_ref().map(|pending| pending.position), Some(24));
    assert!(active.as_ref().is_some_and(|pending| pending.stop_received));
    assert_eq!(delayed.as_ref().map(|pending| pending.position), Some(12));
}

#[test]
fn completed_delayed_break_promotes_the_queued_stop() {
    let mut active = Some(pending(24, 45));
    active.as_mut().unwrap().stop_received = true;
    let mut delayed = Some(pending(12, 40));

    let outcome = BlockBreakState::new(&mut active, &mut delayed).tick_delayed(49, 0.1);

    assert!(matches!(outcome, DelayedBreakOutcome::Complete(_)));
    assert!(active.is_none());
    assert_eq!(delayed.as_ref().map(|pending| pending.position), Some(24));
}

#[test]
fn cancelled_delayed_break_promotes_the_queued_stop() {
    let mut active = Some(pending(24, 45));
    active.as_mut().unwrap().stop_received = true;
    let mut cancelled = pending(12, 40);
    cancelled.expected_target = None;
    let mut delayed = Some(cancelled);

    let outcome = BlockBreakState::new(&mut active, &mut delayed).tick_delayed(49, 0.1);

    assert_eq!(outcome, DelayedBreakOutcome::Cancelled);
    assert!(active.is_none());
    assert_eq!(delayed.as_ref().map(|pending| pending.position), Some(24));
}

#[test]
fn delayed_break_completes_at_one_without_requesting_another_ack() {
    let mut active = None;
    let mut delayed = Some(pending(12, 40));
    let outcome = BlockBreakState::new(&mut active, &mut delayed).tick_delayed(49, 0.1);

    let DelayedBreakOutcome::Complete(completion) = outcome else {
        panic!("expected delayed completion");
    };
    assert!(!completion.acknowledgement.should_send());
    assert!(delayed.is_none());
}

#[test]
fn delayed_break_remains_pending_below_one() {
    let mut active = None;
    let mut delayed = Some(pending(12, 40));
    let outcome = BlockBreakState::new(&mut active, &mut delayed).tick_delayed(48, 0.1);

    assert_eq!(outcome, DelayedBreakOutcome::Pending);
    assert!(delayed.is_some());
}

#[test]
fn mismatched_stop_preserves_the_active_break() {
    let mut active = Some(pending(12, 40));
    let mut delayed = None;
    let outcome = BlockBreakState::new(&mut active, &mut delayed).stop(&stop(24, 5), 49, 0.1);

    assert_eq!(outcome, StopBreakOutcome::Acknowledge { delayed: false });
    assert_eq!(active.as_ref().map(|pending| pending.position), Some(12));
    assert!(delayed.is_none());
}

#[test]
fn stop_on_another_face_of_the_same_block_completes() {
    let mut active = Some(pending(12, 40));
    let mut delayed = None;
    let mut action = stop(12, 5);
    action.direction = Direction::Down;
    let outcome = BlockBreakState::new(&mut active, &mut delayed).stop(&action, 46, 0.1);

    assert!(matches!(outcome, StopBreakOutcome::Complete(_)));
    assert!(active.is_none());
    assert!(delayed.is_none());
}

#[test]
fn delayed_break_without_owner_snapshot_is_cancelled() {
    let mut active = None;
    let mut delayed_pending = pending(12, 40);
    delayed_pending.expected_target = None;
    let mut delayed = Some(delayed_pending);
    let outcome = BlockBreakState::new(&mut active, &mut delayed).tick_delayed(49, 0.1);

    assert_eq!(outcome, DelayedBreakOutcome::Cancelled);
    assert!(delayed.is_none());
}
