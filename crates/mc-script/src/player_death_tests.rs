#[cfg(feature = "lua-runtime")]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "lua-runtime")]
use std::time::Duration;

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
        "solaris-mc-script-player-death-{}-{}",
        std::process::id(),
        NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&plugins_dir);
    let _plugins_dir_guard = TempPluginDir(plugins_dir.clone());
    let plugin_dir = plugins_dir.join("death");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.toml"),
        r#"
            id = "death"
            name = "Death"
            version = "0.1.0"
            api = "0.6.0"
            events = ["player.died"]
        "#,
    )
    .unwrap();
    std::fs::write(
        plugin_dir.join("main.lua"),
        r#"
            function on_player_died(event)
                local expected = {
                    name = true, player_id = true, context_verified = true,
                    uuid = true, username = true, operator = true,
                    x = true, y = true, z = true, dimension = true,
                    game_mode = true,
                }
                local field_count = 0
                for field in pairs(event) do
                    assert(expected[field] == true, "unexpected field: " .. field)
                    field_count = field_count + 1
                end
                assert(field_count == 11)
                assert(event.name == "player.died")
                assert(event.player_id == 7)
                assert(event.context_verified == true)
                assert(event.uuid == "123e4567-e89b-12d3-a456-426614174000")
                assert(event.username == "Alex")
                assert(event.operator == true)
                assert(event.x == 1.5)
                assert(event.y == 64.0)
                assert(event.z == -2.25)
                assert(event.dimension == "minecraft:overworld")
                assert(event.game_mode == "survival")
                solaris.send_message(event.player_id, "player-died")
            end
        "#,
    )
    .unwrap();

    let (boundary, host) = start_lua_host(LuaHostConfig::new(&plugins_dir)).unwrap();
    assert_eq!(host.loaded_plugins(), 1);
    boundary
        .try_enqueue_event(
            ScriptEvent::try_player_died_with_context(
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
                ScriptGameMode::Survival,
            )
            .unwrap(),
        )
        .unwrap();

    let command = tokio::time::timeout(Duration::from_secs(5), boundary.recv_command())
        .await
        .expect("Lua death handler did not emit a command")
        .unwrap();
    let admitted = boundary.accept_host_command(command).unwrap();
    assert_eq!(
        admitted.request(),
        &ScriptCommand::SendChatMessage {
            player_id: ScriptPlayerId::new(7),
            message: "player-died".to_owned(),
        }
    );

    drop(boundary);
    tokio::task::spawn_blocking(move || host.join())
        .await
        .unwrap()
        .unwrap();
}
