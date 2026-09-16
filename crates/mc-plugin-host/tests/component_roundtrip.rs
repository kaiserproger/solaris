//! P0 acceptance: a real component, built from the SDK, runs in the host.
//!
//! The fixture is a real Rust plugin (`sdk/rust/examples/hello`) compiled to
//! `wasm32-unknown-unknown` and encoded into a component with `wit-component`,
//! exactly the way a published package is built. Nothing here hand-writes a
//! module: if the WIT, the SDK and the host disagree by one type, this test does
//! not compile the guest or does not instantiate it.
//!
//! The cases cover what the plan requires of the prototype: real component bytes
//! load; a raw core module (no component types) is rejected before any guest code
//! runs; the artifact bound is enforced before compilation; and a guest that
//! never returns, or allocates without bound, is stopped by its budget instead of
//! taking the host with it.

mod fixture;

use std::path::PathBuf;

use fixture::component_bytes;
use mc_plugin_host::bindings::exports::solaris::plugin::events::{
    Event, EventContext, PlayerJoined,
};
use mc_plugin_host::bindings::exports::solaris::plugin::lifecycle::InitContext;
use mc_plugin_host::bindings::solaris::plugin::commands::{Command, MessageTarget};
use mc_plugin_host::{
    HostError, HostServices, LogLevel, PluginInstance, PluginLimits, engine, linker,
};

/// The services the fixture plugin is given: it logs, and it has an id.
#[derive(Default)]
struct Services {
    id: String,
    lines: Vec<(LogLevel, String)>,
}

impl HostServices for Services {
    fn log(&mut self, level: LogLevel, message: &str) {
        self.lines.push((level, message.to_owned()));
    }

    fn plugin_id(&self) -> &str {
        &self.id
    }
}

/// The path of the core module the shared fixture encodes into a component.
///
/// A raw module carries no component types, which is exactly what a published
/// package must never be. The build belongs to the shared fixture, so asking for
/// the component first is what guarantees the module it was encoded from is on
/// disk.
fn guest_module() -> PathBuf {
    let _ = component_bytes();
    let sdk = fixture::repo_root().join("sdk/rust");
    sdk.join("target/wasm32-unknown-unknown/release/solaris_hello_plugin.wasm")
}

/// Instantiate the example plugin with `config` as its `config.toml` text.
fn instance(config: &str, limits: PluginLimits) -> PluginInstance<Services> {
    let bytes = component_bytes();
    let engine = engine(&limits).expect("engine");
    let compiled = mc_plugin_host::CompiledPlugin::compile(&engine, &bytes, &limits, "0.7.0")
        .expect("compile");
    let linker = linker::<Services>(&engine).expect("linker");
    let mut instance = PluginInstance::instantiate(
        &linker,
        compiled.component(),
        Services {
            id: "hello".to_owned(),
            ..Services::default()
        },
        limits,
    )
    .expect("instantiate");
    assert!(
        instance.configure(config).expect("configure").is_none(),
        "the example plugin answers no rule plan"
    );
    instance
}

