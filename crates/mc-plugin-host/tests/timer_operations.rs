//! Real components schedule, replace and cancel their own timers.
//!
//! Every assertion below is a line a guest published after the host delivered a
//! simulation tick, or a diagnostic the host kept: nothing reads the guest's own
//! bookkeeping, nothing sleeps, and nothing polls. The guest renders each fire as
//! `<timer-id>:<scheduled-tick>:<fired-tick>`, so a fire the host deferred and the
//! deadline it kept are both visible, and a scenario that must publish nothing is
//! proven by the closed command queue rather than by silence.
//!
//! A pushed `ScriptEvent::server_tick` is the only clock: the host stamps each
//! callback with the last tick it observed, which is why every scenario pushes its
//! ticks explicitly and then anchors a deadline to one with a join.

mod fixture;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use mc_plugin_host::{
    DeploymentConfig, DiscoveryMode, HostQueues, InstanceDiagnostics, NoSessions, PluginLimits,
    start_deployment,
};
use mc_script::{ScriptBoundary, ScriptCommand, ScriptEvent, ScriptPlayerContext, ScriptPlayerId};

const PLAYER_UUID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// The contract's bound on one callback's batch, and so the most timer slots one
/// fill callback can schedule. The capacity scenarios fill 256 slots over the
/// joins this implies.
const SLOTS_PER_BATCH: usize = 32;

/// One event a scenario states, in the order the host will see it.
enum Beat {
    /// Push the simulation tick a timer is measured against.
    Tick(u64),
    /// One player join, which the host stamps with the last pushed tick.
    Join(u64),
}

/// One deployment of `rows` - a plugin id, the fixture script it runs and the
/// `count` that script uses - driven by `beats`.
///
/// The whole event program is enqueued before anything is read: the host
/// processes it in order, so a test asserts the exact lines that program
/// produced instead of racing the worker.
async fn drive(
    rows: &[(&str, &str, usize)],
    limits: PluginLimits,
    queues: HostQueues,
    beats: &[Beat],
) -> (Vec<(String, String)>, Vec<(String, InstanceDiagnostics)>) {
    let root = tempfile::tempdir().unwrap();
    for (id, script, count) in rows {
        write_package(root.path(), id, script, *count);
    }
    let packages = mc_plugin_host::discover(
        &DeploymentConfig {
            root: root.path().to_path_buf(),
            mode: DiscoveryMode::Strict,
            expected: rows.iter().map(|(id, _, _)| (*id).to_owned()).collect(),
            grants: BTreeMap::new(),
            require_grants: false,
            precommit_hooks: Vec::new(),
        },
        &limits,
    )
    .unwrap()
    .into_packages();
    let host = start_deployment(packages, limits, queues, Arc::new(NoSessions)).unwrap();
    let boundary = host.boundary().clone();
    for beat in beats {
        match beat {
            Beat::Tick(tick) => boundary
                .try_enqueue_event(ScriptEvent::server_tick(*tick))
                .unwrap(),
            Beat::Join(session) => boundary
                .try_enqueue_event(ScriptEvent::player_joined_with_context(
                    ScriptPlayerId::new(*session),
                    ScriptPlayerContext::try_new(PLAYER_UUID, "Ada", false, 0.0, 64.0, 0.0)
                        .unwrap(),
                ))
                .unwrap(),
        }
    }
    let lines = published(&boundary).await;
    (lines, host.stop())
}

/// Every line a guest published, in delivery order, each with the plugin that
/// answered it.
///
/// Admission is closed first: only that closure, never elapsed silence, proves
/// that a scenario which must publish nothing published nothing.
async fn published(boundary: &ScriptBoundary) -> Vec<(String, String)> {
    boundary.close_event_admission();
    let mut lines = Vec::new();
    while let Some(command) = tokio::time::timeout(Duration::from_secs(10), boundary.recv_command())
        .await
        .expect("the host drains its command queue")
    {
        let admitted = boundary
            .accept_host_command(command)
            .expect("every published command is host-attached");
        let ScriptCommand::BroadcastChatMessage { message } = admitted.request() else {
            panic!(
                "timer scenarios publish only broadcast lines, got {:?}",
                admitted.request()
            );
        };
        lines.push((admitted.plugin_id().to_owned(), message.clone()));
    }
    lines
}

/// The lines a test expects, as owned pairs.
fn expected(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(plugin, line)| ((*plugin).to_owned(), (*line).to_owned()))
        .collect()
}

