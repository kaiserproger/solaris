//! Permission enforcement through real player/shared-tick admission.
use crate::boundary::{ConnectionId, PlayerChange, PlayerInput};
use crate::tick::{TickState, run_tick};
use std::sync::Arc;

fn fixture() -> TickState {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let world = crate::horde_bench::flat_world(&registry);
    let mut nether = TickState::new(
        Arc::clone(&registry),
        crate::horde_bench::flat_world(&registry),
        0,
        512,
    );
    nether.dimension = crate::dimensions::Dimension::Nether;
    let mut state = TickState::new(registry, world, 0, 512);
    state
        .other_dimensions
        .insert(crate::dimensions::Dimension::Nether, Box::new(nether));
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::from_u128(1),
            name: "Alice".into(),
            permission_level: 2,
        }],
    );
    state
}

fn player(state: &mut TickState, permission_level: u8, command: &str) -> Vec<PlayerChange> {
    assert!(
        !state.other_dimensions.is_empty(),
        "exercise deferred shared command execution"
    );
    let (_, deliveries, _) = run_tick(
        state,
        &[PlayerInput::Command {
            connection: ConnectionId(1),
            permission_level,
            command: command.into(),
        }],
    );
    deliveries
        .into_iter()
        .filter(|(connection, _)| *connection == ConnectionId(1))
        .flat_map(|(_, queue)| queue)
        .collect()
}

fn console(state: &mut TickState, text: &str) {
    let (command, mut completed) = crate::command_console::ConsoleCommand::new(text.into());
    run_tick(state, &[PlayerInput::ConsoleCommand(command)]);
    assert!(completed.try_recv().is_ok());
}

fn install_pack(state: &mut TickState, resources: &[(&str, &str)]) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let pack = directory.path().join("permissions");
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::write(
        pack.join("pack.mcmeta"),
        r#"{"pack":{"pack_format":101,"description":"permission regression"}}"#,
    )
    .unwrap();
    for (path, contents) in resources {
        let path = pack.join("data").join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    state.datapacks = crate::datapacks::Runtime::new(
        crate::datapacks::PackSet::load(directory.path(), Default::default()).unwrap(),
        directory.path().to_path_buf(),
    );
    directory
}

#[test]
fn level_two_nested_tick_is_denied_through_shared_player_input() {
    let mut state = fixture();
    assert!(!state.tick_control.frozen());
    let replies = player(&mut state, 2, "execute run tick freeze");
    assert!(
        !state.tick_control.frozen(),
        "level2 must not reach the tick mutation"
    );
    assert!(replies.iter().any(|reply| matches!(reply,
        PlayerChange::Chat { message } if message == &crate::commands::CommandError::Unknown.message()
    )));
    assert!(state.command_context.current.is_none());
}

#[test]
fn level_two_nested_block_log_is_denied_without_enabling_inspection() {
    let mut state = fixture();
    let entity = state.connections.entity_of(ConnectionId(1)).unwrap();
    let position = crate::boundary::BlockPos { x: 0, y: 60, z: 0 };
    assert!(!crate::block_log::inspect(
        &mut state,
        ConnectionId(1),
        entity,
        position,
        false
    ));
    player(&mut state, 2, "execute run co inspect");
    assert!(
        !crate::block_log::inspect(&mut state, ConnectionId(1), entity, position, false),
        "level2 must not enable the level3 block-log inspector"
    );
    player(&mut state, 3, "execute run co inspect");
    assert!(crate::block_log::inspect(
        &mut state,
        ConnectionId(1),
        entity,
        position,
        false
    ));
    player(&mut state, 3, "co inspect");
    assert!(!crate::block_log::inspect(
        &mut state,
        ConnectionId(1),
        entity,
        position,
        false
    ));
}

#[test]
fn direct_and_nested_tick_follow_numeric_levels_zero_through_four() {
    let mut state = fixture();
    let mut observed = Vec::new();
    for level in 0..=4 {
        for text in [
            "tick freeze",
            "execute run tick freeze",
            "execute run execute run tick freeze",
            "execute as @s run tick freeze",
            "execute in minecraft:the_nether run tick freeze",
        ] {
            console(&mut state, "tick unfreeze");
            player(&mut state, level, text);
            observed.push((level, text, state.tick_control.frozen()));
            assert!(state.command_context.current.is_none());
        }
    }
    for (level, text, frozen) in observed {
        assert_eq!(frozen, level >= 3, "permission={level}: {text}");
    }
}