#[test]
fn a_real_component_answers_a_join_and_a_command() {
    let limits = PluginLimits::default();
    let mut plugin = instance("greeting = \"Hi there\"\n", limits);
    plugin
        .init(
            "greeting = \"Hi there\"\n",
            InitContext {
                plugin_id: "hello".to_owned(),
                api_version: "0.7.0".to_owned(),
                world_fingerprint: String::new(),
            },
        )
        .expect("init");

    let context = EventContext {
        tick: 42,
        first_sequence: 0,
        count: 2,
    };
    let events = vec![
        Event::PlayerJoined(PlayerJoined {
            player: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_owned(),
            session: 7,
            name: "Ada".to_owned(),
        }),
        Event::CommandInvoked(
            mc_plugin_host::bindings::exports::solaris::plugin::events::CommandInvoked {
                player: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_owned(),
                session: 7,
                name: "hello".to_owned(),
                arguments: Vec::new(),
            },
        ),
    ];
    let batch = plugin.on_events(context, &events).expect("on-events");
    let staged = batch.into_commands();
    assert_eq!(staged.len(), 2, "one greeting and one command answer");

    let texts = staged
        .iter()
        .map(|command| match command {
            Command::SendMessage(message) => {
                match &message.target {
                    MessageTarget::Player(player) => assert_eq!(
                        player, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
                        "both answers address the player who caused them"
                    ),
                    other => panic!("expected a player answer, saw {other:?}"),
                }
                message.text.clone()
            }
            other => panic!("expected the fixture's greeting, saw {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        texts,
        vec![
            "Hi there Ada".to_owned(),
            "Hello from a WASM plugin.".to_owned()
        ],
        "the configured greeting and the command answer reach the host verbatim"
    );

    let lines = plugin.state().services().lines.clone();
    assert!(
        lines.iter().any(|(level, line)| *level == LogLevel::Info
            && (line.contains("ready") || line.contains("stopping"))),
        "the guest's own log lines arrive through the host import, saw {lines:?}"
    );
    assert!(
        plugin.retired_because().is_none(),
        "the instance stayed live"
    );
    plugin.shutdown().expect("shutdown");
    assert_eq!(
        plugin.state().calls(),
        4,
        "configure, init, on-events, shutdown"
    );
}

#[test]
fn a_raw_core_module_is_rejected_before_any_guest_code_runs() {
    let limits = PluginLimits::default();
    let engine = engine(&limits).expect("engine");
    let bytes = std::fs::read(guest_module()).expect("the guest module exists");
    let error = match mc_plugin_host::CompiledPlugin::compile(&engine, &bytes, &limits, "0.7.0") {
        Ok(_) => panic!("a core module must not compile as a component"),
        Err(error) => error,
    };
    assert!(
        matches!(error, HostError::Component(_)),
        "the refusal names the artifact, not a guest failure: {error}"
    );
}

#[test]
fn the_artifact_bound_is_enforced_before_compilation() {
    let limits = PluginLimits {
        artifact_bytes: 64,
        ..PluginLimits::default()
    };
    let engine = engine(&limits).expect("engine");
    let error =
        match mc_plugin_host::CompiledPlugin::compile(&engine, &[0_u8; 4096], &limits, "0.7.0") {
            Ok(_) => panic!("an oversized artifact must be refused"),
            Err(error) => error,
        };
    assert!(matches!(error, HostError::Component(_)), "{error}");
}

#[test]
fn a_guest_that_never_returns_is_stopped_by_its_budget() {
    // Four epoch ticks, each 50 ms, is the wall-clock watchdog of one call.
    let limits = PluginLimits {
        epoch_ticks_per_call: 4,
        ..PluginLimits::default()
    };
    let bytes = component_bytes();
    // The instance must run in the engine the ticker advances: the watchdog is a
    // thread of its own, never the worker that is stuck inside the guest.
    let engine = engine(&limits).expect("engine");
    let ticker =
        mc_plugin_host::EpochTicker::start(engine.clone(), std::time::Duration::from_millis(50))
            .expect("watchdog");
    let compiled = mc_plugin_host::CompiledPlugin::compile(&engine, &bytes, &limits, "0.7.0")
        .expect("compile");
    let linker = linker::<Services>(&engine).expect("linker");
    let mut plugin =
        PluginInstance::instantiate(&linker, compiled.component(), Services::default(), limits)
            .expect("instantiate");

    let config = "mode = \"spin\"\n";
    plugin.configure(config).expect("configure");
    let start = std::time::Instant::now();
    let error = plugin
        .init(
            config,
            InitContext {
                plugin_id: "hello".to_owned(),
                api_version: "0.7.0".to_owned(),
                world_fingerprint: String::new(),
            },
        )
        .expect_err("a guest that never returns is stopped");
    let elapsed = start.elapsed();
    assert!(
        matches!(error, HostError::Budget),
        "the watchdog reports a budget, saw {error}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "the watchdog stopped the guest promptly, took {elapsed:?}"
    );
    assert!(
        plugin.retired_because().is_some(),
        "a stopped guest is retired, never called again"
    );
    drop(ticker);
}

#[test]
fn a_guest_that_grows_without_bound_hits_its_memory_limit() {
    let limits = PluginLimits {
        guest_memory_bytes: 16 * 1024 * 1024,
        epoch_ticks_per_call: 4,
        ..PluginLimits::default()
    };
    let bytes = component_bytes();
    let engine = engine(&limits).expect("engine");
    let ticker =
        mc_plugin_host::EpochTicker::start(engine.clone(), std::time::Duration::from_millis(50))
            .expect("watchdog");
    let compiled = mc_plugin_host::CompiledPlugin::compile(&engine, &bytes, &limits, "0.7.0")
        .expect("compile");
    let linker = linker::<Services>(&engine).expect("linker");
    let mut plugin =
        PluginInstance::instantiate(&linker, compiled.component(), Services::default(), limits)
            .expect("instantiate");

    let config = "mode = \"grow\"\n";
    plugin.configure(config).expect("configure");
    let error = plugin
        .init(
            config,
            InitContext {
                plugin_id: "hello".to_owned(),
                api_version: "0.7.0".to_owned(),
                world_fingerprint: String::new(),
            },
        )
        .expect_err("a guest that allocates without bound is stopped");
    assert!(
        matches!(error, HostError::Trap(_) | HostError::Budget),
        "the store's memory limit stops the allocation, saw {error}"
    );
    drop(ticker);
}

#[test]
fn a_trapped_guest_does_not_disturb_a_live_one() {
    // Two instances of the same component in one engine, sharing nothing but the
    // compiled artifact: when one is retired by its own budget, the other must go
    // on answering callbacks. This is what lets the composition root host a
    // deployment of many packages without one bad guest taking the rest down.
    let limits = PluginLimits {
        epoch_ticks_per_call: 4,
        ..PluginLimits::default()
    };
    let bytes = component_bytes();
    let engine = engine(&limits).expect("engine");
    let ticker =
        mc_plugin_host::EpochTicker::start(engine.clone(), std::time::Duration::from_millis(50))
            .expect("watchdog");
    let compiled = mc_plugin_host::CompiledPlugin::compile(&engine, &bytes, &limits, "0.7.0")
        .expect("compile");
    let linker = linker::<Services>(&engine).expect("linker");
    let services = || Services {
        id: "hello".to_owned(),
        ..Services::default()
    };
    let context = InitContext {
        plugin_id: "hello".to_owned(),
        api_version: "0.7.0".to_owned(),
        world_fingerprint: String::new(),
    };
    let mut healthy =
        PluginInstance::instantiate(&linker, compiled.component(), services(), limits)
            .expect("healthy instance");
    healthy.configure("greeting = \"Hi\"\n").expect("configure");
    healthy
        .init("greeting = \"Hi\"\n", context.clone())
        .expect("the healthy instance starts");

    let mut doomed = PluginInstance::instantiate(&linker, compiled.component(), services(), limits)
        .expect("second instance");
    doomed.configure("mode = \"spin\"\n").expect("configure");
    let error = doomed
        .init("mode = \"spin\"\n", context)
        .expect_err("the spinning instance is stopped");
    assert!(matches!(error, HostError::Budget), "{error}");
    assert!(
        doomed.retired_because().is_some(),
        "it is retired, not reused"
    );

    let batch = healthy
        .on_events(
            EventContext {
                tick: 7,
                first_sequence: 0,
                count: 1,
            },
            &[Event::PlayerJoined(PlayerJoined {
                player: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_owned(),
                session: 3,
                name: "Ada".to_owned(),
            })],
        )
        .expect("the live instance still answers");
    assert_eq!(batch.len(), 1, "the greeting of the surviving instance");
    assert!(healthy.retired_because().is_none());
    drop(ticker);
}
