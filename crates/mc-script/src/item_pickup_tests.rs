use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

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

#[cfg(feature = "lua-runtime")]
#[tokio::test]
async fn lua_handler_receives_exact_payload() {
    static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

    struct TempPluginDir(std::path::PathBuf);

    impl Drop for TempPluginDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    let plugins_dir = std::env::temp_dir().join(format!(
        "solaris-mc-script-pickup-{}-{}",
        std::process::id(),
        NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&plugins_dir);
    let _plugins_dir_guard = TempPluginDir(plugins_dir.clone());
    let plugin_dir = plugins_dir.join("pickup");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.toml"),
        r#"
            id = "pickup"
            name = "Pickup"
            version = "0.1.0"
            api = "0.6.0"
            events = ["player.item_picked_up"]
        "#,
    )
    .unwrap();
    std::fs::write(
        plugin_dir.join("main.lua"),
        r#"
            function on_player_item_picked_up(event)
                local expected = {
                    name = true, player_id = true, context_verified = true,
                    uuid = true, username = true, operator = true,
                    x = true, y = true, z = true, dimension = true,
                    item_id = true, count = true, source = true,
                    game_mode = true,
                }
                local field_count = 0
                for field in pairs(event) do
                    assert(expected[field] == true, "unexpected field: " .. field)
                    field_count = field_count + 1
                end
                assert(field_count == 14)
                assert(event.name == "player.item_picked_up")
                assert(event.player_id == 7)
                assert(event.context_verified == true)
                assert(event.uuid == "123e4567-e89b-12d3-a456-426614174000")
                assert(event.username == "Alex")
                assert(event.operator == true)
                assert(event.x == 1.5)
                assert(event.y == 64.0)
                assert(event.z == -2.25)
                assert(event.dimension == "minecraft:overworld")
                assert(event.item_id == "minecraft:arrow")
                assert(math.type(event.count) == "integer" and event.count == 12)
                assert(event.source == "arrow")
                assert(event.game_mode == "adventure")
                solaris.send_message(event.player_id, "item-picked-up")
            end
        "#,
    )
    .unwrap();

    let (boundary, host) = start_lua_host(LuaHostConfig::new(&plugins_dir)).unwrap();
    assert_eq!(host.loaded_plugins(), 1);
    boundary
        .try_enqueue_event(
            ScriptEvent::try_player_item_picked_up_with_context(
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
                "minecraft:arrow",
                12,
                ScriptItemPickupSource::Arrow,
                ScriptGameMode::Adventure,
            )
            .unwrap(),
        )
        .unwrap();

    let command = tokio::time::timeout(Duration::from_secs(5), boundary.recv_command())
        .await
        .expect("Lua pickup handler did not emit a command")
        .unwrap();
    let admitted = boundary.accept_host_command(command).unwrap();
    assert_eq!(
        admitted.request(),
        &ScriptCommand::SendChatMessage {
            player_id: ScriptPlayerId::new(7),
            message: "item-picked-up".to_owned(),
        }
    );

    drop(boundary);
    tokio::task::spawn_blocking(move || host.join())
        .await
        .unwrap()
        .unwrap();
}
