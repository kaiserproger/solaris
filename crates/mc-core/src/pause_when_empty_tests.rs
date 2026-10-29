//! Empty-server pause (vanilla `pause-when-empty-seconds`): the world stops
//! advancing after the idle grace period, saves still work, and a join
//! resumes ticking.

use std::time::Duration;

use crate::boundary::{ConnectionId, PlayerInput, TickOutput};
use crate::config::CoreConfig;
use crate::core::Core;

fn config(name: &str, pause_seconds: u32) -> (CoreConfig, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("mc-core-pause-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut config = CoreConfig::new(dir.clone(), 42);
    config.view_distance = 2;
    config.simulation_distance = 0;
    config.pause_when_empty_seconds = pause_seconds;
    // Keep the test off the checkpoint cadence (explicit saves are tested).
    config.checkpoint_every_ticks = u64::MAX;
    (config, dir)
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("test runtime")
}

/// Drain broadcasts until the channel goes quiet (a paused world) or 40 ticks
/// arrived. Returns the highest observed tick.
fn drain_until_idle(
    rt: &tokio::runtime::Runtime,
    rx: &mut tokio::sync::broadcast::Receiver<TickOutput>,
) -> u64 {
    rt.block_on(async {
        let mut last = 0;
        for _ in 0..40 {
            match tokio::time::timeout(Duration::from_millis(250), rx.recv()).await {
                Ok(Ok(output)) => last = output.tick,
                _ => break,
            }
        }
        last
    })
}

fn next_tick(
    rt: &tokio::runtime::Runtime,
    rx: &mut tokio::sync::broadcast::Receiver<TickOutput>,
    wait: Duration,
) -> Option<TickOutput> {
    rt.block_on(async { tokio::time::timeout(wait, rx.recv()).await.ok()?.ok() })
}

#[test]
fn empty_server_stops_ticking_and_a_join_resumes() {
    let (config, dir) = config("resume", 1);
    let core = Core::open(config).expect("open");
    let rt = runtime();
    let mut rx = core.subscribe();

    // One second of grace = 20 ticks, then the world pauses.
    let paused_at = drain_until_idle(&rt, &mut rx);
    assert!(
        paused_at >= 19,
        "the world ticked through its grace period before pausing (tick {paused_at})"
    );
    // No further world ticks arrive while empty: the channel stays quiet.
    assert!(
        next_tick(&rt, &mut rx, Duration::from_millis(400)).is_none(),
        "a paused world must not publish ticks"
    );

    // Saves keep being serviced while paused (vanilla tickConnection path).
    assert!(core.checkpoint(), "checkpoint while paused");

    // A join wakes the world and continuous ticking resumes.
    core.submit(PlayerInput::Join {
        connection: ConnectionId(1),
        uuid: uuid::Uuid::new_v4(),
        name: "resumer".to_owned(),
        permission_level: 0,
    });
    let resumed =
        next_tick(&rt, &mut rx, Duration::from_secs(2)).expect("the join resumes ticking");
    assert!(resumed.tick > paused_at, "tick advances after the join");
    let after_join = drain_until_idle(&rt, &mut rx);
    assert!(
        after_join >= resumed.tick + 5,
        "a player online keeps the world ticking (tick {after_join})"
    );

    core.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn zero_seconds_never_pauses() {
    let (config, dir) = config("disabled", 0);
    let core = Core::open(config).expect("open");
    let rt = runtime();
    let mut rx = core.subscribe();
    let last = drain_until_idle(&rt, &mut rx);
    assert!(
        last >= 25,
        "0 disables the pause: the world keeps ticking while empty (tick {last})"
    );
    core.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn singleplayer_pause_stops_world_until_resume() {
    let (config, dir) = config("singleplayer", 0);
    let core = Core::open(config).expect("open");
    let rt = runtime();
    let mut rx = core.subscribe();
    let connection = ConnectionId(1);
    core.submit(PlayerInput::Join {
        connection,
        uuid: uuid::Uuid::new_v4(),
        name: "owner".to_owned(),
        permission_level: 4,
    });
    let joined = next_tick(&rt, &mut rx, Duration::from_secs(2)).expect("join tick");
    let joined = next_tick(&rt, &mut rx, Duration::from_secs(2)).unwrap_or(joined);
    core.submit(PlayerInput::Command {
        connection,
        command: "solaris_pause on".into(),
        permission_level: 4,
    });
    let paused_at = drain_until_idle(&rt, &mut rx).max(joined.tick);
    assert!(
        next_tick(&rt, &mut rx, Duration::from_millis(400)).is_none(),
        "world ticks must stop while paused"
    );
    assert!(core.checkpoint(), "checkpoint while paused");
    core.submit(PlayerInput::Command {
        connection,
        command: "solaris_pause off".into(),
        permission_level: 4,
    });
    let resumed = next_tick(&rt, &mut rx, Duration::from_secs(2)).expect("resume tick");
    assert_eq!(resumed.tick, paused_at + 1);
    core.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn singleplayer_pause_requires_one_operator_and_join_resumes() {
    let (config, dir) = config("singleplayer-authorization", 0);
    let core = Core::open(config).expect("open");
    let rt = runtime();
    let mut rx = core.subscribe();
    let owner = ConnectionId(1);
    core.submit(PlayerInput::Join {
        connection: owner,
        uuid: uuid::Uuid::new_v4(),
        name: "owner".into(),
        permission_level: 4,
    });
    next_tick(&rt, &mut rx, Duration::from_secs(2)).expect("join");
    next_tick(&rt, &mut rx, Duration::from_secs(2)).expect("joined");
    core.submit(PlayerInput::Command {
        connection: owner,
        command: "solaris_pause on".into(),
        permission_level: 0,
    });
    assert!(next_tick(&rt, &mut rx, Duration::from_secs(2)).is_some());
    assert!(
        next_tick(&rt, &mut rx, Duration::from_secs(2)).is_some(),
        "non-operator cannot pause"
    );
    core.submit(PlayerInput::Command {
        connection: owner,
        command: "solaris_pause on".into(),
        permission_level: 4,
    });
    drain_until_idle(&rt, &mut rx);
    assert!(next_tick(&rt, &mut rx, Duration::from_millis(400)).is_none());
    core.submit(PlayerInput::Join {
        connection: ConnectionId(2),
        uuid: uuid::Uuid::new_v4(),
        name: "guest".into(),
        permission_level: 0,
    });
    next_tick(&rt, &mut rx, Duration::from_secs(2)).expect("guest wakes world");
    next_tick(&rt, &mut rx, Duration::from_secs(2)).expect("guest joined");
    core.submit(PlayerInput::Command {
        connection: owner,
        command: "solaris_pause on".into(),
        permission_level: 4,
    });
    assert!(next_tick(&rt, &mut rx, Duration::from_secs(2)).is_some());
    assert!(
        next_tick(&rt, &mut rx, Duration::from_secs(2)).is_some(),
        "two players cannot pause the world"
    );
    core.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
