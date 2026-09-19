use super::*;

#[test]
fn event_is_a_validated_authoritative_player_snapshot() {
    let context = ScriptPlayerContext::new(
        "123e4567-e89b-12d3-a456-426614174000",
        "kaiser",
        true,
        12.25,
        70.0,
        -4.5,
    );
    let event = ScriptEvent::try_player_died_with_context(
        ScriptPlayerId::new(42),
        context.clone(),
        "minecraft:the_nether",
        ScriptGameMode::Adventure,
    )
    .unwrap();

    assert_eq!(event.event_name(), "player.died");
    assert_eq!(event.target_plugin_id(), None);
    assert_eq!(event.validate(), Ok(()));
    assert!(matches!(
        event.kind(),
        ScriptEventKind::PlayerDied {
            player_id,
            context: event_context,
            dimension,
            game_mode,
        } if *player_id == ScriptPlayerId::new(42)
            && event_context == &context
            && dimension == "minecraft:the_nether"
            && *game_mode == ScriptGameMode::Adventure
    ));
}

#[test]
fn event_rejects_invalid_context_and_dimension() {
    let valid = ScriptPlayerContext::new("player-42", "kaiser", false, 0.0, 64.0, 0.0);
    assert!(
        ScriptEvent::try_player_died_with_context(
            ScriptPlayerId::new(42),
            valid,
            "overworld",
            ScriptGameMode::Survival,
        )
        .is_err()
    );

    assert!(ScriptPlayerContext::try_new("player-42", "", false, 0.0, 64.0, 0.0).is_err());
}