/// The joins a capacity scenario needs to fill `count` slots, one batch each.
fn fill(count: usize) -> Vec<Beat> {
    (0..count.div_ceil(SLOTS_PER_BATCH))
        .map(|index| Beat::Join(index as u64 + 1))
        .collect()
}

/// Write one package whose guest runs one timer scenario.
fn write_package(root: &Path, id: &str, script: &str, count: usize) {
    let directory = root.join(id);
    std::fs::create_dir(&directory).unwrap();
    // No `server.tick` and no timer event is subscribed: a timer fires from the
    // ticks the server already pushes, so a package needs no subscription for
    // them. The join is here only because it is how a test anchors a deadline to
    // a tick it chose.
    std::fs::write(
        directory.join("plugin.toml"),
        format!(
            "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\napi = \"0.7.0\"\n\
             events = [\"player.joined\"]\n"
        ),
    )
    .unwrap();
    std::fs::write(directory.join("plugin.wasm"), fixture::component_bytes()).unwrap();
    std::fs::write(
        directory.join("config.toml"),
        format!("greeting = \"{id}\"\nmode = \"timers\"\nscript = \"{script}\"\ncount = {count}\n"),
    )
    .unwrap();
}

/// The counters of one plugin, which every test here asserts on by id.
fn plugin_counters<'a>(
    diagnostics: &'a [(String, InstanceDiagnostics)],
    id: &str,
) -> &'a InstanceDiagnostics {
    &diagnostics
        .iter()
        .find(|(plugin, _)| plugin == id)
        .expect("the deployment's own plugin")
        .1
}

#[tokio::test]
async fn a_timer_fires_on_the_first_pushed_tick_past_its_deadline_and_never_twice() {
    let beats = [
        Beat::Tick(10),
        Beat::Join(1),
        // A second push of the same tick and a stale one move no timer: the
        // observed tick is the maximum the host has seen.
        Beat::Tick(11),
        Beat::Tick(11),
        Beat::Tick(9),
    ];
    let (lines, _) = drive(
        &[("hello", "probe", 0)],
        PluginLimits::default(),
        HostQueues::default(),
        &beats,
    )
    .await;
    assert_eq!(
        lines,
        expected(&[
            // The init timer was scheduled with no tick pushed yet, so it was due
            // one tick in and fired at the first tick the server pushed.
            ("hello", "probe-init:1:10"),
            ("hello", "probe-join-1:11:11"),
        ])
    );
}

#[tokio::test]
async fn a_replaced_deadline_supersedes_the_original_and_a_cancelled_timer_never_fires() {
    let beats = [
        Beat::Tick(10),
        Beat::Join(1),
        // The cancelled timer was due at 12 and the replaced deadline at 15.
        Beat::Tick(12),
        Beat::Tick(13),
        Beat::Tick(15),
        Beat::Tick(13),
    ];
    let (lines, diagnostics) = drive(
        &[("hello", "replace", 0)],
        PluginLimits::default(),
        HostQueues::default(),
        &beats,
    )
    .await;
    // One line: the replacement moved the deadline to 13 and the original 15
    // never existed any more, the cancelled id never fired, and cancelling an id
    // the guest never scheduled refused nothing.
    assert_eq!(lines, expected(&[("hello", "repeat:13:13")]));
    assert_eq!(plugin_counters(&diagnostics, "hello").commands_submitted, 1);
}

#[tokio::test]
async fn an_earlier_due_callback_cancels_a_later_one_due_on_the_same_tick() {
    let beats = [
        Beat::Tick(20),
        Beat::Join(1),
        Beat::Tick(21),
        Beat::Tick(22),
    ];
    let (lines, _) = drive(
        &[("hello", "same-tick", 0)],
        PluginLimits::default(),
        HostQueues::default(),
        &beats,
    )
    .await;
    // `a-keep` sorts before `b-cancel` at the same deadline, so it ran first and
    // its own batch cancelled the timer the host had not fired yet.
    assert_eq!(lines, expected(&[("hello", "a-keep:21:21")]));
}

