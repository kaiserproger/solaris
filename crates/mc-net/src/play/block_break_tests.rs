use std::collections::HashSet;
use std::sync::Arc;
use std::task::Poll;

use mc_protocol::codec::Identifier;
use mc_protocol::packets::play::{
    Direction, GameMode, ItemStack, PlayerActionKind, ServerboundPlayerAction, pack_block_pos,
};
use tokio::sync::mpsc;

use super::block_break::{
    BlockBreakState, DelayedBreakOutcome, HeldMiningTool, PendingBreak, StopBreakOutcome,
    handle_block_destroy_action, mining_tool_matches,
};
use super::persistence::XpState;
use super::survival::BlockMutationSnapshot;
use super::survival::SurvivalState;
use super::tests::{fluid_test_registry, insert_fluid_test_chunk, interaction_state_for_blocks};
use super::{PlayerPose, simulation_channel};
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
    let outcome = BlockBreakState::new(&mut active, &mut delayed).stop(
        &stop(12, 5),
        HeldMiningTool {
            hotbar_slot: 0,
            stack: Some(&ItemStack::new(10, 64)),
        },
        46,
        0.1,
    );

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
    let outcome = BlockBreakState::new(&mut active, &mut delayed).stop(
        &stop(12, 5),
        HeldMiningTool {
            hotbar_slot: 0,
            stack: Some(&ItemStack::new(10, 2)),
        },
        44,
        0.1,
    );

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
fn second_early_stop_does_not_overwrite_existing_delayed_break() {
    let mut active = Some(pending(24, 50));
    let mut delayed = Some(pending(12, 40));
    let outcome = BlockBreakState::new(&mut active, &mut delayed).stop(
        &stop(24, 8),
        HeldMiningTool {
            hotbar_slot: 0,
            stack: Some(&ItemStack::new(10, 1)),
        },
        52,
        0.1,
    );

    assert_eq!(outcome, StopBreakOutcome::Acknowledge { delayed: false });
    assert_eq!(delayed.as_ref().map(|pending| pending.position), Some(12));
}

#[test]
fn delayed_break_completes_at_one_without_requesting_another_ack() {
    let mut active = None;
    let mut delayed = Some(pending(12, 40));
    let held = ItemStack::new(10, 3);
    let outcome = BlockBreakState::new(&mut active, &mut delayed).tick_delayed(
        HeldMiningTool {
            hotbar_slot: 0,
            stack: Some(&held),
        },
        49,
        0.1,
    );

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
    let held = ItemStack::new(10, 3);
    let outcome = BlockBreakState::new(&mut active, &mut delayed).tick_delayed(
        HeldMiningTool {
            hotbar_slot: 0,
            stack: Some(&held),
        },
        48,
        0.1,
    );

    assert_eq!(outcome, DelayedBreakOutcome::Pending);
    assert!(delayed.is_some());
}

#[test]
fn mismatched_stop_is_acknowledged_without_delaying_or_completing() {
    let mut active = Some(pending(12, 40));
    let mut delayed = None;
    let outcome = BlockBreakState::new(&mut active, &mut delayed).stop(
        &stop(24, 5),
        HeldMiningTool {
            hotbar_slot: 0,
            stack: Some(&ItemStack::new(10, 1)),
        },
        49,
        0.1,
    );

    assert_eq!(outcome, StopBreakOutcome::Acknowledge { delayed: false });
    assert!(active.is_none());
    assert!(delayed.is_none());
}

#[test]
fn delayed_break_cancels_when_tool_damage_changes() {
    let mut active = None;
    let mut delayed_pending = pending(12, 40);
    delayed_pending.held_item = Some(ItemStack::new(10, 1).with_damage(3));
    let mut delayed = Some(delayed_pending);
    let changed_tool = ItemStack::new(10, 1).with_damage(4);
    let outcome = BlockBreakState::new(&mut active, &mut delayed).tick_delayed(
        HeldMiningTool {
            hotbar_slot: 0,
            stack: Some(&changed_tool),
        },
        49,
        0.1,
    );

    assert_eq!(outcome, DelayedBreakOutcome::Cancelled);
    assert!(delayed.is_none());
}

#[test]
fn delayed_break_without_owner_snapshot_is_cancelled() {
    let mut active = None;
    let mut delayed_pending = pending(12, 40);
    delayed_pending.expected_target = None;
    let mut delayed = Some(delayed_pending);
    let held = ItemStack::new(10, 1);
    let outcome = BlockBreakState::new(&mut active, &mut delayed).tick_delayed(
        HeldMiningTool {
            hotbar_slot: 0,
            stack: Some(&held),
        },
        49,
        0.1,
    );

    assert_eq!(outcome, DelayedBreakOutcome::Cancelled);
    assert!(delayed.is_none());
}

#[test]
fn mining_tool_identity_ignores_count_only() {
    let expected = ItemStack::new(10, 1)
        .with_damage(3)
        .with_enchantment(Identifier::parse("minecraft:efficiency").unwrap(), 2);
    let same_tool_larger_stack = ItemStack::new(10, 64)
        .with_damage(3)
        .with_enchantment(Identifier::parse("minecraft:efficiency").unwrap(), 2);
    let different_item = ItemStack::new(11, 1)
        .with_damage(3)
        .with_enchantment(Identifier::parse("minecraft:efficiency").unwrap(), 2);
    let different_damage = ItemStack::new(10, 1)
        .with_damage(4)
        .with_enchantment(Identifier::parse("minecraft:efficiency").unwrap(), 2);
    let different_enchantment = ItemStack::new(10, 1)
        .with_damage(3)
        .with_enchantment(Identifier::parse("minecraft:efficiency").unwrap(), 3);

    assert!(mining_tool_matches(
        Some(&expected),
        Some(&same_tool_larger_stack)
    ));
    assert!(!mining_tool_matches(Some(&expected), Some(&different_item)));
    assert!(!mining_tool_matches(
        Some(&expected),
        Some(&different_damage)
    ));
    assert!(!mining_tool_matches(
        Some(&expected),
        Some(&different_enchantment)
    ));
}
