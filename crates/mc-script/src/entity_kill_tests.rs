use super::*;

#[test]
fn event_is_a_validated_authoritative_melee_kill_snapshot() {
    let context = ScriptPlayerContext::new(
        "123e4567-e89b-12d3-a456-426614174000",
        "kaiser",
        true,
        12.25,
        70.0,
        -4.5,
    );
    let event = ScriptEvent::try_player_entity_killed_with_context(
        ScriptPlayerId::new(42),
        context.clone(),
        "minecraft:the_nether",
        ScriptEntityId::new(91),
        "minecraft:zombie",
        ScriptEntityKillSource::Melee,
        ScriptGameMode::Adventure,
    )
    .unwrap();

    assert_eq!(event.event_name(), "player.entity_killed");
    assert_eq!(event.target_plugin_id(), None);
    assert_eq!(event.validate(), Ok(()));
    assert!(matches!(
        event.kind(),
        ScriptEventKind::PlayerEntityKilled {
            player_id,
            context: event_context,
            dimension,
            entity_id,
            entity_type,
            source,
            game_mode,
        } if *player_id == ScriptPlayerId::new(42)
            && event_context == &context
            && dimension == "minecraft:the_nether"
            && *entity_id == ScriptEntityId::new(91)
            && entity_type == "minecraft:zombie"
            && *source == ScriptEntityKillSource::Melee
            && *game_mode == ScriptGameMode::Adventure
    ));
}

#[test]
fn event_rejects_invalid_context_dimension_and_entity_type() {
    let valid = ScriptPlayerContext::new("player-42", "kaiser", false, 0.0, 64.0, 0.0);
    for (dimension, entity_type) in [
        ("overworld", "minecraft:zombie"),
        ("minecraft:overworld", "minecraft:Zombie"),
    ] {
        assert!(
            ScriptEvent::try_player_entity_killed_with_context(
                ScriptPlayerId::new(42),
                valid.clone(),
                dimension,
                ScriptEntityId::new(91),
                entity_type,
                ScriptEntityKillSource::Melee,
                ScriptGameMode::Survival,
            )
            .is_err(),
            "accepted dimension={dimension:?} entity_type={entity_type:?}"
        );
    }

    assert!(ScriptPlayerContext::try_new("player-42", "", false, 0.0, 64.0, 0.0).is_err());
}
