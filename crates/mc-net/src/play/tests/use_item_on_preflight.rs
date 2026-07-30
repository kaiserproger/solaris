use super::{
    GameMode, PlayerPose, ServerboundUseItemOn, SurvivalState, UseItemOnNoOpReason,
    UseItemOnOutcome, classify_use_item_on_preflight, pack_block_pos,
};

pub(super) fn test_use_item_on(position: i64) -> ServerboundUseItemOn {
    ServerboundUseItemOn {
        hand: mc_protocol::packets::play::InteractionHand::MainHand,
        position,
        direction: mc_protocol::packets::play::Direction::Up,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside: false,
        world_border_hit: false,
        sequence: 4,
    }
}

#[test]
fn use_item_on_preflight_reports_dead_survival_player() {
    let mut survival = SurvivalState::FULL;
    survival.health = 0.0;
    let action = test_use_item_on(pack_block_pos(0, 64, 0));

    assert_eq!(
        classify_use_item_on_preflight(
            GameMode::Survival,
            survival,
            PlayerPose::new(0.5, 64.0, 0.5),
            &action,
        ),
        UseItemOnOutcome::NoOp {
            reason: UseItemOnNoOpReason::DeadPlayer,
        }
    );
}

#[test]
fn use_item_on_preflight_reports_unsupported_game_mode() {
    let action = test_use_item_on(pack_block_pos(0, 64, 0));

    assert_eq!(
        classify_use_item_on_preflight(
            GameMode::Adventure,
            SurvivalState::FULL,
            PlayerPose::new(0.5, 64.0, 0.5),
            &action,
        ),
        UseItemOnOutcome::NoOp {
            reason: UseItemOnNoOpReason::UnsupportedGameMode,
        }
    );
}

#[test]
fn use_item_on_preflight_reports_out_of_reach_survival_target() {
    let action = test_use_item_on(pack_block_pos(128, 64, 0));

    assert_eq!(
        classify_use_item_on_preflight(
            GameMode::Survival,
            SurvivalState::FULL,
            PlayerPose::new(0.5, 64.0, 0.5),
            &action,
        ),
        UseItemOnOutcome::NoOp {
            reason: UseItemOnNoOpReason::OutOfReach,
        }
    );
}

#[test]
fn use_item_on_preflight_rejects_out_of_reach_creative_and_allows_reachable_targets() {
    let action = test_use_item_on(pack_block_pos(0, 64, 0));

    assert_eq!(
        classify_use_item_on_preflight(
            GameMode::Creative,
            SurvivalState::FULL,
            PlayerPose::new(100.5, 64.0, 100.5),
            &action,
        ),
        UseItemOnOutcome::NoOp {
            reason: UseItemOnNoOpReason::OutOfReach,
        }
    );
    assert_eq!(
        classify_use_item_on_preflight(
            GameMode::Creative,
            SurvivalState::FULL,
            PlayerPose::new(0.5, 64.0, 0.5),
            &action,
        ),
        UseItemOnOutcome::PlaceBlock
    );
    assert_eq!(
        classify_use_item_on_preflight(
            GameMode::Survival,
            SurvivalState::FULL,
            PlayerPose::new(0.5, 64.0, 0.5),
            &action,
        ),
        UseItemOnOutcome::PlaceBlock
    );
}