#[test]
fn nested_function_inherits_permission_without_escalating_or_lowering_it() {
    let mut state = fixture();
    let _directory = install_pack(
        &mut state,
        &[
            (
                "test/function/outer.mcfunction",
                "execute run function test:inner\ntime set 321",
            ),
            ("test/function/inner.mcfunction", "execute run tick freeze"),
        ],
    );
    for level in [2, 3] {
        for text in [
            "execute as @s run function test:outer",
            "debug function test:outer",
        ] {
            console(&mut state, "tick unfreeze");
            player(&mut state, level, text);
            assert_eq!(state.day_time, 321, "the function ran its level2 leaf");
            assert_eq!(
                state.tick_control.frozen(),
                level >= 3,
                "source level {level}: {text}"
            );
            assert!(state.command_context.current.is_none());
        }
    }
}

#[test]
fn console_level_four_can_run_direct_nested_and_function_tick_commands() {
    let mut state = fixture();
    let _directory = install_pack(
        &mut state,
        &[("test/function/freeze.mcfunction", "execute run tick freeze")],
    );
    for text in [
        "tick freeze",
        "execute run tick freeze",
        "function test:freeze",
    ] {
        console(&mut state, "tick unfreeze");
        console(&mut state, text);
        assert!(state.tick_control.frozen(), "{text}");
        assert!(state.command_context.current.is_none());
    }
}

#[test]
fn load_function_keeps_level_two_despite_reserved_console_connection() {
    let mut state = fixture();
    let _directory = install_pack(
        &mut state,
        &[
            (
                "test/function/server.mcfunction",
                "tick freeze\ntime set 321",
            ),
            (
                "minecraft/tags/function/load.json",
                r#"{"values":["test:server"]}"#,
            ),
        ],
    );
    run_tick(&mut state, &[]);
    assert_eq!(state.day_time, 321, "load function must actually execute");
    assert!(
        !state.tick_control.frozen(),
        "server-function source is level2, not console4"
    );
}

#[test]
fn scheduled_function_uses_level_two_not_its_level_four_scheduler() {
    let mut state = fixture();
    let _directory = install_pack(
        &mut state,
        &[(
            "test/function/server.mcfunction",
            "tick freeze\ntime set 321",
        )],
    );
    console(&mut state, "schedule function test:server 1t");
    assert!(!state.datapacks.image().is_empty());
    run_tick(&mut state, &[]);
    assert!(
        state.datapacks.image().is_empty(),
        "scheduled invocation became due"
    );
    assert_eq!(
        state.day_time, 321,
        "scheduled function must actually execute"
    );
    assert!(
        !state.tick_control.frozen(),
        "scheduled source must not inherit console4"
    );
}

#[test]
fn public_and_level_two_native_commands_retain_existing_admission() {
    let mut state = fixture();
    for level in 0..=4 {
        let replies = player(&mut state, level, "me public-permission-check");
        assert!(!replies.iter().any(|reply| matches!(reply,
            PlayerChange::Chat { message } if message == &crate::commands::CommandError::Unknown.message()
        )));
    }
    player(&mut state, 2, "execute run time set 321");
    assert_eq!(state.day_time, 321);
    player(&mut state, 1, "execute run time set 999");
    assert_ne!(state.day_time, 999);
}

#[test]
fn plugin_owned_tick_root_routes_before_native_permission_and_never_falls_back() {
    let mut state = fixture();
    let capacity = std::num::NonZeroUsize::new(4).unwrap();
    let (boundary, endpoint) = mc_script::script_boundary_pair(capacity, capacity);
    let manifest = mc_script::ScriptPluginManifest::new(
        "owner",
        "Owner",
        "0.1.0",
        mc_script::COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_player_command_root("tick")
    .validate()
    .unwrap();
    endpoint.register_plugin_routes(&manifest).unwrap();
    state.plugin_runtime = Some(crate::plugin::PluginRuntime::with_test_boundary(
        boundary.clone(),
    ));
    crate::command_console::execute(
        &mut state,
        ConnectionId(1),
        "tick freeze",
        0,
        &mut crate::boundary::ChangeSet::default(),
    );
    assert!(!state.tick_control.frozen());
    assert!(
        matches!(state.plugin_runtime.as_ref().unwrap().pending_events[0].kind(),
        mc_script::ScriptEventKind::PlayerCommand { root, .. } if root == "tick")
    );
    boundary.close_event_admission();
    crate::command_console::execute(
        &mut state,
        ConnectionId(1),
        "tick freeze",
        2,
        &mut crate::boundary::ChangeSet::default(),
    );
    assert!(!state.tick_control.frozen());
    assert_eq!(
        state.outbox[&ConnectionId(1)].last(),
        Some(&PlayerChange::Chat {
            message: "Plugin command admission is unavailable.".into(),
        })
    );
}
