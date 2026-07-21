use super::player_breathing::{
    PLAYER_AIR_SUPPLY_METADATA_INDEX, PlayerBreathingState, player_can_drown,
};
use mc_protocol::packets::play::{EntityDataValue, GameMode};

#[test]
fn submerged_survival_player_drowns_on_vanilla_air_boundary() {
    let mut breathing = PlayerBreathingState::default();
    for _ in 0..319 {
        let (next, tick) = breathing.tick(true, true);
        breathing = next;
        assert_eq!(tick.drowning_damage, 0.0);
    }
    assert_eq!(breathing.air_supply(), -19);

    let (next, tick) = breathing.tick(true, true);
    assert_eq!(next.air_supply(), 0);
    assert_eq!(tick.drowning_damage, 2.0);
}

#[test]
fn air_recovers_by_four_and_caps_at_vanilla_maximum() {
    let mut breathing = PlayerBreathingState::default();
    for _ in 0..10 {
        (breathing, _) = breathing.tick(true, true);
    }
    assert_eq!(breathing.air_supply(), 290);

    let (breathing, tick) = breathing.tick(false, true);
    assert!(tick.air_changed);
    assert_eq!(breathing.air_supply(), 294);
    let (breathing, _) = breathing.tick(false, true);
    let (breathing, _) = breathing.tick(false, true);
    assert_eq!(breathing.air_supply(), 300);
}

#[test]
fn invulnerable_player_refills_instead_of_losing_air() {
    let mut breathing = PlayerBreathingState::default();
    (breathing, _) = breathing.tick(true, true);
    let (breathing, tick) = breathing.tick(true, false);
    assert_eq!(breathing.air_supply(), 300);
    assert_eq!(tick.drowning_damage, 0.0);
}

#[test]
fn air_metadata_uses_vanilla_entity_index_one() {
    let breathing = PlayerBreathingState::default();
    assert!(matches!(
        breathing.metadata(),
        EntityDataValue::Int {
            index: PLAYER_AIR_SUPPLY_METADATA_INDEX,
            value: 300
        }
    ));
}

#[test]
fn survival_and_adventure_players_drown_but_invulnerable_modes_do_not() {
    assert!(player_can_drown(GameMode::Survival, false));
    assert!(player_can_drown(GameMode::Adventure, false));
    assert!(!player_can_drown(GameMode::Creative, false));
    assert!(!player_can_drown(GameMode::Spectator, false));
    assert!(!player_can_drown(GameMode::Survival, true));
}

#[test]
fn rejected_drowning_damage_can_retry_without_losing_the_air_boundary() {
    let mut breathing = PlayerBreathingState::default();
    for _ in 0..319 {
        (breathing, _) = breathing.tick(true, true);
    }

    let (uncommitted, first) = breathing.tick(true, true);
    let (retried, second) = breathing.tick(true, true);

    assert_eq!(breathing.air_supply(), -19);
    assert_eq!(uncommitted.air_supply(), 0);
    assert_eq!(retried.air_supply(), 0);
    assert_eq!(first.drowning_damage, 2.0);
    assert_eq!(second.drowning_damage, 2.0);
}

#[test]
fn respawn_reset_restores_full_air() {
    let mut breathing = PlayerBreathingState::default();
    (breathing, _) = breathing.tick(true, true);

    assert!(breathing.reset());
    assert_eq!(breathing.air_supply(), PlayerBreathingState::MAX_AIR_SUPPLY);
    assert!(!breathing.reset());
}
