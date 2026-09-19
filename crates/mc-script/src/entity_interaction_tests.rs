use super::*;

#[test]
fn event_is_an_exact_validated_authoritative_interaction_snapshot() {
    let context = ScriptPlayerContext::new(
        "123e4567-e89b-12d3-a456-426614174000",
        "kaiser",
        true,
        12.25,
        70.0,
        -4.5,
    );
    let event = ScriptEvent::try_player_entity_interacted_with_context(
        ScriptPlayerId::new(42),
        context.clone(),
        "minecraft:the_nether",
        ScriptEntityId::new(91),
        "minecraft:villager",
        ScriptInteractionHand::OffHand,
        true,
        ScriptGameMode::Adventure,
    )
    .unwrap();

    assert_eq!(event.event_name(), "player.entity_interacted");
    assert_eq!(event.target_plugin_id(), None);
    assert_eq!(event.validate(), Ok(()));
    assert!(matches!(
        event.kind(),
        ScriptEventKind::PlayerEntityInteracted {
            player_id,
            context: event_context,
            dimension,
            entity_id,
            entity_type,
            hand,
            secondary_action,
            game_mode,
        } if *player_id == ScriptPlayerId::new(42)
            && event_context == &context
            && dimension == "minecraft:the_nether"
            && *entity_id == ScriptEntityId::new(91)
            && entity_type == "minecraft:villager"
            && *hand == ScriptInteractionHand::OffHand
            && *secondary_action
            && *game_mode == ScriptGameMode::Adventure
    ));
}

#[test]
fn event_rejects_invalid_context_dimension_and_entity_type() {
    let valid = ScriptPlayerContext::new("player-42", "kaiser", false, 0.0, 64.0, 0.0);
    for (dimension, entity_type) in [
        ("overworld", "minecraft:villager"),
        ("minecraft:overworld", "minecraft:Villager"),
    ] {
        assert!(
            ScriptEvent::try_player_entity_interacted_with_context(
                ScriptPlayerId::new(42),
                valid.clone(),
                dimension,
                ScriptEntityId::new(91),
                entity_type,
                ScriptInteractionHand::MainHand,
                false,
                ScriptGameMode::Survival,
            )
            .is_err(),
            "accepted dimension={dimension:?} entity_type={entity_type:?}"
        );
    }

    let mut invalid_context = valid;
    invalid_context.snapshot.username.clear();
    assert!(
        ScriptEvent::try_player_entity_interacted_with_context(
            ScriptPlayerId::new(42),
            invalid_context,
            "minecraft:overworld",
            ScriptEntityId::new(91),
            "minecraft:villager",
            ScriptInteractionHand::MainHand,
            false,
            ScriptGameMode::Creative,
        )
        .is_err()
    );
}

#[test]
fn hand_values_and_subscription_name_are_exact() {
    assert_eq!(ScriptInteractionHand::MainHand.as_str(), "main_hand");
    assert_eq!(ScriptInteractionHand::OffHand.as_str(), "off_hand");

    let manifest = ScriptPluginManifest::new(
        "interactions",
        "Interactions",
        "0.1.0",
        COMPONENT_PLUGIN_API_VERSION,
    )
    .subscribe_event(" PLAYER.ENTITY_INTERACTED ")
    .validate()
    .unwrap();
    assert_eq!(
        manifest.event_subscriptions()[0].event_name(),
        "player.entity_interacted"
    );
}
