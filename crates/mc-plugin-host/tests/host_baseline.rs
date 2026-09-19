//! The P0 baseline: what one callback, one compile and one hosted instance cost
//! on a known machine, recorded as raw numbers so P7 can compare the ported
//! packages against them.
//!
//! These are *measurements*, not contracts. Nothing here asserts that a callback
//! is fast or that an instance is small: a bound belongs in `PluginLimits`, where
//! it is enforced and where it must be calibrated from numbers like these plus two
//! real plugins, which has not happened yet. The latencies are asserted only to be
//! well formed (ascending, finite) and the run only to have completed every call,
//! so the test fails when the *measurement* is broken, never when the machine is
//! slow.
//!
//! The measured path is the public host API over the real fixture: a component
//! built by the SDK from `sdk/rust/examples/hello`, compiled with
//! [`CompiledPlugin::compile`], its startup phase run in the short-lived store
//! [`PluginStartup`] owns, and its runtime store driven through `on_events` with
//! one `PlayerJoined` event per call - the smallest realistic batch, and the shape
//! of most deliveries a busy server makes. The deployment path in `src/host.rs` is
//! deliberately not on the measured path; the test prints what that leaves
//! unreported.

mod fixture;

use std::time::{Duration, Instant};

use mc_plugin_host::bindings::exports::solaris::plugin::events::{
    Event, EventContext, PlayerJoined,
};
use mc_plugin_host::bindings::exports::solaris::plugin::lifecycle::InitContext;
use mc_plugin_host::{
    CompiledPlugin, HostServices, LogLevel, PluginInstance, PluginLimits, PluginStartup, engine,
    linker,
};

/// The plugin id the baseline instance is bound to.
const PLUGIN_ID: &str = "hello";

/// The contract version the fixture artifact is compiled as.
const API_VERSION: &str = "0.7.0";

/// The player the baseline batch describes. A fixed id, so the guest lifts and
/// matches the same string every iteration.
const PLAYER: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// The player's session number, as the server would hand it over.
const SESSION: u64 = 7;

/// The player's display name.
const PLAYER_NAME: &str = "Ada";

/// `config.toml` the instance is configured with: the fixture's own key, so the
/// run goes through the plugin's configured path rather than a special one, and
/// no `mode` key, because a mode is a fault injection, not a workload.
const CONFIG: &str = "greeting = \"Hi there\"\n";

/// Timed callbacks. Past the 200 the plan asks for, so the tail quantiles rest on
/// more than a couple of samples.
const ITERATIONS: usize = 250;

/// Untimed callbacks before the measured run. The first calls into a fresh guest
/// fault in its linear memory and grow the host's staging buffers; that cost
/// belongs to the sequence (one instance starting), not to a steady instance
/// answering its thousandth event.
const WARMUP: usize = 10;

/// The services the fixture is given: it logs, and the baseline only counts the
/// lines, so writing diagnostics cannot dominate a measurement of the guest.
#[derive(Default)]
struct Services {
    id: String,
    lines: u64,
}

impl HostServices for Services {
    fn log(&mut self, _level: LogLevel, _message: &str) {
        self.lines += 1;
    }

    fn plugin_id(&self) -> &str {
        &self.id
    }
}

/// The process's peak resident set, in KiB, as the kernel reports it.
///
/// `VmHWM` is a high-water mark: it never falls, so a reading taken after hosting
/// an instance is the peak the process reached *while* hosting it, and no later
/// reading can be smaller. That is what a deployment's footprint has to be
/// compared against - and why this file reports two samples instead of a
/// difference to be read as "what one instance costs".
fn peak_rss_kib() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("the process has a status");
    let line = status
        .lines()
        .find(|line| line.starts_with("VmHWM:"))
        .expect("Linux reports VmHWM");
    let kib = line
        .split_whitespace()
        .nth(1)
        .expect("VmHWM carries a value");
    kib.parse().expect("VmHWM is a number of KiB")
}