#[tokio::test]
async fn a_callback_reschedules_from_the_tick_the_host_observed_not_the_old_deadline() {
    let beats = [
        Beat::Tick(20),
        Beat::Join(1),
        // `first` was due at 21 but only runs at 25, so two ticks past the tick
        // it observed is 27 - not two ticks past its own deadline.
        Beat::Tick(25),
        Beat::Tick(26),
        Beat::Tick(27),
    ];
    let (lines, _) = drive(
        &[("hello", "chain", 0)],
        PluginLimits::default(),
        HostQueues::default(),
        &beats,
    )
    .await;
    assert_eq!(
        lines,
        expected(&[("hello", "first:21:25"), ("hello", "second:27:27")])
    );
}

#[tokio::test]
async fn one_pushed_tick_fires_at_most_eight_due_timers_and_defers_the_rest() {
    let beats = [
        Beat::Tick(40),
        Beat::Join(1),
        Beat::Tick(41),
        Beat::Tick(41),
        Beat::Tick(42),
        Beat::Tick(41),
    ];
    let (lines, _) = drive(
        &[("hello", "fanout", 9)],
        PluginLimits::default(),
        HostQueues::default(),
        &beats,
    )
    .await;
    let mut want = (0..8)
        .map(|index| ("hello".to_owned(), format!("timer-{index:02}:41:41")))
        .collect::<Vec<_>>();
    // The ninth keeps the deadline it was scheduled for and fires at the next
    // pushed tick: a deferral, not a drop.
    want.push(("hello".to_owned(), "timer-08:41:42".to_owned()));
    assert_eq!(lines, want);
}

#[tokio::test]
async fn one_pushed_tick_shares_one_command_budget_across_its_due_callbacks() {
    let beats = [
        Beat::Tick(50),
        Beat::Join(1),
        Beat::Join(2),
        Beat::Tick(51),
        Beat::Tick(52),
    ];
    let (lines, _) = drive(
        &[("hello", "budget", 8)],
        // Eight answers fit the delivery the tick makes.
        PluginLimits {
            commands_per_call: 8,
            ..PluginLimits::default()
        },
        HostQueues::default(),
        &beats,
    )
    .await;
    let want = (0..8)
        .map(|index| ("hello".to_owned(), format!("t-{index:02}:51:51")))
        .collect::<Vec<_>>();
    assert_eq!(lines, want);

    // One command less on the same number of due timers: the tick's callbacks
    // share the one bound, so the eighth answer cannot be admitted by giving each
    // callback a budget of its own.
    let (lines, _) = drive(
        &[("hello", "budget", 8)],
        PluginLimits {
            commands_per_call: 7,
            ..PluginLimits::default()
        },
        HostQueues::default(),
        &beats,
    )
    .await;
    assert_eq!(lines, Vec::<(String, String)>::new());
}

#[tokio::test]
async fn timer_mutations_count_toward_the_shared_command_budget() {
    let beats = [
        Beat::Tick(50),
        Beat::Join(1),
        Beat::Join(2),
        Beat::Tick(51),
        Beat::Tick(52),
    ];
    let (admitted, _) = drive(
        &[("hello", "timer-budget", 4)],
        PluginLimits {
            commands_per_call: 8,
            ..PluginLimits::default()
        },
        HostQueues::default(),
        &beats,
    )
    .await;
    let expected = (0..4)
        .map(|index| ("hello".to_owned(), format!("t-{index:02}:51:51")))
        .collect::<Vec<_>>();
    assert_eq!(admitted, expected);

    let (refused, _) = drive(
        &[("hello", "timer-budget", 4)],
        PluginLimits {
            commands_per_call: 7,
            ..PluginLimits::default()
        },
        HostQueues::default(),
        &beats,
    )
    .await;
    assert!(refused.is_empty(), "{refused:?}");
}

#[tokio::test]
async fn due_callbacks_share_fuel_and_exhaustion_discards_the_whole_tick() {
    let beats = [Beat::Tick(90), Beat::Join(1), Beat::Tick(91)];
    let limits = PluginLimits {
        fuel_per_call: 1_000_000,
        ..PluginLimits::default()
    };
    let (single, _) = drive(
        &[("hello", "fuel", 1)],
        limits,
        HostQueues::default(),
        &beats,
    )
    .await;
    assert_eq!(single, expected(&[("hello", "t-00:91:91")]));

    // Each callback fits by itself. Rearming fuel between them would let all
    // eight publish; the shared budget must instead discard their whole tick.
    let (combined, _) = drive(
        &[("hello", "fuel", 8)],
        limits,
        HostQueues::default(),
        &beats,
    )
    .await;
    assert!(combined.is_empty(), "{combined:?}");
}

