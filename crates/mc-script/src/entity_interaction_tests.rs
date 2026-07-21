use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

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

    let manifest =
        ScriptPluginManifest::new("interactions", "Interactions", "0.1.0", SCRIPT_API_VERSION)
            .subscribe_event(" PLAYER.ENTITY_INTERACTED ")
            .validate()
            .unwrap();
    assert_eq!(
        manifest.event_subscriptions()[0].event_name(),
        "player.entity_interacted"
    );
}

#[cfg(feature = "lua-runtime")]
#[tokio::test]
async fn lua_handler_receives_exact_fifteen_field_payload() {
    static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

    struct TempPluginDir(std::path::PathBuf);

    impl Drop for TempPluginDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    let plugins_dir = std::env::temp_dir().join(format!(
        "solaris-mc-script-entity-interaction-{}-{}",
        std::process::id(),
        NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&plugins_dir);
    let _plugins_dir_guard = TempPluginDir(plugins_dir.clone());
    let plugin_dir = plugins_dir.join("interactions");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.toml"),
        r#"
            id = "interactions"
            name = "Interactions"
            version = "0.1.0"
            api = "0.6.0"
            events = ["player.entity_interacted"]
        "#,
    )
    .unwrap();
    std::fs::write(
        plugin_dir.join("main.lua"),
        r#"
            function on_player_entity_interacted(event)
                local expected = {
                    name = true, player_id = true, context_verified = true,
                    uuid = true, username = true, operator = true,
                    x = true, y = true, z = true, dimension = true,
                    entity_id = true, entity_type = true, hand = true,
                    secondary_action = true, game_mode = true,
                }
                local field_count = 0
                for field in pairs(event) do
                    assert(expected[field] == true, "unexpected field: " .. field)
                    field_count = field_count + 1
                end
                assert(field_count == 15)
                assert(event.name == "player.entity_interacted")
                assert(event.player_id == 7)
                assert(event.context_verified == true)
                assert(event.uuid == "123e4567-e89b-12d3-a456-426614174000")
                assert(event.username == "Alex")
                assert(event.operator == true)
                assert(event.x == 1.5)
                assert(event.y == 64.0)
                assert(event.z == -2.25)
                assert(event.dimension == "minecraft:overworld")
                assert(event.entity_id == 91)
                assert(event.entity_type == "minecraft:villager")
                assert(event.hand == "off_hand")
                assert(event.secondary_action == true)
                assert(event.game_mode == "creative")
                solaris.send_message(event.player_id, "entity-interacted")
            end
        "#,
    )
    .unwrap();

    let (boundary, host) = start_lua_host(LuaHostConfig::new(&plugins_dir)).unwrap();
    assert_eq!(host.loaded_plugins(), 1);
    boundary
        .try_enqueue_event(
            ScriptEvent::try_player_entity_interacted_with_context(
                ScriptPlayerId::new(7),
                ScriptPlayerContext::new(
                    "123e4567-e89b-12d3-a456-426614174000",
                    "Alex",
                    true,
                    1.5,
                    64.0,
                    -2.25,
                ),
                "minecraft:overworld",
                ScriptEntityId::new(91),
                "minecraft:villager",
                ScriptInteractionHand::OffHand,
                true,
                ScriptGameMode::Creative,
            )
            .unwrap(),
        )
        .unwrap();

    let command = tokio::time::timeout(Duration::from_secs(5), boundary.recv_command())
        .await
        .expect("Lua entity-interaction handler did not emit a command")
        .unwrap();
    let admitted = boundary.accept_host_command(command).unwrap();
    assert_eq!(
        admitted.request(),
        &ScriptCommand::SendChatMessage {
            player_id: ScriptPlayerId::new(7),
            message: "entity-interacted".to_owned(),
        }
    );

    drop(boundary);
    tokio::task::spawn_blocking(move || host.join())
        .await
        .unwrap()
        .unwrap();
}
