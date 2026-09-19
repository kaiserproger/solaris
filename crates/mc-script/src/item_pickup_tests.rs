use super::*;

#[test]
fn event_is_a_validated_snapshot_without_integer_caps() {
    let context = ScriptPlayerContext::new(
        "123e4567-e89b-12d3-a456-426614174000",
        "kaiser",
        true,
        12.25,
        70.0,
        -4.5,
    );
    let event = ScriptEvent::try_player_item_picked_up_with_context(
        ScriptPlayerId::new(42),
        context.clone(),
        "minecraft:the_nether",
        "minecraft:arrow",
        u64::from(u32::MAX) + 1,
        ScriptItemPickupSource::Arrow,
        ScriptGameMode::Adventure,
    )
    .unwrap();

    assert_eq!(event.event_name(), "player.item_picked_up");
    assert_eq!(event.target_plugin_id(), None);
    assert_eq!(event.validate(), Ok(()));
    assert!(matches!(
        event.kind(),
        ScriptEventKind::PlayerItemPickedUp {
            player_id,
            context: event_context,
            dimension,
            item_id,
            count,
            source,
            game_mode,
        } if *player_id == ScriptPlayerId::new(42)
            && event_context == &context
            && dimension == "minecraft:the_nether"
            && item_id == "minecraft:arrow"
            && *count == u64::from(u32::MAX) + 1
            && *source == ScriptItemPickupSource::Arrow
            && *game_mode == ScriptGameMode::Adventure
    ));
}

#[test]
fn event_rejects_invalid_ids_and_zero_count() {
    let context = ScriptPlayerContext::new("player-42", "kaiser", false, 0.0, 64.0, 0.0);
    for (dimension, item_id, count) in [
        ("overworld", "minecraft:stick", 1),
        ("minecraft:overworld", "minecraft:Stick", 1),
        ("minecraft:overworld", "minecraft:stick", 0),
    ] {
        assert!(
            ScriptEvent::try_player_item_picked_up_with_context(
                ScriptPlayerId::new(42),
                context.clone(),
                dimension,
                item_id,
                count,
                ScriptItemPickupSource::ItemEntity,
                ScriptGameMode::Survival,
            )
            .is_err(),
            "accepted dimension={dimension:?} item_id={item_id:?} count={count}"
        );
    }

    assert_eq!(ScriptItemPickupSource::ItemEntity.as_str(), "item_entity");
    assert_eq!(ScriptItemPickupSource::Arrow.as_str(), "arrow");
}
