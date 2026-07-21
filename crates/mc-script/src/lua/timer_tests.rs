use std::num::NonZeroUsize;

use super::*;
use crate::{RuntimeControls, SCRIPT_API_VERSION, ScriptPluginManifest};

fn runtime(source: &str) -> LuaScriptRuntime {
    let manifest = ScriptPluginManifest::new("timers", "Timers", "0.1.0", SCRIPT_API_VERSION)
        .subscribe_event("server.started")
        .validate()
        .unwrap();
    LuaScriptRuntime::from_source(manifest, source, LuaRuntimeLimits::default()).unwrap()
}

fn handle(runtime: &mut LuaScriptRuntime, event: &ScriptEvent) -> CommandBatch {
    let controls = RuntimeControls::unrestricted();
    runtime
        .handle_event(
            event,
            RuntimeContext::new(&controls, NonZeroUsize::new(COMMANDS_PER_EVENT).unwrap()),
        )
        .unwrap()
}

fn broadcast_messages(batch: CommandBatch) -> Vec<String> {
    batch
        .into_commands()
        .into_iter()
        .map(|command| match command {
            ScriptCommand::BroadcastChatMessage { message } => message,
            other => panic!("unexpected timer command: {other:?}"),
        })
        .collect()
}

#[test]
fn timer_fires_from_pushed_simulation_ticks_without_tick_subscription() {
    let mut runtime = runtime(
        r#"
            function on_server_started(_event)
                local scheduled_tick = solaris.schedule_timer("refresh", 3)
                assert(scheduled_tick == 13)
            end

            function on_plugin_timer(event)
                assert(event.name == "plugin.timer")
                assert(event.timer_id == "refresh")
                assert(event.scheduled_tick == 13)
                assert(event.fired_tick == 13)
                solaris.broadcast("timer-fired")
            end
        "#,
    );

    assert!(
        handle(&mut runtime, &ScriptEvent::server_tick(10))
            .commands()
            .is_empty()
    );
    assert!(
        handle(&mut runtime, &ScriptEvent::server_started())
            .commands()
            .is_empty()
    );
    assert!(
        handle(&mut runtime, &ScriptEvent::server_tick(12))
            .commands()
            .is_empty()
    );
    assert_eq!(
        broadcast_messages(handle(&mut runtime, &ScriptEvent::server_tick(13))),
        ["timer-fired"]
    );
}

#[test]
fn timer_replace_cancel_and_callback_reschedule_are_atomic() {
    let mut runtime = runtime(
        r#"
            function on_server_started(_event)
                assert(solaris.schedule_timer("repeat", 5) == 15)
                assert(solaris.schedule_timer("cancelled", 2) == 12)
                assert(solaris.cancel_timer("cancelled") == true)
                assert(solaris.cancel_timer("missing") == false)
                assert(solaris.schedule_timer("repeat", 3) == 13)
            end

            function on_plugin_timer(event)
                solaris.broadcast(event.timer_id .. ":" .. event.scheduled_tick .. ":" .. event.fired_tick)
                if event.timer_id == "repeat" and event.scheduled_tick == 13 then
                    assert(solaris.schedule_timer("repeat", 2) == 15)
                end
            end
        "#,
    );

    handle(&mut runtime, &ScriptEvent::server_tick(10));
    handle(&mut runtime, &ScriptEvent::server_started());
    assert!(
        handle(&mut runtime, &ScriptEvent::server_tick(12))
            .commands()
            .is_empty()
    );
    assert_eq!(
        broadcast_messages(handle(&mut runtime, &ScriptEvent::server_tick(13))),
        ["repeat:13:13"]
    );
    assert_eq!(
        broadcast_messages(handle(&mut runtime, &ScriptEvent::server_tick(15))),
        ["repeat:15:15"]
    );
}

#[test]
fn timer_callbacks_are_ordered_and_bounded_per_pushed_tick() {
    let mut runtime = runtime(
        r#"
            function on_server_started(_event)
                for index = 9, 1, -1 do
                    solaris.schedule_timer(string.format("timer-%02d", index), 1)
                end
            end

            function on_plugin_timer(event)
                solaris.broadcast(event.timer_id .. ":" .. event.fired_tick)
            end
        "#,
    );

    handle(&mut runtime, &ScriptEvent::server_started());
    assert_eq!(
        broadcast_messages(handle(&mut runtime, &ScriptEvent::server_tick(1))),
        [
            "timer-01:1",
            "timer-02:1",
            "timer-03:1",
            "timer-04:1",
            "timer-05:1",
            "timer-06:1",
            "timer-07:1",
            "timer-08:1",
        ]
    );
    assert_eq!(
        broadcast_messages(handle(&mut runtime, &ScriptEvent::server_tick(2))),
        ["timer-09:2"]
    );
}

