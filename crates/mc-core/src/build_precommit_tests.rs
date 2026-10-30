use std::num::NonZeroUsize;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::time::Instant;

use mc_script::precommit::{
    BuildEdit, HookFailurePolicy, HookRegistration, HookStorageVersionGuards, Request,
};
use mc_script::{
    ScriptHostEndpoint, ScriptHostInput, ScriptPluginStorageVersionGuard, ScriptStorageMutation,
    script_boundary_pair,
};

use super::*;
use crate::boundary::{EntityId, TickOutput};
use crate::crafter::tests::{block, stack};
use crate::journal::JournalEntry;
use crate::tick::run_tick;

const CONNECTION: ConnectionId = ConnectionId(1);
const POS: BlockPos = BlockPos { x: 8, y: 201, z: 8 };
const BASE: BlockPos = BlockPos { x: 8, y: 200, z: 8 };
const UPPER: BlockPos = BlockPos { x: 8, y: 202, z: 8 };
const SEQUENCE: i32 = 37;

fn fixture(directory: Option<std::path::PathBuf>) -> (TickState, EntityId) {
    // Chunk admission is synchronous native-owner work, not an async task.
    let (mut state, actor) =
        tokio::task::block_in_place(|| crate::crafter::tests::fixture(directory));
    let wanted = state.desired_chunks();
    tokio::task::block_in_place(|| state.world.ensure_blocking(&wanted));
    let teleport_id = state.players[&actor]
        .awaiting_teleport
        .expect("fixture join awaits the client's initial teleport confirmation");
    run_tick(
        &mut state,
        &[PlayerInput::ConfirmTeleport {
            connection: CONNECTION,
            teleport_id,
        }],
    );
    assert_eq!(state.players[&actor].awaiting_teleport, None);
    (state, actor)
}

fn hooked(state: &mut TickState) -> (Gateway, mc_script::ScriptBoundary, ScriptHostEndpoint) {
    let capacity = NonZeroUsize::new(8).unwrap();
    let (boundary, endpoint) = script_boundary_pair(capacity, capacity);
    boundary
        .set_precommit_hooks(vec![HookRegistration::new(
            "judge",
            HookKind::Build,
            0,
            HookFailurePolicy::Deny,
        )])
        .unwrap();
    let (gateway, queries) = Gateway::new(Some(boundary.clone()));
    assert!(queries.is_some());
    state.build_queries = queries;
    (gateway, boundary, endpoint)
}

fn held(state: &mut TickState, actor: EntityId, name: &str, count: i32) {
    let item = stack(state, name, count);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = item;
}

fn plant(state: &mut TickState, position: BlockPos, value: u32) {
    state
        .world
        .set_block(position.x, position.y, position.z, value);
    state.world.fold_overlay();
}

fn place() -> PlayerInput {
    use_on(BASE, Hand::Main, 1)
}

fn use_on(position: BlockPos, hand: Hand, face: u8) -> PlayerInput {
    PlayerInput::UseItemOn {
        connection: CONNECTION,
        hand,
        position,
        face,
        cursor: [0.5; 3],
        sequence: SEQUENCE,
    }
}

fn dig() -> PlayerInput {
    PlayerInput::Dig {
        connection: CONNECTION,
        position: POS,
        action: DigAction::Finish,
        sequence: SEQUENCE,
    }
}

// Drive only ready work. Each next poll is caused by the actual preview/answer
// producer; no sleeps, elapsed-time polling, or invented approval payloads.
fn pending<F: Future>(future: Pin<&mut F>) {
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(future.poll(&mut context), Poll::Pending));
}

fn take_request(endpoint: &mut ScriptHostEndpoint) -> Request {
    match endpoint.recv_input_blocking() {
        Some(ScriptHostInput::Precommit(request)) => request,
        _ => panic!("native preview must reach the real precommit FIFO"),
    }
}

async fn through_host(
    state: &mut TickState,
    gateway: &Gateway,
    endpoint: &mut ScriptHostEndpoint,
    input: PlayerInput,
    answer: impl FnOnce(Request),
) -> (Result<PlayerInput, BuildRefusal>, Vec<BuildEdit>) {
    if let PlayerInput::Dig {
        connection,
        position,
        action: DigAction::Finish,
        ..
    } = &input
    {
        crate::tick::mining_tests::ready_to_finish(state, *connection, *position);
    }
    let mut admission = std::pin::pin!(gateway.admit(input));
    pending(admission.as_mut());
    answer_previews(state);
    pending(admission.as_mut());
    let request = take_request(endpoint);
    assert_eq!(request.kind(), HookKind::Build);
    let HookContext::Build(context) = request.context() else {
        panic!("a native build must provide a build context");
    };
    assert_eq!(context.dimension(), "minecraft:overworld");
    let HookActor::Player(actor) = context.actor() else {
        panic!("a client build must identify its player");
    };
    assert_eq!(actor.session(), CONNECTION.0);
    assert_eq!(
        actor.uuid(),
        state.connections.uuid_of(CONNECTION).unwrap().to_string()
    );
    let edits = context.edits().to_vec();
    assert!(!edits.is_empty());
    answer(request);
    (admission.await, edits)
}

async fn keep(
    state: &mut TickState,
    gateway: &Gateway,
    endpoint: &mut ScriptHostEndpoint,
    input: PlayerInput,
) -> (PlayerInput, Vec<BuildEdit>) {
    let (result, edits) = through_host(state, gateway, endpoint, input, |request| {
        request.answer(HookDecision::Keep, Vec::new()).unwrap();
    })
    .await;
    let input = result.unwrap();
    assert!(matches!(input, PlayerInput::AuthorizedBuild { .. }));
    (input, edits)
}

fn changes_for_player(drains: Vec<(ConnectionId, Vec<PlayerChange>)>) -> Vec<PlayerChange> {
    drains
        .into_iter()
        .filter(|(connection, _)| *connection == CONNECTION)
        .flat_map(|(_, changes)| changes)
        .collect()
}

fn assert_ack(changes: &[PlayerChange]) {
    assert_eq!(
        changes
            .iter()
            .filter(|change| matches!(change,
                PlayerChange::BlockActionsAcked { sequence }
                    if *sequence == SEQUENCE
            ))
            .count(),
        1,
        "prediction is acknowledged exactly once"
    );
}

fn assert_repair(
    state: &TickState,
    actor: EntityId,
    changes: &[PlayerChange],
    positions: &[BlockPos],
) {
    assert_ack(changes);
    for position in positions {
        let expected = state
            .world
            .block_at(position.x, position.y, position.z)
            .unwrap();
        assert_eq!(
            changes
                .iter()
                .filter(|change| matches!(change,
                    PlayerChange::BlockResynced { position: at, block }
                        if at == position && *block == expected
                ))
                .count(),
            1,
            "each predicted cell receives one authoritative reliable correction"
        );
    }
    let contents = state.players[&actor].inventory_contents();
    assert!(
        changes.contains(&contents),
        "repair includes the exact authoritative inventory"
    );
}

fn has_block_journal(entries: &[JournalEntry]) -> bool {
    entries.iter().any(|entry| match entry {
        JournalEntry::Block { .. } => true,
        JournalEntry::Dimension { entries, .. } => has_block_journal(entries),
        _ => false,
    })
}

fn assert_no_build_journal(state: &mut TickState, output: &TickOutput) {
    assert!(
        output.changes.blocks.is_empty(),
        "repair is not a global block mutation"
    );
    let entries = crate::core::journal_entries_for_tick(state, output);
    assert!(
        !has_block_journal(&entries),
        "the real Core tick journal must not persist prediction repair"
    );
}