#[tokio::test]
async fn a_tick_whose_delivery_fails_publishes_none_of_its_prefix() {
    let beats = [
        Beat::Tick(30),
        Beat::Join(1),
        Beat::Tick(31),
        Beat::Tick(32),
    ];
    let (lines, diagnostics) = drive(
        &[("hello", "trap", 0)],
        PluginLimits::default(),
        HostQueues::default(),
        &beats,
    )
    .await;
    // The first fire answered its line before the second faulted; the whole tick
    // is one transaction, so that line exists nowhere.
    assert_eq!(lines, Vec::<(String, String)>::new());
    let counters = plugin_counters(&diagnostics, "hello");
    // The tick was delivered - the join and the fire before the fault reached the
    // guest - so the empty queue is a discarded prefix, not a missing delivery.
    assert!(counters.events_delivered >= 2, "{counters:?}");
    assert_eq!(counters.commands_submitted, 0);
}

#[tokio::test]
async fn a_refused_delivery_keeps_the_guest_live_and_commits_no_timer() {
    let beats = [
        Beat::Tick(60),
        // Two lines where the queue holds one, beside a timer: the delivery is
        // refused as a whole, so the timer request goes with the lines.
        Beat::Join(1),
        Beat::Tick(61),
        Beat::Join(2),
        Beat::Tick(62),
        Beat::Tick(63),
    ];
    let (lines, diagnostics) = drive(
        &[("hello", "backpressure", 0)],
        PluginLimits::default(),
        HostQueues {
            events: 64,
            commands: 1,
        },
        &beats,
    )
    .await;
    // `held` was scheduled before the refused command in the same batch and still
    // never fired, while the guest stayed live enough to schedule and fire the
    // control timer.
    assert_eq!(lines, expected(&[("hello", "control:62:62")]));
    // The refusal is the host's own count, not an inference from the silence.
    assert!(plugin_counters(&diagnostics, "hello").commands_refused >= 1);
}

#[tokio::test]
async fn a_full_plugin_refuses_a_new_timer_and_admits_one_more_slot_below_capacity() {
    // 256 pending: one more new id is the plugin's own violation, so the request
    // is refused and nothing it asked for survives.
    let beats = capacity_beats(256, 9, true);
    let (lines, _) = drive(
        &[("hello", "capacity-new", 256)],
        PluginLimits::default(),
        HostQueues::default(),
        &beats,
    )
    .await;
    assert_eq!(lines, Vec::<(String, String)>::new());

    // 255 pending: the same request is the one that fills the last slot.
    let beats = capacity_beats(255, 9, false);
    let (lines, _) = drive(
        &[("hello", "capacity-new", 255)],
        PluginLimits::default(),
        HostQueues::default(),
        &beats,
    )
    .await;
    assert_eq!(lines, expected(&[("hello", "overflow:72:72")]));
}

/// The events a capacity scenario states: fill `count` slots, then send the
/// request under test one tick later, and - when the request is expected to be
/// refused - the batch and tick a guest that survived it would still answer, so
/// silence is a retirement rather than a quiet tick.
fn capacity_beats(count: usize, session: u64, refused: bool) -> Vec<Beat> {
    let mut beats = vec![Beat::Tick(70)];
    beats.extend(fill(count));
    // The tick a later schedule is measured against.
    beats.extend([Beat::Tick(71), Beat::Join(session), Beat::Tick(72)]);
    if refused {
        beats.extend([Beat::Join(session + 1), Beat::Tick(73)]);
    }
    beats
}

#[tokio::test]
async fn a_batch_that_fills_the_last_slot_of_a_full_plugin_publishes_none_of_itself() {
    // One slot free: the batch asks for a valid timer and one more new id, and
    // the second request fails the whole batch - the first is not committed.
    let beats = capacity_beats(255, 9, true);
    let (lines, _) = drive(
        &[("hello", "capacity-mixed", 255)],
        PluginLimits::default(),
        HostQueues::default(),
        &beats,
    )
    .await;
    assert_eq!(lines, Vec::<(String, String)>::new());

    // Two slots free: the same batch is admissible, which is what makes the
    // refusal above capacity rather than the batch's shape.
    let beats = capacity_beats(254, 9, false);
    let (lines, _) = drive(
        &[("hello", "capacity-mixed", 254)],
        PluginLimits::default(),
        HostQueues::default(),
        &beats,
    )
    .await;
    assert_eq!(
        lines,
        expected(&[("hello", "overflow:72:72"), ("hello", "probe-join:72:72")])
    );
}

