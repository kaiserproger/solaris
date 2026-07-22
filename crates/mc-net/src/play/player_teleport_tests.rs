use std::sync::Arc;

use mc_protocol::packets::play::{Direction, InteractionHand, ItemStack};
use mc_protocol::packets::play::{MovePlayerFlags, PlayerInput};
use mc_script::{ScriptPlayerTeleportFailure, ScriptPosition};

use super::PlayerPose;
use super::block_break::PendingBreak;
use super::combat::ShieldUseState;
use super::player_teleport::prepare_script_player_teleport;
use super::survival::{PendingUse, UseKind};
use super::tests::{fluid_test_registry, interaction_state_for_blocks};

#[test]
fn teleport_candidate_preserves_orientation_and_stance_but_resets_fall_origin() {
    let current = PlayerPose {
        x: 1.0,
        y: 80.0,
        z: 2.0,
        yaw: 37.5,
        pitch: -12.0,
        flags: MovePlayerFlags::new(true, false),
        input: PlayerInput::default(),
        sprinting: true,
        shifting: true,
        in_water: true,
        eye_in_water: true,
        swimming: true,
        fall_start_y: 120.0,
    };
    let target = ScriptPosition::try_new(12.5, 70.0, -4.5).unwrap();

    let candidate = prepare_script_player_teleport(current, target, false).unwrap();

    assert_eq!((candidate.x, candidate.y, candidate.z), (12.5, 70.0, -4.5));
    assert_eq!((candidate.yaw, candidate.pitch), (37.5, -12.0));
    assert_eq!(candidate.flags, current.flags);
    assert_eq!(candidate.input, current.input);
    assert!(candidate.sprinting);
    assert!(candidate.shifting);
    assert_eq!(candidate.fall_start_y, 70.0);
}

#[test]
fn pending_teleport_rejects_before_building_a_candidate() {
    assert_eq!(
        prepare_script_player_teleport(
            PlayerPose::new(0.5, 64.0, 0.5),
            ScriptPosition::try_new(12.5, 70.0, -4.5).unwrap(),
            true,
        )
        .unwrap_err(),
        ScriptPlayerTeleportFailure::TeleportPending
    );
}

#[test]
fn committed_teleport_clears_every_position_bound_interaction() {
    let mut state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    let pending_break = PendingBreak {
        sequence: 1,
        position: 2,
        direction: Direction::Up,
        started_tick: 3,
        started_progress_per_tick: 0.5,
        held_hotbar_slot: 0,
        held_item: Some(ItemStack::new(4, 1)),
        expected_target: None,
        stop_received: false,
    };
    state.pending_break = Some(pending_break.clone());
    state.delayed_break = Some(pending_break);
    state.pending_use = Some(PendingUse {
        started_tick: 5,
        required_ticks: 20,
        held_hotbar_slot: 0,
        held_slot: 36,
        held_item_id: 4,
        kind: UseKind::Bow,
    });
    state.shield_use = Some(ShieldUseState {
        hand: InteractionHand::MainHand,
        started_tick: 6,
        slot: 36,
        stack: ItemStack::new(5, 1),
    });

    super::player_teleport::clear_player_interactions_for_teleport(&mut state);

    assert!(state.pending_break.is_none());
    assert!(state.delayed_break.is_none());
    assert!(state.pending_use.is_none());
    assert!(state.shield_use.is_none());
}