fn assert_committed(state: &mut TickState, output: &TickOutput, edits: &[BuildEdit]) {
    assert_eq!(output.changes.blocks.len(), edits.len());
    for edit in edits {
        let position = BlockPos {
            x: edit.x,
            y: edit.y,
            z: edit.z,
        };
        assert_eq!(
            state.world.block_at(edit.x, edit.y, edit.z),
            Some(edit.proposed_state)
        );
        assert_eq!(
            output
                .changes
                .blocks
                .iter()
                .filter(|change| change.position == position && change.state == edit.proposed_state)
                .count(),
            1,
            "the frozen native edit is committed once"
        );
    }
    let entries = crate::core::journal_entries_for_tick(state, output);
    let blocks: Vec<_> = entries
        .iter()
        .filter_map(|entry| match entry {
            JournalEntry::Block { pos, state } => Some((*pos, *state)),
            _ => None,
        })
        .collect();
    assert_eq!(blocks.len(), edits.len());
    for edit in edits {
        assert!(blocks.contains(&([edit.x, edit.y, edit.z], edit.proposed_state)));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn hook_free_gateway_is_the_unchanged_direct_native_path() {
    let (mut state, actor) = fixture(None);
    held(&mut state, actor, "dirt", 3);
    let capacity = NonZeroUsize::new(1).unwrap();
    let (boundary, _endpoint) = script_boundary_pair(capacity, capacity);
    boundary
        .try_enqueue_event(mc_script::ScriptEvent::server_tick(1))
        .unwrap();
    let (gateway, queries) = Gateway::new(Some(boundary));
    assert!(queries.is_none(), "no hooks allocate no preview channel");
    let input = place();
    let admitted = gateway.admit(input.clone()).await.unwrap();
    assert_eq!(
        admitted, input,
        "hook-free input is not wrapped or re-planned"
    );
    let (output, drains, _) = run_tick(&mut state, &[admitted]);
    assert_eq!(output.changes.blocks.len(), 1);
    assert_eq!(
        state.world.block_at(POS.x, POS.y, POS.z),
        Some(state.registry.states.dirt)
    );
    assert_eq!(state.players[&actor].inventory.slots[0].count, 2);
    assert_ack(&changes_for_player(drains));
    let (gateway, queries) = Gateway::new(None);
    assert!(queries.is_none());
    assert_eq!(
        gateway
            .admit(PlayerInput::SelectSlot {
                connection: CONNECTION,
                slot: 2
            })
            .await
            .unwrap(),
        PlayerInput::SelectSlot {
            connection: CONNECTION,
            slot: 2
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn actual_keep_break_and_placement_commit_the_native_plan_once() {
    for breaking in [false, true] {
        let (mut state, actor) = fixture(None);
        if breaking {
            let dirt = state.registry.states.dirt;
            plant(&mut state, POS, dirt);
        } else {
            held(&mut state, actor, "dirt", 3);
        }
        let before = state.world.block_at(POS.x, POS.y, POS.z).unwrap();
        let inventory = state.players[&actor].inventory.slots.clone();
        let (gateway, _boundary, mut endpoint) = hooked(&mut state);
        let (admitted, edits) = keep(
            &mut state,
            &gateway,
            &mut endpoint,
            if breaking { dig() } else { place() },
        )
        .await;
        assert_eq!(
            edits,
            vec![BuildEdit::new(
                POS.x,
                POS.y,
                POS.z,
                before,
                if breaking {
                    state.registry.states.air
                } else {
                    state.registry.states.dirt
                }
            )]
        );
        assert_eq!(state.world.block_at(POS.x, POS.y, POS.z), Some(before));
        assert_eq!(
            state.players[&actor].inventory.slots, inventory,
            "preview and hook cannot mutate native custody"
        );
        let replay = admitted.clone();
        let (output, drains, _) = run_tick(&mut state, &[admitted, replay.clone()]);
        assert_committed(&mut state, &output, &edits);
        assert_ack(&changes_for_player(drains));
        if breaking {
            assert_eq!(state.players[&actor].inventory.slots, inventory);
        } else {
            assert_eq!(state.players[&actor].inventory.slots[0].count, 2);
        }
        let inventory = state.players[&actor].inventory.slots.clone();
        let (output, drains, _) = run_tick(&mut state, &[replay]);
        assert_no_build_journal(&mut state, &output);
        assert_eq!(state.players[&actor].inventory.slots, inventory);
        assert!(!changes_for_player(drains).iter().any(|change| matches!(
            change,
            PlayerChange::BlockActionsAcked {
                sequence: SEQUENCE,
                ..
            }
        )));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn native_denied_placement_repairs_inventory_without_build_hooks() {
    for denied_cell in [
        BASE,
        POS,
        BlockPos {
            x: 80,
            y: 200,
            z: 8,
        },
    ] {
        let (mut state, actor) = fixture(None);
        held(&mut state, actor, "cobblestone", 8);
        let input = if denied_cell == BASE || denied_cell == POS {
            state.plugins.set_domain(
                "towns",
                "player-zones",
                serde_json::json!({
                    "claim": {
                        "dimension": "minecraft:overworld",
                        "minimum": [denied_cell.x, denied_cell.y, denied_cell.z],
                        "maximum": [denied_cell.x, denied_cell.y, denied_cell.z],
                        "allowed_actors": [uuid::Uuid::new_v4().simple().to_string()],
                        "public_use": false
                    }
                }),
            );
            crate::plugin_player::restore(&mut state).unwrap();
            assert!(!crate::plugin_player::allows_block(
                &state,
                actor,
                denied_cell
            ));
            place()
        } else {
            assert!(!crate::tick::within_block_reach(&state, actor, denied_cell));
            use_on(denied_cell, Hand::Main, 1)
        };
        let capacity = NonZeroUsize::new(8).unwrap();
        let (boundary, _endpoint) = script_boundary_pair(capacity, capacity);
        let (gateway, queries) = Gateway::new(Some(boundary));
        assert!(queries.is_none(), "towns zones do not register build hooks");
        let admitted = gateway.admit(input).await.unwrap();
        assert!(matches!(admitted, PlayerInput::UseItemOn { .. }));
        let inventory = state.players[&actor].inventory.slots.clone();
        let before = state.world.block_at(POS.x, POS.y, POS.z);
        let revision = state.world.chunk_revision(0, 0);
        let (output, drains, _) = run_tick(&mut state, &[admitted]);
        assert_eq!(state.players[&actor].inventory.slots[0].count, 8);
        assert_eq!(state.players[&actor].inventory.slots, inventory);
        assert_eq!(state.world.block_at(POS.x, POS.y, POS.z), before);
        assert_eq!(state.world.chunk_revision(0, 0), revision);
        let changes = changes_for_player(drains);
        assert!(
            changes.contains(&state.players[&actor].inventory_contents()),
            "server retains all 8 cobblestone and must repair the client's predicted debit"
        );
        assert_repair(
            &state,
            actor,
            &changes,
            if denied_cell == BASE || denied_cell == POS {
                &[BASE, POS]
            } else {
                &[]
            },
        );
        assert_no_build_journal(&mut state, &output);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn actual_deny_repairs_break_and_placement_without_debit_or_block_wal() {
    for breaking in [false, true] {
        let (mut state, actor) = fixture(None);
        held(&mut state, actor, "dirt", 3);
        if breaking {
            let dirt = state.registry.states.dirt;
            plant(&mut state, POS, dirt);
        }
        let before = state.world.block_at(POS.x, POS.y, POS.z);
        let inventory = state.players[&actor].inventory.slots.clone();
        let revision = state.world.chunk_revision(0, 0);
        let (gateway, _boundary, mut endpoint) = hooked(&mut state);
        let (result, _) = through_host(
            &mut state,
            &gateway,
            &mut endpoint,
            if breaking { dig() } else { place() },
            |request| {
                request.answer(HookDecision::Cancel, Vec::new()).unwrap();
            },
        )
        .await;
        let refusal = result.unwrap_err();
        assert_eq!(refusal.reason, HookFailure::Cancelled);
        let (output, drains, _) = run_tick(
            &mut state,
            &[PlayerInput::BuildResync {
                request: refusal.correction,
            }],
        );
        assert_eq!(state.world.block_at(POS.x, POS.y, POS.z), before);
        assert_eq!(state.world.chunk_revision(0, 0), revision);
        assert_eq!(state.players[&actor].inventory.slots, inventory);
        assert_repair(
            &state,
            actor,
            &changes_for_player(drains),
            if breaking { &[POS] } else { &[BASE, POS] },
        );
        assert_no_build_journal(&mut state, &output);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn host_failure_and_abandoned_answer_fail_closed_with_normal_repair() {
    for abandoned in [false, true] {
        let (mut state, actor) = fixture(None);
        held(&mut state, actor, "dirt", 3);
        let inventory = state.players[&actor].inventory.slots.clone();
        let (gateway, _boundary, mut endpoint) = hooked(&mut state);
        let (result, _) = through_host(&mut state, &gateway, &mut endpoint, place(), |request| {
            if abandoned {
                drop(request);
            } else {
                request.fail(HookFailure::Invalid).unwrap();
            }
        })
        .await;
        let refusal = result.unwrap_err();
        assert_eq!(
            refusal.reason,
            if abandoned {
                HookFailure::Unavailable
            } else {
                HookFailure::Invalid
            }
        );
        let (output, drains, _) = run_tick(
            &mut state,
            &[PlayerInput::BuildResync {
                request: refusal.correction,
            }],
        );
        assert_eq!(state.players[&actor].inventory.slots, inventory);
        assert_eq!(
            state.world.block_at(POS.x, POS.y, POS.z),
            Some(state.registry.states.air)
        );
        assert_repair(&state, actor, &changes_for_player(drains), &[BASE, POS]);
        assert_no_build_journal(&mut state, &output);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn ticks_continue_while_real_host_question_is_unanswered() {
    let (mut state, actor) = fixture(None);
    held(&mut state, actor, "dirt", 3);
    let (gateway, _boundary, mut endpoint) = hooked(&mut state);
    let mut admission = std::pin::pin!(gateway.admit(place()));
    pending(admission.as_mut());
    let previous_tick = state.tick;
    let (output, _, _) = run_tick(&mut state, &[]);
    assert!(
        output.tick > previous_tick,
        "native preview never awaits the host"
    );
    assert_no_build_journal(&mut state, &output);
    pending(admission.as_mut());
    let request = take_request(&mut endpoint);
    let previous_tick = state.tick;
    let (output, _, _) = run_tick(&mut state, &[]);
    assert!(
        output.tick > previous_tick,
        "a queued hook cannot hold the tick owner"
    );
    assert_no_build_journal(&mut state, &output);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 3);
    request.fail(HookFailure::Expired).unwrap();
    let refusal = admission.await.unwrap_err();
    assert_eq!(refusal.reason, HookFailure::Expired);
    let (output, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::BuildResync {
            request: refusal.correction,
        }],
    );
    assert_repair(&state, actor, &changes_for_player(drains), &[BASE, POS]);
    assert_no_build_journal(&mut state, &output);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn injected_expired_deadline_denies_without_waiting_or_touching_native_state() {
    let (mut state, actor) = fixture(None);
    held(&mut state, actor, "dirt", 3);
    let inventory = state.players[&actor].inventory.slots.clone();
    let (gateway, _boundary, _endpoint) = hooked(&mut state);
    let input = place();
    let correction = BuildResync::for_input(&input).unwrap();
    let deadline = Instant::now().checked_sub(PRECOMMIT_DEADLINE).unwrap();
    let refusal = gateway
        .admit_with_deadline(input, correction, deadline)
        .await
        .unwrap_err();
    assert_eq!(refusal.reason, HookFailure::Expired);
    let (output, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::BuildResync {
            request: refusal.correction,
        }],
    );
    assert_eq!(state.players[&actor].inventory.slots, inventory);
    assert_eq!(
        state.world.block_at(POS.x, POS.y, POS.z),
        Some(state.registry.states.air)
    );
    assert_repair(&state, actor, &changes_for_player(drains), &[BASE, POS]);
    assert_no_build_journal(&mut state, &output);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn stale_block_full_held_stack_player_mode_reach_and_occupancy_reject_at_commit() {
    for stale in [
        "block",
        "held-count",
        "held-components",
        "selected-slot",
        "mode",
        "reach",
        "occupancy",
    ] {
        let (mut state, actor) = fixture(None);
        held(&mut state, actor, "dirt", 3);
        let (gateway, _boundary, mut endpoint) = hooked(&mut state);
        let (admitted, _) = keep(&mut state, &gateway, &mut endpoint, place()).await;
        match stale {
            "block" => {
                let stone = state.registry.states.stone;
                plant(&mut state, POS, stone);
            }
            "selected-slot" => state.players.get_mut(&actor).unwrap().inventory.selected = 1,
            "held-count" => state.players.get_mut(&actor).unwrap().inventory.slots[0].count = 4,
            "held-components" => {
                state.players.get_mut(&actor).unwrap().inventory.slots[0].custom_name =
                    Some("changed after approval".into())
            }
            "mode" => {
                state.players.get_mut(&actor).unwrap().game_mode =
                    crate::commands::GameMode::Creative
            }
            "reach" => {
                let slot = state.entities.slot_of(actor).unwrap();
                state.entities.set_pos(slot, [14.5, 201.0, 14.5]);
            }
            "occupancy" => {
                let slot = state.entities.slot_of(actor).unwrap();
                state.entities.set_pos(slot, [8.5, 201.0, 8.5]);
            }
            _ => unreachable!(),
        }
        let before = state.world.block_at(POS.x, POS.y, POS.z);
        let inventory = state.players[&actor].inventory.slots.clone();
        let (output, drains, _) = run_tick(&mut state, &[admitted]);
        assert_eq!(state.world.block_at(POS.x, POS.y, POS.z), before, "{stale}");
        assert_eq!(state.players[&actor].inventory.slots, inventory, "{stale}");
        assert_repair(&state, actor, &changes_for_player(drains), &[BASE, POS]);
        assert_no_build_journal(&mut state, &output);
    }
}

fn set_storage(state: &mut TickState, value: &str) -> u64 {
    let expected = state.plugins.version("judge", "claim");
    let mutation = ScriptStorageMutation::compare_and_swap("claim", expected, value).unwrap();
    state.plugins.apply_storage("judge", &[mutation]);
    state.plugins.version("judge", "claim").unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn native_plugin_kv_guard_is_rechecked_in_the_commit_turn() {
    for stale in [false, true] {
        let (mut state, actor) = fixture(None);
        held(&mut state, actor, "dirt", 3);
        let version = set_storage(&mut state, "owner-a");
        let (gateway, _boundary, mut endpoint) = hooked(&mut state);
        let (result, edits) =
            through_host(&mut state, &gateway, &mut endpoint, place(), |request| {
                request
                    .answer(
                        HookDecision::Keep,
                        vec![HookStorageVersionGuards::new(
                            "judge".into(),
                            vec![ScriptPluginStorageVersionGuard::new(
                                "claim".into(),
                                version,
                            )],
                        )],
                    )
                    .unwrap();
            })
            .await;
        // A timely host answer may spend later native ticks waiting to merge.
        // Its KV guard must still be checked in that eventual owner turn.
        for _ in 0..2 {
            let (output, _, _) = run_tick(&mut state, &[]);
            assert_no_build_journal(&mut state, &output);
        }
        if stale {
            set_storage(&mut state, "owner-b");
        }
        let storage = state
            .plugins
            .storage_value_version("judge", "claim")
            .map(|(value, version)| (value.map(str::to_owned), version));
        let inventory = state.players[&actor].inventory.slots.clone();
        let (output, drains, _) = run_tick(&mut state, &[result.unwrap()]);
        assert_eq!(
            state
                .plugins
                .storage_value_version("judge", "claim")
                .map(|(value, version)| (value.map(str::to_owned), version)),
            storage,
            "build approval may inspect KV, never mutate it"
        );
        if stale {
            assert_eq!(state.players[&actor].inventory.slots, inventory);
            assert_repair(&state, actor, &changes_for_player(drains), &[BASE, POS]);
            assert_no_build_journal(&mut state, &output);
        } else {
            assert_committed(&mut state, &output, &edits);
            assert_eq!(state.players[&actor].inventory.slots[0].count, 2);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn changed_hook_roster_invalidates_an_already_kept_native_build() {
    let (mut state, actor) = fixture(None);
    held(&mut state, actor, "dirt", 3);
    let (gateway, boundary, mut endpoint) = hooked(&mut state);
    let (admitted, _) = keep(&mut state, &gateway, &mut endpoint, place()).await;
    boundary.set_precommit_hooks(Vec::new()).unwrap();
    let (output, drains, _) = run_tick(&mut state, &[admitted]);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 3);
    assert_eq!(
        state.world.block_at(POS.x, POS.y, POS.z),
        Some(state.registry.states.air)
    );
    assert_repair(&state, actor, &changes_for_player(drains), &[BASE, POS]);
    assert_no_build_journal(&mut state, &output);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn real_two_cell_door_placement_and_break_report_the_whole_native_batch() {
    let (mut state, actor) = fixture(None);
    held(&mut state, actor, "oak_door", 3);
    let (gateway, _boundary, mut endpoint) = hooked(&mut state);
    let (admitted, edits) = keep(&mut state, &gateway, &mut endpoint, place()).await;
    assert_eq!(edits.len(), 2);
    for position in [POS, UPPER] {
        assert!(edits.iter().any(|edit| [edit.x, edit.y, edit.z]
            == [position.x, position.y, position.z]
            && edit.previous_state == state.registry.states.air));
    }
    let (output, drains, _) = run_tick(&mut state, &[admitted]);
    assert_committed(&mut state, &output, &edits);
    assert_ack(&changes_for_player(drains));
    assert_eq!(
        state.players[&actor].inventory.slots[0].count, 2,
        "one door item pays for both cells"
    );
    let (admitted, edits) = keep(&mut state, &gateway, &mut endpoint, dig()).await;
    assert_eq!(edits.len(), 2);
    assert!(
        edits
            .iter()
            .all(|edit| edit.proposed_state == state.registry.states.air)
    );
    let (output, drains, _) = run_tick(&mut state, &[admitted]);
    assert_committed(&mut state, &output, &edits);
    assert_ack(&changes_for_player(drains));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn spawn_protected_door_use_repairs_both_halves_before_ack() {
    let (mut state, actor) = fixture(None);
    state.spawn_protection = 256;
    state.spawn_operators.insert(uuid::Uuid::new_v4());
    let lower = block(&state, "oak_door", &[("half", "lower"), ("open", "false")]);
    let upper = block(&state, "oak_door", &[("half", "upper"), ("open", "false")]);
    plant(&mut state, POS, lower);
    plant(&mut state, UPPER, upper);
    assert!(crate::spawn_protection::protected(&state, actor, POS));
    let (_, drains, _) = run_tick(&mut state, &[use_on(POS, Hand::Main, 3)]);
    let changes = changes_for_player(drains);
    let ack = changes.iter().position(|change| matches!(change,
        PlayerChange::BlockActionsAcked { sequence } if *sequence == SEQUENCE
    )).expect("refused use must be acknowledged");
    for (position, expected) in [(POS, lower), (UPPER, upper)] {
        let repair = changes.iter().position(|change| matches!(change,
            PlayerChange::BlockResynced { position: cell, block } if *cell == position && *block == expected
        )).expect("both door cells must be repaired");
        assert!(repair < ack, "repair must precede its acknowledgement");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn denied_two_cell_door_repairs_even_the_unwritten_upper_prediction() {
    let (mut state, actor) = fixture(None);
    held(&mut state, actor, "oak_door", 3);
    let (gateway, _boundary, mut endpoint) = hooked(&mut state);
    let (result, edits) = through_host(&mut state, &gateway, &mut endpoint, place(), |request| {
        request.answer(HookDecision::Cancel, Vec::new()).unwrap();
    })
    .await;
    assert_eq!(edits.len(), 2);
    let refusal = result.unwrap_err();
    let (output, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::BuildResync {
            request: refusal.correction,
        }],
    );
    assert_repair(
        &state,
        actor,
        &changes_for_player(drains),
        &[BASE, POS, UPPER],
    );
    assert_eq!(state.players[&actor].inventory.slots[0].count, 3);
    for position in [POS, UPPER] {
        assert_eq!(
            state.world.block_at(position.x, position.y, position.z),
            Some(state.registry.states.air)
        );
    }
    assert_no_build_journal(&mut state, &output);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn hook_free_contended_door_keeps_winner_and_acknowledges_rejection() {
    let (mut state, first) = fixture(None);
    let second_connection = ConnectionId(2);
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: second_connection,
            uuid: uuid::Uuid::new_v4(),
            name: "door-builder".into(),
            permission_level: 0,
        }],
    );
    let second = state.connections.entity_of(second_connection).unwrap();
    let slot = state.entities.slot_of(second).unwrap();
    state.entities.set_pos(slot, [10.5, 201.0, 8.5]);
    held(&mut state, first, "oak_planks", 2);
    held(&mut state, second, "oak_door", 3);
    let door = PlayerInput::UseItemOn {
        connection: second_connection,
        hand: Hand::Main,
        position: BASE,
        face: 1,
        cursor: [0.5; 3],
        sequence: SEQUENCE,
    };
    assert!(crate::tick::plan_client_build(&state, first, &place()).is_some());
    assert_eq!(
        crate::tick::plan_client_build(&state, second, &door)
            .unwrap()
            .footprint
            .block_write
            .len(),
        2
    );
    let (output, drains, _) = run_tick(&mut state, &[place(), door]);
    let planks = block(&state, "oak_planks", &[]);
    assert_eq!(state.world.block_at(POS.x, POS.y, POS.z), Some(planks));
    assert_eq!(
        state.world.block_at(UPPER.x, UPPER.y, UPPER.z),
        Some(state.registry.states.air)
    );
    assert_eq!(state.players[&first].inventory.slots[0].count, 1);
    assert_eq!(state.players[&second].inventory.slots[0].count, 3);
    assert_eq!(
        output.changes.blocks.len(),
        1,
        "only the winner changes the world"
    );
    let changes: Vec<_> = drains
        .into_iter()
        .filter(|(connection, _)| *connection == second_connection)
        .flat_map(|(_, changes)| changes)
        .collect();
    // The acknowledgement rolls both client-predicted door cells back; only
    // the winning plank placement belongs in the world change stream.
    assert_ack(&changes);
    assert!(changes.contains(&state.players[&second].inventory_contents()));
}

async fn direct_interaction(
    state: &mut TickState,
    gateway: &Gateway,
    input: PlayerInput,
) -> PlayerInput {
    let mut admission = std::pin::pin!(gateway.admit(input.clone()));
    pending(admission.as_mut());
    answer_previews(state);
    let admitted = admission.await.unwrap();
    assert_eq!(
        admitted, input,
        "an actual native non-build bypasses the registered judge"
    );
    admitted
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn container_sign_shelf_jukebox_and_campfire_interactions_are_not_builds() {
    for kind in ["chest", "oak_sign", "oak_shelf", "jukebox", "campfire"] {
        let (mut state, actor) = fixture(None);
        let properties: &[(&str, &str)] = match kind {
            "oak_shelf" => &[("facing", "south")],
            "campfire" => &[("lit", "true")],
            _ => &[],
        };
        let id = block(&state, kind, properties);
        plant(&mut state, POS, id);
        // Dirt is itself buildable: the non-build intercept must win instead of
        // obtaining an approval for a placement which native input would not do.
        held(
            &mut state,
            actor,
            if kind == "campfire" {
                "chicken"
            } else {
                "dirt"
            },
            3,
        );
        let (gateway, boundary, _endpoint) = hooked(&mut state);
        let input = use_on(POS, Hand::Main, if kind == "oak_shelf" { 3 } else { 1 });
        let admitted = direct_interaction(&mut state, &gateway, input).await;
        let (output, drains, _) = run_tick(&mut state, &[admitted]);
        assert_eq!(
            state.world.block_at(POS.x, POS.y, POS.z),
            Some(id),
            "{kind}"
        );
        assert_eq!(
            state.world.block_at(UPPER.x, UPPER.y, UPPER.z),
            Some(state.registry.states.air),
            "{kind}"
        );
        assert_no_build_journal(&mut state, &output);
        let changes = changes_for_player(drains);
        match kind {
            "chest" => assert!(
                changes
                    .iter()
                    .any(|change| matches!(change, PlayerChange::MenuOpened { .. }))
            ),
            "oak_sign" => assert_eq!(state.sign_editors[&CONNECTION].position, POS),
            "oak_shelf" => {
                assert!(state.players[&actor].inventory.slots[0].is_empty());
                assert_eq!(
                    state.block_entities[&(POS.x, POS.y, POS.z)].slots[1].count,
                    3
                );
            }
            "campfire" => assert_eq!(state.players[&actor].inventory.slots[0].count, 2),
            "jukebox" => assert_eq!(state.players[&actor].inventory.slots[0].count, 3),
            _ => unreachable!(),
        }
        // An empty precommit FIFO accepts a full-capacity event burst. This
        // catches a ghost hook question even if a direct result is returned.
        for tick in 0..8 {
            boundary
                .try_enqueue_event(mc_script::ScriptEvent::server_tick(tick))
                .unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn ordinary_block_toggle_is_direct_even_while_holding_a_buildable_item() {
    let (mut state, actor) = fixture(None);
    let closed = block(&state, "oak_fence_gate", &[("open", "false")]);
    plant(&mut state, POS, closed);
    held(&mut state, actor, "dirt", 3);
    let (gateway, _boundary, _endpoint) = hooked(&mut state);
    let admitted = direct_interaction(&mut state, &gateway, use_on(POS, Hand::Main, 1)).await;
    let (output, _, _) = run_tick(&mut state, &[admitted]);
    let current = state.world.block_at(POS.x, POS.y, POS.z).unwrap();
    assert_ne!(current, closed);
    assert!(
        state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(current))
            .unwrap()
            .properties
            .iter()
            .any(|(key, value)| key == "open" && value == "true")
    );
    assert_eq!(
        output.changes.blocks.len(),
        1,
        "the ordinary interaction still makes its real native edit"
    );
    assert_eq!(state.players[&actor].inventory.slots[0].count, 3);
    assert_eq!(
        state.world.block_at(UPPER.x, UPPER.y, UPPER.z),
        Some(state.registry.states.air)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn nonfood_campfire_and_wrong_shelf_face_still_reach_real_build_admission() {
    for kind in ["campfire", "oak_shelf"] {
        let (mut state, actor) = fixture(None);
        let id = block(
            &state,
            kind,
            if kind == "oak_shelf" {
                &[("facing", "south")]
            } else {
                &[]
            },
        );
        plant(&mut state, POS, id);
        held(&mut state, actor, "dirt", 3);
        let (gateway, _boundary, mut endpoint) = hooked(&mut state);
        let (admitted, edits) = keep(
            &mut state,
            &gateway,
            &mut endpoint,
            use_on(POS, Hand::Main, 1),
        )
        .await;
        assert_eq!(
            edits,
            vec![BuildEdit::new(
                UPPER.x,
                UPPER.y,
                UPPER.z,
                state.registry.states.air,
                state.registry.states.dirt
            )]
        );
        let (output, _, _) = run_tick(&mut state, &[admitted]);
        assert_committed(&mut state, &output, &edits);
        assert_eq!(state.players[&actor].inventory.slots[0].count, 2);
        assert_eq!(state.world.block_at(POS.x, POS.y, POS.z), Some(id));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn saturated_preview_queue_fails_closed_and_abandoned_previews_do_no_work() {
    let (mut state, actor) = fixture(None);
    held(&mut state, actor, "dirt", 3);
    let (gateway, boundary, _endpoint) = hooked(&mut state);
    let mut waiting: Vec<_> = (0..MAX_PRECOMMIT_REQUESTS)
        .map(|_| Box::pin(gateway.admit(place())))
        .collect();
    for admission in &mut waiting {
        pending(admission.as_mut());
    }
    let refusal = gateway.admit(place()).await.unwrap_err();
    assert_eq!(refusal.reason, HookFailure::QueueFull);
    drop(waiting);
    let (output, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::BuildResync {
            request: refusal.correction,
        }],
    );
    assert_repair(&state, actor, &changes_for_player(drains), &[BASE, POS]);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 3);
    assert_no_build_journal(&mut state, &output);
    for tick in 0..8 {
        boundary
            .try_enqueue_event(mc_script::ScriptEvent::server_tick(tick))
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn authorization_from_a_departed_player_cannot_build_for_a_reused_connection() {
    let (mut state, actor) = fixture(None);
    held(&mut state, actor, "dirt", 3);
    let (gateway, _boundary, mut endpoint) = hooked(&mut state);
    let (admitted, _) = keep(&mut state, &gateway, &mut endpoint, place()).await;
    run_tick(
        &mut state,
        &[PlayerInput::Leave {
            connection: CONNECTION,
        }],
    );
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: CONNECTION,
            uuid: uuid::Uuid::new_v4(),
            name: "replacement".into(),
            permission_level: 0,
        }],
    );
    let replacement = state.connections.entity_of(CONNECTION).unwrap();
    assert_ne!(replacement, actor);
    let inventory = state.players[&replacement].inventory.slots.clone();
    let (output, drains, _) = run_tick(&mut state, &[admitted]);
    assert_eq!(state.players[&replacement].inventory.slots, inventory);
    assert_eq!(
        state.world.block_at(POS.x, POS.y, POS.z),
        Some(state.registry.states.air)
    );
    assert_repair(
        &state,
        replacement,
        &changes_for_player(drains),
        &[BASE, POS],
    );
    assert_no_build_journal(&mut state, &output);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn sneaking_offhand_and_other_editor_sign_fallthroughs_remain_real_builds() {
    for fallback in ["sneaking", "offhand", "other-editor"] {
        let (mut state, actor) = fixture(None);
        let sign = block(&state, "oak_sign", &[]);
        plant(&mut state, POS, sign);
        held(&mut state, actor, "dirt", 3);
        let hand = if fallback == "offhand" {
            let item = stack(&state, "dirt", 3);
            let inventory = &mut state.players.get_mut(&actor).unwrap().inventory;
            inventory.slots[0] = mc_data::ItemStack::EMPTY;
            inventory.slots[crate::inventory::OFF_HAND] = item;
            Hand::Off
        } else {
            Hand::Main
        };
        if fallback == "sneaking" {
            run_tick(
                &mut state,
                &[PlayerInput::Sneak {
                    connection: CONNECTION,
                    sneaking: true,
                }],
            );
        } else if fallback == "other-editor" {
            run_tick(
                &mut state,
                &[PlayerInput::Join {
                    connection: ConnectionId(2),
                    uuid: uuid::Uuid::new_v4(),
                    name: "editor".into(),
                    permission_level: 0,
                }],
            );
            let editor = state.connections.entity_of(ConnectionId(2)).unwrap();
            let editor_slot = state.entities.slot_of(editor).unwrap();
            state.entities.set_pos(editor_slot, [8.5, 201.0, 10.5]);
            state.entities.set_ground(editor_slot, true);
            run_tick(
                &mut state,
                &[PlayerInput::UseItemOn {
                    connection: ConnectionId(2),
                    hand: Hand::Main,
                    position: POS,
                    face: 1,
                    cursor: [0.5; 3],
                    sequence: 2,
                }],
            );
            assert_eq!(state.sign_editors[&ConnectionId(2)].position, POS);
        }
        let (gateway, _boundary, mut endpoint) = hooked(&mut state);
        let (admitted, edits) =
            keep(&mut state, &gateway, &mut endpoint, use_on(POS, hand, 1)).await;
        assert_eq!(
            edits,
            vec![BuildEdit::new(
                UPPER.x,
                UPPER.y,
                UPPER.z,
                state.registry.states.air,
                state.registry.states.dirt
            )],
            "{fallback}"
        );
        let (output, _, _) = run_tick(&mut state, &[admitted]);
        assert_committed(&mut state, &output, &edits);
        let slot = crate::tick::held_slot_index(&state.players[&actor], hand);
        assert_eq!(
            state.players[&actor].inventory.slots[slot].count, 2,
            "{fallback}"
        );
        assert_eq!(state.world.block_at(POS.x, POS.y, POS.z), Some(sign));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn dig_authorization_rejects_changed_selected_slot_even_with_identical_stacks() {
    let (mut state, actor) = fixture(None);
    let dirt = state.registry.states.dirt;
    plant(&mut state, POS, dirt);
    held(&mut state, actor, "diamond_pickaxe", 1);
    let original = state.players[&actor].inventory.slots[0].clone();
    state.players.get_mut(&actor).unwrap().inventory.slots[1] = original;
    let (gateway, _boundary, mut endpoint) = hooked(&mut state);
    let (admitted, _) = keep(&mut state, &gateway, &mut endpoint, dig()).await;
    state.players.get_mut(&actor).unwrap().inventory.selected = 1;
    assert_eq!(
        state.players[&actor].inventory.slots[0],
        state.players[&actor].inventory.slots[1]
    );
    let inventory = state.players[&actor].inventory.slots.clone();
    let (output, drains, _) = run_tick(&mut state, &[admitted]);
    assert_eq!(state.world.block_at(POS.x, POS.y, POS.z), Some(dirt));
    assert_eq!(state.players[&actor].inventory.slots, inventory);
    assert_repair(&state, actor, &changes_for_player(drains), &[POS]);
    assert_no_build_journal(&mut state, &output);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn child_dimension_preview_uses_and_restores_global_protection_and_kv_authority() {
    use crate::boundary::ChunkPos;
    use crate::dimensions::Dimension;
    use crate::world::ChunkWorld;
    use std::sync::Arc;

    for protected in [true, false] {
        let (mut state, actor) = fixture(None);
        held(&mut state, actor, "dirt", 3);
        let uuid = state.connections.uuid_of(CONNECTION).unwrap();
        let registry = Arc::clone(&state.registry);
        let generator =
            Arc::new(mc_worldgen::EndGenerator::try_new(42, Arc::clone(&registry.blocks)).unwrap());
        let mut world = ChunkWorld::new(
            42,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [8, 201, 10],
            Dimension::End.geometry(),
        );
        let chunk = ChunkPos { x: 0, z: 0 };
        tokio::task::block_in_place(|| world.ensure_blocking(&[(chunk, 0)].into_iter().collect()));
        for x in 0..16 {
            for z in 0..16 {
                world.set_block(x, 200, z, registry.states.stone);
                for y in 201..206 {
                    world.set_block(x, y, z, registry.states.air);
                }
            }
        }
        world.fold_overlay();
        let mut child = TickState::new(registry, world, 0, 512);
        child.dimension = Dimension::End;
        child
            .tickets
            .insert("build-preview-test".into(), (chunk, 0));
        state
            .other_dimensions
            .insert(Dimension::End, Box::new(child));
        run_tick(
            &mut state,
            &[PlayerInput::Command {
                connection: CONNECTION,
                command: "execute in the_end run tp @s 8.5 201 10.5".into(),
                permission_level: 2,
            }],
        );
        run_tick(
            &mut state,
            &[PlayerInput::PermissionLevel {
                connection: CONNECTION,
                level: 0,
            }],
        );
        assert!(!state.players.contains_key(&actor));
        assert!(
            state.other_dimensions[&Dimension::End]
                .players
                .contains_key(&actor)
        );
        let owner = if protected {
            uuid::Uuid::new_v4()
        } else {
            uuid
        };
        // This is the real persisted native zone format, restored through the
        // same owner that restores checkpoint/WAL protection, not a guest cache.
        state.plugins.set_domain(
            "towns",
            "player-zones",
            serde_json::json!({
                "claim": {
                    "dimension": "minecraft:the_end",
                    "minimum": [8.0, 200.0, 8.0],
                    "maximum": [8.0, 203.0, 8.0],
                    "allowed_actors": [owner.simple().to_string()],
                    "public_use": false
                }
            }),
        );
        crate::plugin_player::restore(&mut state).unwrap();
        let version = set_storage(&mut state, "global-end-claim");
        crate::plugin::journal(&mut state, &mut Vec::new());
        let global_plugins = state.plugins.clone();
        let local_plugins = state.other_dimensions[&Dimension::End].plugins.clone();
        let (gateway, boundary, mut endpoint) = hooked(&mut state);
        let mut admission = std::pin::pin!(gateway.admit(place()));
        pending(admission.as_mut());
        answer_previews(&mut state);
        assert_eq!(
            state.plugins, global_plugins,
            "preview returns global ownership intact"
        );
        assert_eq!(
            state.other_dimensions[&Dimension::End].plugins,
            local_plugins,
            "preview must not strand global custody in the child"
        );
        assert_eq!(
            state.other_dimensions[&Dimension::End].players[&actor]
                .inventory
                .slots[0]
                .count,
            3
        );
        if protected {
            let refusal = admission.await.unwrap_err();
            assert_eq!(refusal.reason, HookFailure::Cancelled);
            let (output, drains, _) = run_tick(
                &mut state,
                &[PlayerInput::BuildResync {
                    request: refusal.correction,
                }],
            );
            let child = &state.other_dimensions[&Dimension::End];
            assert_eq!(
                child.world.block_at(POS.x, POS.y, POS.z),
                Some(child.registry.states.air)
            );
            assert_eq!(child.players[&actor].inventory.slots[0].count, 3);
            assert_repair(child, actor, &changes_for_player(drains), &[BASE, POS]);
            assert_no_build_journal(&mut state, &output);
            for tick in 0..8 {
                boundary
                    .try_enqueue_event(mc_script::ScriptEvent::server_tick(tick))
                    .unwrap();
            }
        } else {
            pending(admission.as_mut());
            let request = take_request(&mut endpoint);
            let HookContext::Build(context) = request.context() else {
                panic!("child build must carry its native build context");
            };
            assert_eq!(context.dimension(), "minecraft:the_end");
            assert_eq!(
                context.edits(),
                &[BuildEdit::new(
                    POS.x,
                    POS.y,
                    POS.z,
                    state.registry.states.air,
                    state.registry.states.dirt
                )]
            );
            request
                .answer(
                    HookDecision::Keep,
                    vec![HookStorageVersionGuards::new(
                        "judge".into(),
                        vec![ScriptPluginStorageVersionGuard::new(
                            "claim".into(),
                            version,
                        )],
                    )],
                )
                .unwrap();
            let admitted = admission.await.unwrap();
            assert!(matches!(admitted, PlayerInput::AuthorizedBuild { .. }));
            let (_, drains, _) = run_tick(&mut state, &[admitted]);
            assert_ack(&changes_for_player(drains));
            let child = &state.other_dimensions[&Dimension::End];
            assert_eq!(
                child.world.block_at(POS.x, POS.y, POS.z),
                Some(child.registry.states.dirt)
            );
            assert_eq!(child.players[&actor].inventory.slots[0].count, 2);
            assert_eq!(
                state.world.block_at(POS.x, POS.y, POS.z),
                Some(state.registry.states.air)
            );
        }
        assert_eq!(
            state.plugins, global_plugins,
            "neither refusal nor commit mutates global plugin storage"
        );
        assert_eq!(
            state.other_dimensions[&Dimension::End].plugins,
            local_plugins
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn bone_meal_keep_freezes_growth_roll_across_ticks_and_deny_preserves_crop_and_custody() {
    for decision in [HookDecision::Keep, HookDecision::Cancel] {
        let (mut state, actor) = fixture(None);
        state
            .gamerules
            .insert("random_tick_speed".into(), "0".into());
        let farmland = block(&state, "farmland", &[("moisture", "7")]);
        let wheat = block(&state, "wheat", &[("age", "0")]);
        plant(&mut state, BASE, farmland);
        plant(&mut state, POS, wheat);
        held(&mut state, actor, "bone_meal", 3);
        let input = use_on(POS, Hand::Main, 1);
        // Deliberately choose a native simulation tick whose successor would
        // make a different growth roll. An accidental re-roll cannot pass.
        let planning_tick = (1..128)
            .find(|tick| {
                let first =
                    crate::tick::plan_client_build_at(&state, actor, &input, *tick, None).unwrap();
                let next = crate::tick::plan_client_build_at(&state, actor, &input, tick + 1, None)
                    .unwrap();
                first.footprint.block_write != next.footprint.block_write
            })
            .expect("native crop growth has different adjacent-tick rolls");
        state.tick = planning_tick;
        let frozen = crate::tick::plan_client_build(&state, actor, &input).unwrap();
        let inventory = state.players[&actor].inventory.slots.clone();
        let (gateway, _boundary, mut endpoint) = hooked(&mut state);
        let (result, edits) = through_host(
            &mut state,
            &gateway,
            &mut endpoint,
            input.clone(),
            |request| {
                request.answer(decision, Vec::new()).unwrap();
            },
        )
        .await;
        assert_eq!(
            edits,
            vec![BuildEdit::new(
                POS.x,
                POS.y,
                POS.z,
                wheat,
                frozen.footprint.block_write[0].1
            )]
        );
        let (idle, _, _) = run_tick(&mut state, &[]);
        assert_no_build_journal(&mut state, &idle);
        assert_eq!(state.tick, planning_tick + 1);
        let rerolled = crate::tick::plan_client_build(&state, actor, &input).unwrap();
        assert_ne!(rerolled.footprint.block_write, frozen.footprint.block_write);
        let submitted = match result {
            Ok(admitted) => admitted,
            Err(refusal) => {
                assert_eq!(decision, HookDecision::Cancel);
                assert_eq!(refusal.reason, HookFailure::Cancelled);
                PlayerInput::BuildResync {
                    request: refusal.correction,
                }
            }
        };
        let (output, drains, _) = run_tick(&mut state, &[submitted]);
        let changes = changes_for_player(drains);
        if decision == HookDecision::Keep {
            assert_committed(&mut state, &output, &edits);
            assert_ack(&changes);
            assert_eq!(state.players[&actor].inventory.slots[0].count, 2);
        } else {
            assert_eq!(state.world.block_at(POS.x, POS.y, POS.z), Some(wheat));
            assert_eq!(state.players[&actor].inventory.slots, inventory);
            assert_repair(&state, actor, &changes, &[POS, UPPER]);
            assert_no_build_journal(&mut state, &output);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn real_chance_based_composter_fill_keep_and_deny_preserve_native_roll_and_debit() {
    for decision in [HookDecision::Keep, HookDecision::Cancel] {
        let (mut state, actor) = fixture(None);
        let initial = block(&state, "composter", &[("level", "1")]);
        let filled = block(&state, "composter", &[("level", "2")]);
        plant(&mut state, POS, initial);
        held(&mut state, actor, "wheat_seeds", 3);
        let sequence = (0..1000)
            .find(|sequence| {
                matches!(
                    crate::composter::plan_use(&state, actor, Hand::Main, POS, *sequence),
                    crate::composter::UsePlan::Fill { current, next, .. } if current != next
                )
            })
            .expect("a successful native 30-percent compost roll");
        let input = PlayerInput::UseItemOn {
            connection: CONNECTION,
            hand: Hand::Main,
            position: POS,
            face: 1,
            cursor: [0.5; 3],
            sequence,
        };
        let frozen = crate::tick::plan_client_build(&state, actor, &input).unwrap();
        let inventory = state.players[&actor].inventory.slots.clone();
        let scheduled = state.block_ticks.image();
        let (gateway, _boundary, mut endpoint) = hooked(&mut state);
        let (result, edits) = through_host(
            &mut state,
            &gateway,
            &mut endpoint,
            input.clone(),
            |request| {
                request.answer(decision, Vec::new()).unwrap();
            },
        )
        .await;
        assert_eq!(
            edits,
            vec![BuildEdit::new(POS.x, POS.y, POS.z, initial, filled)]
        );
        let (idle, _, _) = run_tick(&mut state, &[]);
        assert_no_build_journal(&mut state, &idle);
        assert_eq!(
            crate::tick::plan_client_build(&state, actor, &input)
                .unwrap()
                .footprint,
            frozen.footprint,
            "compost chance depends on cell and wire sequence, not tick"
        );
        let submitted = match result {
            Ok(admitted) => admitted,
            Err(refusal) => {
                assert_eq!(refusal.reason, HookFailure::Cancelled);
                PlayerInput::BuildResync {
                    request: refusal.correction,
                }
            }
        };
        let (output, drains, _) = run_tick(&mut state, &[submitted]);
        let changes = changes_for_player(drains);
        assert_eq!(
            changes
                .iter()
                .filter(|change| matches!(change,
                    PlayerChange::BlockActionsAcked { sequence: ack }
                        if *ack == sequence
                ))
                .count(),
            1
        );
        if decision == HookDecision::Keep {
            assert_committed(&mut state, &output, &edits);
            assert_eq!(state.players[&actor].inventory.slots[0].count, 2);
        } else {
            assert_eq!(state.world.block_at(POS.x, POS.y, POS.z), Some(initial));
            assert_eq!(state.players[&actor].inventory.slots, inventory);
            assert_eq!(state.block_ticks.image(), scheduled);
            assert!(changes.contains(&state.players[&actor].inventory_contents()));
            assert!(changes.iter().any(|change| matches!(change,
                PlayerChange::BlockResynced { position, block }
                    if *position == POS && *block == initial
            )));
            assert_no_build_journal(&mut state, &output);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn clicked_bucket_pickup_and_pour_keep_and_deny_use_real_native_exchange() {
    for pickup in [false, true] {
        for decision in [HookDecision::Keep, HookDecision::Cancel] {
            let (mut state, actor) = fixture(None);
            let source = state.registry.states.water_levels[0];
            let air = state.registry.states.air;
            if pickup {
                plant(&mut state, POS, source);
                held(&mut state, actor, "bucket", 1);
            } else {
                held(&mut state, actor, "water_bucket", 1);
            }
            let inventory = state.players[&actor].inventory.slots.clone();
            let fluids = state.fluid_ticks.image();
            let input = if pickup {
                use_on(POS, Hand::Main, 1)
            } else {
                place()
            };
            let (gateway, _boundary, mut endpoint) = hooked(&mut state);
            let (result, edits) =
                through_host(&mut state, &gateway, &mut endpoint, input, |request| {
                    request.answer(decision, Vec::new()).unwrap();
                })
                .await;
            assert_eq!(
                edits,
                vec![BuildEdit::new(
                    POS.x,
                    POS.y,
                    POS.z,
                    if pickup { source } else { air },
                    if pickup { air } else { source }
                )]
            );
            let submitted = match result {
                Ok(admitted) => admitted,
                Err(refusal) => {
                    assert_eq!(refusal.reason, HookFailure::Cancelled);
                    PlayerInput::BuildResync {
                        request: refusal.correction,
                    }
                }
            };
            let (output, drains, _) = run_tick(&mut state, &[submitted]);
            let changes = changes_for_player(drains);
            if decision == HookDecision::Keep {
                assert_committed(&mut state, &output, &edits);
                assert_ack(&changes);
                assert_eq!(
                    state.players[&actor].inventory.slots[0],
                    stack(&state, if pickup { "water_bucket" } else { "bucket" }, 1)
                );
            } else {
                assert_eq!(
                    state.world.block_at(POS.x, POS.y, POS.z),
                    Some(if pickup { source } else { air })
                );
                assert_eq!(state.players[&actor].inventory.slots, inventory);
                assert_eq!(state.fluid_ticks.image(), fluids);
                assert_repair(
                    &state,
                    actor,
                    &changes,
                    if pickup { &[POS, UPPER] } else { &[BASE, POS] },
                );
                assert_no_build_journal(&mut state, &output);
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn raycast_air_bucket_keep_and_deny_pick_up_exactly_the_native_target() {
    for decision in [HookDecision::Keep, HookDecision::Cancel] {
        let (mut state, actor) = fixture(None);
        let pool = BlockPos {
            x: 8,
            y: 200,
            z: 10,
        };
        let support = BlockPos { y: 199, ..pool };
        let stone = state.registry.states.stone;
        let source = state.registry.states.water_levels[0];
        plant(&mut state, support, stone);
        plant(&mut state, pool, source);
        held(&mut state, actor, "bucket", 1);
        let actor_slot = state.entities.slot_of(actor).unwrap();
        state.entities.set_rot(actor_slot, 25.0, -15.0);
        let inventory = state.players[&actor].inventory.slots.clone();
        let input = PlayerInput::UseItem {
            connection: CONNECTION,
            hand: Hand::Main,
            yaw: 0.0,
            pitch: 90.0,
            sequence: SEQUENCE,
        };
        let (gateway, _boundary, mut endpoint) = hooked(&mut state);
        let (result, edits) = through_host(&mut state, &gateway, &mut endpoint, input, |request| {
            request.answer(decision, Vec::new()).unwrap();
        })
        .await;
        assert_eq!(
            &state.entities.rot(actor_slot)[..2],
            &[25.0, -15.0],
            "preview does not apply the packet's native rotation early"
        );
        assert_eq!(
            edits,
            vec![BuildEdit::new(
                pool.x,
                pool.y,
                pool.z,
                source,
                state.registry.states.air
            )]
        );
        let submitted = match result {
            Ok(admitted) => admitted,
            Err(refusal) => {
                assert_eq!(refusal.reason, HookFailure::Cancelled);
                PlayerInput::BuildResync {
                    request: refusal.correction,
                }
            }
        };
        let inputs = if decision == HookDecision::Keep {
            vec![
                submitted,
                PlayerInput::Move {
                    connection: CONNECTION,
                    position: [9.5, 201.0, 10.5],
                    yaw: 0.0,
                    pitch: 90.0,
                    on_ground: true,
                },
            ]
        } else {
            vec![submitted]
        };
        let (output, drains, _) = run_tick(&mut state, &inputs);
        assert_eq!(
            &state.entities.rot(actor_slot)[..2],
            &[0.0, 90.0],
            "allow and denial retain ordinary UseItem packet rotation parity"
        );
        let changes = changes_for_player(drains);
        if decision == HookDecision::Keep {
            assert_committed(&mut state, &output, &edits);
            assert_ack(&changes);
            assert_eq!(
                state.players[&actor].inventory.slots[0],
                stack(&state, "water_bucket", 1)
            );
            assert_eq!(state.entities.pos(actor_slot)[0], 9.5);
            assert_eq!(
                state.world.block_at(9, 200, 10),
                Some(stone),
                "small movement must not reraycast the approved bucket at the new eye"
            );
        } else {
            assert_eq!(state.world.block_at(pool.x, pool.y, pool.z), Some(source));
            assert_eq!(state.players[&actor].inventory.slots, inventory);
            assert_repair(&state, actor, &changes, &[pool]);
            assert_no_build_journal(&mut state, &output);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn later_same_batch_slot_or_creative_stack_change_invalidates_kept_build_at_merge() {
    for mutation in ["selected-slot", "creative-count", "creative-components"] {
        let (mut state, actor) = fixture(None);
        held(&mut state, actor, "dirt", 3);
        let original = state.players[&actor].inventory.slots[0].clone();
        state.players.get_mut(&actor).unwrap().inventory.slots[1] = original.clone();
        if mutation != "selected-slot" {
            state.players.get_mut(&actor).unwrap().game_mode = crate::commands::GameMode::Creative;
        }
        let (gateway, _boundary, mut endpoint) = hooked(&mut state);
        let (admitted, edits) = keep(&mut state, &gateway, &mut endpoint, place()).await;
        assert_eq!(
            edits,
            vec![BuildEdit::new(
                POS.x,
                POS.y,
                POS.z,
                state.registry.states.air,
                state.registry.states.dirt
            )]
        );
        let mut replacement = original.clone();
        let later_input = match mutation {
            "selected-slot" => PlayerInput::SelectSlot {
                connection: CONNECTION,
                slot: 1,
            },
            "creative-count" => {
                replacement.count = 4;
                PlayerInput::CreativeSlot {
                    connection: CONNECTION,
                    slot: 36,
                    stack: replacement.clone(),
                }
            }
            "creative-components" => {
                replacement.custom_name = Some("changed later in the same input batch".into());
                PlayerInput::CreativeSlot {
                    connection: CONNECTION,
                    slot: 36,
                    stack: replacement.clone(),
                }
            }
            _ => unreachable!(),
        };
        // The authorization is first. Input-time validation would incorrectly
        // spend it before the legitimate later native inventory transition.
        let (output, drains, _) = run_tick(&mut state, &[admitted, later_input]);
        if mutation == "selected-slot" {
            assert_eq!(state.players[&actor].inventory.selected, 1);
            assert_eq!(state.players[&actor].inventory.slots[0], original);
            assert_eq!(state.players[&actor].inventory.slots[1], original);
        } else {
            assert_eq!(
                state.players[&actor].inventory.slots[0], replacement,
                "the real creative input applies; the earlier build does not"
            );
        }
        assert_eq!(
            state.world.block_at(POS.x, POS.y, POS.z),
            Some(state.registry.states.air)
        );
        assert_repair(&state, actor, &changes_for_player(drains), &[BASE, POS]);
        assert_no_build_journal(&mut state, &output);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn later_same_batch_accepted_far_move_invalidates_kept_build_at_merge() {
    let (mut state, actor) = fixture(None);
    held(&mut state, actor, "dirt", 3);
    let inventory = state.players[&actor].inventory.slots.clone();
    let (gateway, _boundary, mut endpoint) = hooked(&mut state);
    let (admitted, _) = keep(&mut state, &gateway, &mut endpoint, place()).await;
    let destination = [14.5, 201.0, 14.5];
    let (output, drains, _) = run_tick(
        &mut state,
        &[
            admitted,
            PlayerInput::Move {
                connection: CONNECTION,
                position: destination,
                yaw: 0.0,
                pitch: 0.0,
                on_ground: true,
            },
        ],
    );
    let slot = state.entities.slot_of(actor).unwrap();
    assert_eq!(
        state.entities.pos(slot),
        destination,
        "the movement input is accepted, not rejected to manufacture a stale-build result"
    );
    assert!(!crate::tick::within_block_reach(&state, actor, BASE));
    assert_eq!(
        state.world.block_at(POS.x, POS.y, POS.z),
        Some(state.registry.states.air)
    );
    assert_eq!(state.players[&actor].inventory.slots, inventory);
    assert_repair(&state, actor, &changes_for_player(drains), &[BASE, POS]);
    assert_no_build_journal(&mut state, &output);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn ordinary_yaw_sneak_and_small_move_preserve_approved_native_placement_geometry() {
    for change in ["yaw", "sneak", "small-move"] {
        let (mut state, actor) = fixture(None);
        held(&mut state, actor, "oak_stairs", 3);
        let actor_slot = state.entities.slot_of(actor).unwrap();
        state.entities.set_rot(actor_slot, 0.0, 0.0);
        let (gateway, _boundary, mut endpoint) = hooked(&mut state);
        let (admitted, edits) = keep(&mut state, &gateway, &mut endpoint, place()).await;
        assert_eq!(edits.len(), 1);
        let proposed = state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(edits[0].proposed_state))
            .unwrap();
        assert_eq!(proposed.block.id.path(), "oak_stairs");
        assert!(proposed.properties.iter().any(|(key, _)| key == "facing"));
        let later = match change {
            "yaw" => PlayerInput::Move {
                connection: CONNECTION,
                position: [8.5, 201.0, 10.5],
                yaw: 90.0,
                pitch: 0.0,
                on_ground: true,
            },
            "sneak" => PlayerInput::Sneak {
                connection: CONNECTION,
                sneaking: true,
            },
            "small-move" => PlayerInput::Move {
                connection: CONNECTION,
                position: [8.75, 201.0, 10.5],
                yaw: 0.0,
                pitch: 0.0,
                on_ground: true,
            },
            _ => unreachable!(),
        };
        let (output, drains, _) = run_tick(&mut state, &[admitted, later]);
        assert_committed(&mut state, &output, &edits);
        assert_ack(&changes_for_player(drains));
        assert_eq!(
            state.players[&actor].inventory.slots[0].count, 2,
            "{change}"
        );
        match change {
            "yaw" => assert_eq!(state.entities.rot(actor_slot)[0], 90.0),
            "sneak" => assert_ne!(
                state.entities.row(actor_slot).appearance.shared_flags & crate::tick::SNEAKING_FLAG,
                0
            ),
            "small-move" => assert_eq!(state.entities.pos(actor_slot)[0], 8.75),
            _ => unreachable!(),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn queued_preview_observes_earlier_tick_inputs_instead_of_stale_empty_hand_and_pose() {
    let (mut state, actor) = fixture(None);
    let stairs = stack(&state, "oak_stairs", 3);
    state.players.get_mut(&actor).unwrap().inventory.slots[1] = stairs;
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    let actor_slot = state.entities.slot_of(actor).unwrap();
    state.entities.set_rot(actor_slot, 0.0, 0.0);
    let (mut gateway, _boundary, mut endpoint) = hooked(&mut state);
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel(8);
    gateway.set_tick_owner(std::thread::current(), input_tx.clone());
    for input in [
        PlayerInput::SelectSlot {
            connection: CONNECTION,
            slot: 1,
        },
        PlayerInput::Move {
            connection: CONNECTION,
            position: [8.5, 201.0, 10.5],
            yaw: 90.0,
            pitch: 0.0,
            on_ground: true,
        },
        PlayerInput::Sneak {
            connection: CONNECTION,
            sneaking: true,
        },
    ] {
        input_tx.try_send(input).unwrap();
    }
    let mut admission = std::pin::pin!(gateway.admit(place()));
    pending(admission.as_mut());
    // Reproduce a query arriving just after an idle owner's input peek. The
    // query must remain pending rather than bypassing the judge on empty hand.
    answer_previews(&mut state);
    pending(admission.as_mut());
    assert!(state.build_queries.as_ref().unwrap().pending.is_some());
    let mut inputs = Vec::new();
    while let Ok(input) = input_rx.try_recv() {
        inputs.push(input);
    }
    assert!(matches!(
        inputs.last(),
        Some(PlayerInput::BuildPreviewFence { .. })
    ));
    let (idle, _, _) = run_tick(&mut state, &inputs);
    assert_no_build_journal(&mut state, &idle);
    assert_eq!(state.players[&actor].inventory.selected, 1);
    assert_eq!(state.entities.rot(actor_slot)[0], 90.0);
    pending(admission.as_mut());
    let request = take_request(&mut endpoint);
    let HookContext::Build(context) = request.context() else {
        panic!("newly selected buildable hand must reach the actual hook");
    };
    let edits = context.edits().to_vec();
    assert_eq!(edits.len(), 1);
    let expected = crate::tick::plan_client_build(&state, actor, &place()).unwrap();
    assert_eq!(
        edits,
        vec![BuildEdit::new(
            POS.x,
            POS.y,
            POS.z,
            state.registry.states.air,
            expected.footprint.block_write[0].1
        )]
    );
    assert_eq!(
        state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(edits[0].proposed_state))
            .unwrap()
            .block
            .id
            .path(),
        "oak_stairs",
        "post-input preview uses the newly selected item and native orientation"
    );
    request.answer(HookDecision::Keep, Vec::new()).unwrap();
    let admitted = admission.await.unwrap();
    assert!(matches!(admitted, PlayerInput::AuthorizedBuild { .. }));
    let (output, drains, _) = run_tick(&mut state, &[admitted]);
    assert_committed(&mut state, &output, &edits);
    assert_ack(&changes_for_player(drains));
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    assert_eq!(state.players[&actor].inventory.slots[1].count, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn merge_time_ray_refusal_does_not_overwrite_a_later_move_look() {
    let (mut state, actor) = fixture(None);
    let pool = BlockPos {
        x: 8,
        y: 200,
        z: 10,
    };
    let source = state.registry.states.water_levels[0];
    let stone = state.registry.states.stone;
    plant(&mut state, BlockPos { y: 199, ..pool }, stone);
    plant(&mut state, pool, source);
    held(&mut state, actor, "bucket", 1);
    let inventory = state.players[&actor].inventory.slots.clone();
    let (gateway, _boundary, mut endpoint) = hooked(&mut state);
    let (admitted, _) = keep(
        &mut state,
        &gateway,
        &mut endpoint,
        PlayerInput::UseItem {
            connection: CONNECTION,
            hand: Hand::Main,
            yaw: 0.0,
            pitch: 90.0,
            sequence: SEQUENCE,
        },
    )
    .await;
    let (output, drains, _) = run_tick(
        &mut state,
        &[
            admitted,
            PlayerInput::SelectSlot {
                connection: CONNECTION,
                slot: 1,
            },
            PlayerInput::Move {
                connection: CONNECTION,
                position: [8.5, 201.0, 10.5],
                yaw: 73.0,
                pitch: 12.0,
                on_ground: true,
            },
        ],
    );
    let actor_slot = state.entities.slot_of(actor).unwrap();
    assert_eq!(&state.entities.rot(actor_slot)[..2], &[73.0, 12.0]);
    assert_eq!(state.world.block_at(pool.x, pool.y, pool.z), Some(source));
    assert_eq!(state.players[&actor].inventory.slots, inventory);
    assert_repair(&state, actor, &changes_for_player(drains), &[pool]);
    assert_no_build_journal(&mut state, &output);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn idle_metadata_fence_releases_preview_without_advancing_native_tick() {
    let (mut state, actor) = fixture(None);
    held(&mut state, actor, "dirt", 3);
    let (mut gateway, _boundary, mut endpoint) = hooked(&mut state);
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel(8);
    gateway.set_tick_owner(std::thread::current(), input_tx);
    let tick = state.tick;
    let mut admission = std::pin::pin!(gateway.admit(place()));
    pending(admission.as_mut());
    answer_previews(&mut state);
    pending(admission.as_mut());
    let PlayerInput::BuildPreviewFence { fence } = input_rx.try_recv().unwrap() else {
        panic!("idle admission must enqueue only its metadata fence");
    };
    fence.apply();
    answer_previews(&mut state);
    pending(admission.as_mut());
    assert_eq!(
        state.tick, tick,
        "read-only idle work cannot manufacture another tick"
    );
    let request = take_request(&mut endpoint);
    let HookContext::Build(context) = request.context() else {
        panic!("real build context");
    };
    let edits = context.edits().to_vec();
    request.answer(HookDecision::Keep, Vec::new()).unwrap();
    let admitted = admission.await.unwrap();
    let (output, drains, _) = run_tick(&mut state, &[admitted]);
    assert_committed(&mut state, &output, &edits);
    assert_ack(&changes_for_player(drains));
    assert_eq!(state.players[&actor].inventory.slots[0].count, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn interrupted_authorization_reader_does_not_silently_lose_the_build() {
    let (mut state, actor) = fixture(None);
    held(&mut state, actor, "dirt", 3);
    let (gateway, _boundary, mut endpoint) = hooked(&mut state);
    let (admitted, edits) = keep(&mut state, &gateway, &mut endpoint, place()).await;
    let PlayerInput::AuthorizedBuild { authorization } = &admitted else {
        panic!("admitted native build");
    };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = authorization.inner.lock();
        panic!("interrupted authorization reader");
    }));
    let replay = admitted.clone();
    let (output, drains, _) = run_tick(&mut state, &[admitted, replay]);
    assert_committed(&mut state, &output, &edits);
    assert_ack(&changes_for_player(drains));
    assert_eq!(state.players[&actor].inventory.slots[0].count, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn early_finish_keeps_hook_approval_until_delayed_native_completion() {
    for decision in [HookDecision::Keep, HookDecision::Cancel] {
        let (mut state, actor) = fixture(None);
        state.players.get_mut(&actor).unwrap().game_mode = crate::commands::GameMode::Survival;
        let stone = block(&state, "stone", &[]);
        let mut hardness = vec![-1.0; stone as usize + 1];
        hardness[stone as usize] = 1.5;
        Arc::make_mut(&mut state.registry).mining =
            mc_data::block_mining::BlockMiningTable::from_arrays(
                "fixture",
                hardness,
                vec![false; stone as usize + 1],
            );
        plant(&mut state, POS, stone);
        run_tick(
            &mut state,
            &[PlayerInput::Dig {
                connection: CONNECTION,
                position: POS,
                action: DigAction::Start,
                sequence: SEQUENCE - 1,
            }],
        );
        let (gateway, _boundary, mut endpoint) = hooked(&mut state);
        // Do not use through_host: that helper primes fully elapsed mining.
        let mut admission = std::pin::pin!(gateway.admit(dig()));
        pending(admission.as_mut());
        answer_previews(&mut state);
        pending(admission.as_mut());
        let request = take_request(&mut endpoint);
        request.answer(decision, Vec::new()).unwrap();
        let admitted = admission.await;
        if decision == HookDecision::Keep {
            let authorized = admitted.unwrap();
            assert!(matches!(authorized, PlayerInput::AuthorizedBuild { .. }));
            run_tick(&mut state, &[authorized]);
            assert!(state.players[&actor].delayed_dig.is_some());
        } else {
            assert!(admitted.is_err());
        }
        assert_eq!(state.world.block_at(POS.x, POS.y, POS.z), Some(stone));
        for _ in 0..50 {
            run_tick(&mut state, &[]);
        }
        assert_eq!(
            state.world.block_at(POS.x, POS.y, POS.z),
            Some(if decision == HookDecision::Keep {
                state.registry.states.air
            } else {
                stone
            }),
        );
        assert!(state.players[&actor].delayed_dig.is_none());
    }
}