/// The nearest-rank quantile of an ascending sample, in microseconds.
///
/// Rank `ceil(percent * n / 100)`, index `rank - 1`, clamped into the sample: a
/// real observed latency, never an interpolation between two of them. An
/// interpolated "p99" of 250 samples would be a number no guest ever took, and
/// the point of the baseline is that every figure here was taken by this host on
/// this machine.
fn nearest_rank_us(sorted: &[Duration], percent: usize) -> f64 {
    let n = sorted.len();
    let rank = (percent * n).div_ceil(100);
    sorted[rank.saturating_sub(1).min(n - 1)].as_secs_f64() * 1e6
}

/// One event: the player joined, which is what a server delivers most often and
/// what the fixture answers with exactly one message.
fn joined(player: &str, session: u64, name: &str) -> Event {
    Event::PlayerJoined(PlayerJoined {
        player: player.to_owned(),
        session,
        name: name.to_owned(),
    })
}

/// The context one batch is delivered under: a single event of a single tick.
fn joined_context() -> EventContext {
    EventContext {
        tick: 42,
        first_sequence: 0,
        count: 1,
    }
}

#[test]
fn the_p0_baseline_reports_what_a_callback_compile_and_instance_cost() {
    // The first sample is taken before this process has touched a component, so
    // the second one is the peak that hosting an instance added.
    let peak_before_kib = peak_rss_kib();
    assert!(peak_before_kib > 0, "a live process has a peak RSS");

    let bytes = fixture::component_bytes();
    let limits = PluginLimits::default();
    let engine = engine(&limits).expect("the host engine builds");

    // Compilation is host work a guest's fuel never pays for, so it is measured
    // on its own rather than folded into instantiation.
    let compile_started = Instant::now();
    let compiled = CompiledPlugin::compile(&engine, &bytes, &limits, API_VERSION)
        .expect("the fixture compiles as a component");
    let compile_ms = compile_started.elapsed().as_secs_f64() * 1e3;
    assert_eq!(
        compiled.api_version(),
        API_VERSION,
        "the compiled artifact carries the contract version it was compiled as"
    );

    let linker = linker::<Services>(&engine).expect("the contract's linker builds");
    // The startup phase first, in the store of its own the host drops before the
    // runtime store exists - the same two-phase sequence `start_deployment_with`
    // and `check_deployment` run.
    let contribution = PluginStartup::instantiate(
        &linker,
        compiled.component(),
        Services {
            id: PLUGIN_ID.to_owned(),
            ..Services::default()
        },
        limits,
    )
    .expect("the startup store instantiates under the default limits")
    .configure(CONFIG)
    .expect("the fixture's configure succeeds");
    assert!(
        contribution.is_none(),
        "the example plugin answers no startup contribution"
    );
    let mut plugin = PluginInstance::instantiate(
        &linker,
        compiled.component(),
        Services {
            id: PLUGIN_ID.to_owned(),
            ..Services::default()
        },
        limits,
    )
    .expect("the runtime store instantiates under the default limits");
    plugin
        .init(
            CONFIG,
            InitContext {
                plugin_id: PLUGIN_ID.to_owned(),
                api_version: API_VERSION.to_owned(),
                world_fingerprint: String::new(),
            },
        )
        .expect("the fixture's init succeeds");

    let context = joined_context();
    let events = [joined(PLAYER, SESSION, PLAYER_NAME)];
    for _ in 0..WARMUP {
        plugin
            .on_events(context, &events)
            .expect("the fixture answers a warm-up event");
    }

    let mut latencies = Vec::with_capacity(ITERATIONS);
    let mut last = None;
    let run_started = Instant::now();
    for _ in 0..ITERATIONS {
        // The context a delivery carries is built by the caller, so it is built
        // here and outside the timer: what is measured is the callback, not the
        // harness.
        let delivered = context;
        let started = Instant::now();
        let batch = plugin
            .on_events(delivered, &events)
            .expect("the fixture answers one event per call");
        latencies.push(started.elapsed());
        // Every batch but the last is dropped unread: staging the answer is what
        // a callback costs the host, and reading it back is the deployment's own
        // work. The last one is kept to prove the answer was the fixture's.
        last = Some(batch);
    }
    let run_secs = run_started.elapsed().as_secs_f64();

    // The last sample is taken while the instance is still live, so it is the
    // peak of a process that is hosting exactly this instance.
    let peak_after_kib = peak_rss_kib();

    latencies.sort_unstable();
    let p50 = nearest_rank_us(&latencies, 50);
    let p95 = nearest_rank_us(&latencies, 95);
    let p99 = nearest_rank_us(&latencies, 99);
    let min = latencies[0].as_secs_f64() * 1e6;
    let max = latencies[latencies.len() - 1].as_secs_f64() * 1e6;

    // The invariants: the sample is well formed, and every call happened and was
    // answered. The measured values themselves are not asserted - a slow machine
    // is not a broken host.
    assert!(
        min.is_finite() && p50.is_finite() && p95.is_finite() && p99.is_finite() && max.is_finite(),
        "every reported latency is a finite number of microseconds"
    );
    assert!(
        min <= p50 && p50 <= p95 && p95 <= p99 && p99 <= max,
        "an ascending sample answers ascending quantiles"
    );
    assert!(peak_after_kib >= peak_before_kib, "peak RSS never falls");
    assert!(
        plugin.retired_because().is_none(),
        "the fixture answered every call rather than being retired"
    );
    assert_eq!(
        plugin.state().calls(),
        (1 + WARMUP + ITERATIONS) as u64,
        "init, the warm-up calls and the measured calls are every call this store made: \
         the startup phase ran in its own, already dropped store"
    );
    assert_eq!(
        plugin.state().log_lines_dropped(),
        0,
        "the fixture stays inside the per-call log bound"
    );
    let commands = last
        .expect("the measured run called the guest")
        .into_commands();
    assert_eq!(commands.len(), 1, "one join event answers one command");
    // Read before the shutdown call, which is itself a callback and would show up
    // in the counter.
    let calls_measured = plugin.state().calls();
    let log_lines_dropped = plugin.state().log_lines_dropped();
    plugin.shutdown().expect("the fixture shuts down");
    let calls_hosted = plugin.state().calls();
    let guest_log_lines = plugin.state().services().lines;

    println!("--- P0 host baseline (mc-plugin-host) ---");
    println!(
        "guest: fixture component, {} bytes, contract {API_VERSION}",
        bytes.len()
    );
    println!(
        "callback: on_events, 1 PlayerJoined event, {ITERATIONS} iterations after {WARMUP} warm-up calls"
    );
    println!("  p50  {p50:9.3} us");
    println!("  p95  {p95:9.3} us");
    println!("  p99  {p99:9.3} us");
    println!("  min  {min:9.3} us");
    println!("  max  {max:9.3} us");
    println!("  measured run: {run_secs:.3} s");
    println!("compile: CompiledPlugin::compile, one measurement");
    println!("  {compile_ms:9.3} ms");
    println!("rss: /proc/self/status VmHWM, KiB (a high-water mark, it never falls)");
    println!("  before hosting: {peak_before_kib}");
    println!("  after hosting:  {peak_after_kib}");
    println!(
        "limits this run used: fuel_per_call {} | epoch_ticks_per_call {} | hostcall_bytes {} | guest_memory_bytes {} | commands_per_call {}",
        limits.fuel_per_call,
        limits.epoch_ticks_per_call,
        limits.hostcall_bytes,
        limits.guest_memory_bytes,
        limits.commands_per_call
    );
    println!(
        "tick: EPOCH_INTERVAL is {} ms, and every callback deadline is armed in units of it",
        mc_plugin_host::host::EPOCH_INTERVAL.as_millis()
    );
    println!(
        "counters reachable without a deployment directory: calls {calls_measured} through the measured run, {calls_hosted} with the shutdown call | staged commands in the last answer {} | guest log lines through the host import {} | log_lines_dropped {}",
        commands.len(),
        guest_log_lines,
        log_lines_dropped
    );
    println!(
        "not reported: InstanceDiagnostics (events_delivered, commands_submitted, commands_refused). It is the deployment's own accounting in src/host.rs and is only reachable by starting a deployment - a package directory, the host thread and the boundary - which this baseline deliberately does not do; the deployment tests cover those counters."
    );
}