#[tokio::test]
async fn a_full_plugin_still_moves_the_deadline_of_a_timer_it_already_holds() {
    let beats = capacity_beats(256, 9, false);
    let (lines, diagnostics) = drive(
        &[("hello", "capacity-replace", 256)],
        PluginLimits::default(),
        HostQueues::default(),
        &beats,
    )
    .await;
    // The fill kept every filler at a tick no test reaches; the replacement moved
    // the first one to the next tick, which needs the 256th slot to be reusable
    // by the id that already holds it.
    assert_eq!(lines, expected(&[("hello", "cap-000:72:72")]));
    assert_eq!(plugin_counters(&diagnostics, "hello").commands_submitted, 1);
}

#[tokio::test]
async fn timers_belong_to_the_plugin_that_scheduled_them_and_only_that_one() {
    let beats = [
        Beat::Tick(90),
        Beat::Join(1),
        Beat::Tick(92),
        Beat::Tick(93),
        Beat::Tick(95),
    ];
    // Both packages run the same scenario with the same timer ids, one of them
    // cancelling and replacing the ids the other also holds.
    let (mut lines, _) = drive(
        &[("alpha", "replace", 0), ("beta", "replace", 0)],
        PluginLimits::default(),
        HostQueues::default(),
        &beats,
    )
    .await;
    lines.sort();
    assert_eq!(
        lines,
        expected(&[("alpha", "repeat:93:93"), ("beta", "repeat:93:93")])
    );

    // One package faults on its own tick; the other keeps its schedule, its
    // ticks and its answers.
    let beats = [
        Beat::Tick(100),
        Beat::Join(1),
        Beat::Tick(101),
        Beat::Tick(102),
        Beat::Join(2),
        Beat::Tick(103),
    ];
    let (lines, diagnostics) = drive(
        &[("sick", "trap", 0), ("well", "probe", 0)],
        PluginLimits::default(),
        HostQueues::default(),
        &beats,
    )
    .await;
    assert_eq!(
        lines,
        expected(&[
            ("well", "probe-init:1:100"),
            ("well", "probe-join-1:101:101"),
            ("well", "probe-join-2:103:103"),
        ])
    );
    // The failed tick published nothing for the faulting package, and the
    // healthy one's later callbacks are not evidence the faulting one recovered.
    assert_eq!(plugin_counters(&diagnostics, "sick").commands_submitted, 0);
    assert_eq!(plugin_counters(&diagnostics, "well").commands_submitted, 3);
}

#[tokio::test]
async fn a_malformed_timer_request_fails_its_whole_batch_and_the_plugin_with_it() {
    let beats = [
        Beat::Tick(80),
        Beat::Join(1),
        Beat::Tick(81),
        // The malformed request arrives beside a valid one: the valid timer of
        // that batch must not survive it.
        Beat::Join(2),
        Beat::Tick(82),
        Beat::Join(3),
        Beat::Tick(83),
    ];
    for script in [
        "malformed-id-empty",
        "malformed-id-long",
        "malformed-delay-zero",
        "malformed-delay-past-bound",
    ] {
        let (lines, _) = drive(
            &[("hello", script, 0)],
            PluginLimits::default(),
            HostQueues::default(),
            &beats,
        )
        .await;
        // Only the batch before the malformed one published, and no later batch
        // of that package did: the guest's own malformed request retired it.
        assert_eq!(
            lines,
            expected(&[("hello", "probe-join:81:81")]),
            "script {script}"
        );
    }
}

#[tokio::test]
async fn the_contract_s_own_id_and_delay_bounds_are_accepted_at_their_edges() {
    let beats = [
        Beat::Tick(80),
        Beat::Join(1),
        Beat::Tick(81),
        Beat::Tick(82),
        Beat::Join(2),
        Beat::Tick(83),
    ];
    let (lines, _) = drive(
        &[("hello", "boundary-limits", 0)],
        PluginLimits::default(),
        HostQueues::default(),
        &beats,
    )
    .await;
    // A 64-byte id is inside the script-id bound and fires; a delay of
    // 630720000 ticks is inside the delay bound and is still waiting, which the
    // later batch proves by being admitted at all.
    assert_eq!(
        lines,
        vec![
            ("hello".to_owned(), format!("{}:81:81", "b".repeat(64))),
            ("hello".to_owned(), "after:83:83".to_owned()),
        ]
    );
}