#[test]
fn timer_api_rejects_invalid_inputs_and_capacity_without_partial_replacement() {
    let mut runtime = runtime(
        r#"
            function on_server_started(_event)
                for index = 1, 256 do
                    solaris.schedule_timer("timer-" .. index, index)
                end
                assert(not pcall(function() solaris.schedule_timer("overflow", 1) end))
                assert(not pcall(function() solaris.schedule_timer("", 1) end))
                assert(not pcall(function() solaris.schedule_timer(string.rep("x", 65), 1) end))
                assert(not pcall(function() solaris.schedule_timer("zero", 0) end))
                assert(not pcall(function() solaris.schedule_timer("negative", -1) end))
                assert(not pcall(function() solaris.schedule_timer("float", 1.5) end))
                assert(solaris.schedule_timer("timer-1", 2) == 2)
            end
        "#,
    );

    assert!(
        handle(&mut runtime, &ScriptEvent::server_started())
            .commands()
            .is_empty()
    );
    assert!(
        handle(&mut runtime, &ScriptEvent::server_tick(1))
            .commands()
            .is_empty()
    );
    assert_eq!(runtime.pending_timer_count(), 256);
}

#[test]
fn stale_tick_does_not_move_timer_clock_backwards_or_repeat_a_callback() {
    let mut runtime = runtime(
        r#"
            function on_server_started(_event)
                assert(solaris.schedule_timer("once", 2) == 12)
            end
            function on_plugin_timer(event)
                solaris.broadcast(event.timer_id)
            end
        "#,
    );

    handle(&mut runtime, &ScriptEvent::server_tick(10));
    handle(&mut runtime, &ScriptEvent::server_started());
    assert!(
        handle(&mut runtime, &ScriptEvent::server_tick(9))
            .commands()
            .is_empty()
    );
    assert_eq!(
        broadcast_messages(handle(&mut runtime, &ScriptEvent::server_tick(12))),
        ["once"]
    );
    assert!(
        handle(&mut runtime, &ScriptEvent::server_tick(12))
            .commands()
            .is_empty()
    );
}

#[test]
fn earlier_due_callback_can_cancel_a_later_timer_due_on_the_same_tick() {
    let mut runtime = runtime(
        r#"
            function on_server_started(_event)
                solaris.schedule_timer("a", 1)
                solaris.schedule_timer("b", 1)
            end
            function on_plugin_timer(event)
                solaris.broadcast(event.timer_id)
                if event.timer_id == "a" then
                    assert(solaris.cancel_timer("b") == true)
                end
            end
        "#,
    );

    handle(&mut runtime, &ScriptEvent::server_started());
    assert_eq!(
        broadcast_messages(handle(&mut runtime, &ScriptEvent::server_tick(1))),
        ["a"]
    );
}

#[test]
fn failed_handler_discards_its_staged_timer_changes() {
    let mut runtime = runtime(
        r#"
            function on_server_started(_event)
                solaris.schedule_timer("must-not-survive", 1)
                error("handler failed")
            end
        "#,
    );

    let controls = RuntimeControls::unrestricted();
    assert!(
        runtime
            .handle_event(
                &ScriptEvent::server_started(),
                RuntimeContext::new(&controls, NonZeroUsize::new(COMMANDS_PER_EVENT).unwrap()),
            )
            .is_err()
    );
    assert_eq!(runtime.pending_timer_count(), 0);
}

#[test]
fn timer_callbacks_share_one_instruction_budget_for_the_pushed_tick() {
    let manifest = || {
        ScriptPluginManifest::new("timers", "Timers", "0.1.0", SCRIPT_API_VERSION)
            .subscribe_event("server.started")
            .validate()
            .unwrap()
    };
    let limits = LuaRuntimeLimits {
        instructions_per_event: NonZeroU64::new(10_000).unwrap(),
        ..LuaRuntimeLimits::default()
    };
    let mut single = LuaScriptRuntime::from_source(
        manifest(),
        r#"
            function on_server_started(_event)
                solaris.schedule_timer("a", 1)
            end
            function on_plugin_timer(_event)
                local total = 0
                for index = 1, 3000 do
                    total = total + index
                end
                assert(total > 0)
            end
        "#,
        limits,
    )
    .unwrap();
    handle(&mut single, &ScriptEvent::server_started());
    assert!(
        handle(&mut single, &ScriptEvent::server_tick(1))
            .commands()
            .is_empty()
    );

    let mut runtime = LuaScriptRuntime::from_source(
        manifest(),
        r#"
            function on_server_started(_event)
                solaris.schedule_timer("a", 1)
                solaris.schedule_timer("b", 1)
            end
            function on_plugin_timer(_event)
                local total = 0
                for index = 1, 3000 do
                    total = total + index
                end
                assert(total > 0)
            end
        "#,
        limits,
    )
    .unwrap();
    handle(&mut runtime, &ScriptEvent::server_started());

    let controls = RuntimeControls::unrestricted();
    let error = runtime
        .handle_event(
            &ScriptEvent::server_tick(1),
            RuntimeContext::new(&controls, NonZeroUsize::new(COMMANDS_PER_EVENT).unwrap()),
        )
        .unwrap_err();
    assert!(matches!(
        error,
        RuntimeError::Trap { message } if message.contains("instruction budget exceeded")
    ));
    assert!(
        handle(&mut runtime, &ScriptEvent::server_tick(2))
            .commands()
            .is_empty()
    );
}
