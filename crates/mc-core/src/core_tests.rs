//! Reliable-outbox tests: real `Core`, tiny cap. Either every change arrives
//! in order, or the last change is `Disconnect` — never a gap.

use std::collections::HashMap;

use crate::boundary::{ConnectionId, PlayerChange, PlayerInput};
use crate::config::CoreConfig;
use crate::core::Core;

fn test_config(dir: std::path::PathBuf, outbox_cap: usize) -> CoreConfig {
    let mut config = CoreConfig::new(dir, 42);
    config.player_outbox_cap = outbox_cap;
    // No checkpoints during the test: fewer moving parts.
    config.checkpoint_every_ticks = u64::MAX;
    config
}

fn open_core(outbox_cap: usize) -> (Core, std::path::PathBuf) {
    // Thread id: both tests open a cap-2 core in parallel; a shared dir lets
    // one test delete the other's live journal, which halts that tick thread
    // and leaves its `blocking_recv` waiting forever.
    let dir = std::env::temp_dir().join(format!(
        "mc-core-outbox-{}-{}-{:?}",
        outbox_cap,
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let core = Core::open(test_config(dir.clone(), outbox_cap)).expect("open");
    (core, dir)
}

/// Join one player, drain the outbox for `ticks` broadcast ticks, and return
/// every change per connection plus the last snapshot seen.
fn join_and_drain(
    core: &Core,
    connection: ConnectionId,
    ticks: usize,
) -> (
    HashMap<ConnectionId, Vec<PlayerChange>>,
    crate::boundary::TickOutput,
) {
    let mut rx = core.subscribe();
    core.submit(PlayerInput::Join {
        connection,
        uuid: uuid::Uuid::from_u128(0xCAFE_F00D_DEAD_BEEF_0123_4567_89AB_CDEF),
        name: "probe".to_owned(),
        permission_level: 0,
    });
    let mut drained: HashMap<ConnectionId, Vec<PlayerChange>> = HashMap::new();
    let mut last = None;
    for _ in 0..ticks {
        match rx.blocking_recv() {
            Ok(output) => {
                last = Some(output);
                for (conn, queue) in core.try_drain_outbox() {
                    drained.entry(conn).or_default().extend(queue);
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                for (conn, queue) in core.try_drain_outbox() {
                    drained.entry(conn).or_default().extend(queue);
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                panic!("tick thread died")
            }
        }
    }
    // Final sweep: anything published after the last broadcast.
    for (conn, queue) in core.try_drain_outbox() {
        drained.entry(conn).or_default().extend(queue);
    }
    (drained, last.expect("at least one tick"))
}

fn is_disconnect(change: &PlayerChange) -> bool {
    matches!(change, PlayerChange::Disconnect { .. })
}

/// Tiny cap: the join burst overflows, so the queue is sealed with a terminal
/// `Disconnect` — and the pre-seal prefix matches the uncapped core exactly
/// (no gap, in order).
#[test]
fn overflow_seals_with_terminal_disconnect_and_no_gap() {
    let connection = ConnectionId(7);
    let (small, small_dir) = open_core(2);
    let (big, big_dir) = open_core(512);
    let (small_drained, _) = join_and_drain(&small, connection, 5);
    let (big_drained, _) = join_and_drain(&big, connection, 5);

    let big_queue = big_drained.get(&connection).cloned().unwrap_or_default();
    assert!(
        big_queue.len() >= 4 && !big_queue.iter().any(is_disconnect),
        "uncapped join burst arrives whole ({} changes)",
        big_queue.len()
    );
    let small_queue = small_drained.get(&connection).cloned().unwrap_or_default();
    assert!(
        small_queue.len() >= 2 && is_disconnect(small_queue.last().expect("non-empty")),
        "capped queue ends with Disconnect: {small_queue:?}"
    );
    let prefix = &small_queue[..small_queue.len() - 1];
    assert_eq!(
        prefix,
        &big_queue[..prefix.len()],
        "pre-seal prefix is gapless and ordered"
    );

    small.shutdown();
    big.shutdown();
    let _ = std::fs::remove_dir_all(small_dir);
    let _ = std::fs::remove_dir_all(big_dir);
}

/// After the seal, nothing more is ever queued for the connection, and the
/// player is removed Leave-equivalently (gone from a later snapshot).
#[test]
fn sealed_connection_queues_nothing_more_and_player_leaves() {
    let connection = ConnectionId(9);
    let (core, dir) = open_core(2);
    let (drained, _) = join_and_drain(&core, connection, 4);
    let queue = drained.get(&connection).cloned().unwrap_or_default();
    assert!(queue.iter().any(is_disconnect), "Disconnect was sealed");

    // Keep draining: sealed queues accept nothing further.
    let mut rx = core.subscribe();
    for _ in 0..6 {
        match rx.blocking_recv() {
            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                panic!("tick thread died")
            }
        }
    }
    let mut late = Vec::new();
    for (conn, changes) in core.try_drain_outbox() {
        if conn == connection {
            late.extend(changes);
        }
    }
    // At most the (already counted) sealed queue may still be in transit;
    // re-drain until empty, then nothing new may appear.
    assert!(
        late.iter().filter(|change| !is_disconnect(change)).count() == 0 || late.is_empty(),
        "nothing queued after Disconnect: {late:?}"
    );

    // Leave-equivalent removal: the player entity is gone from the snapshot.
    let mut seen_player = false;
    let mut rx = core.subscribe();
    for _ in 0..4 {
        if let Ok(output) = rx.blocking_recv() {
            seen_player = output
                .snapshot
                .entities
                .iter()
                .any(|view| view.appearance.custom_name.as_deref() == Some("probe"));
            if !seen_player {
                break;
            }
        }
    }
    assert!(!seen_player, "sealed player removed from the snapshot");

    core.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

fn contract_test_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("mc-core-contract-{}", uuid::Uuid::new_v4()))
}

#[test]
fn new_world_persists_generation_contract_and_matching_revision_reopens() {
    let dir = contract_test_dir();
    let config = test_config(dir.clone(), 512);
    Core::open(config.clone())
        .expect("fresh world opens")
        .shutdown();
    let identity = crate::native_checkpoint::native_identity(&config);
    let read_generation = || {
        let store = mc_world::native::NativeReadOnlyStore::open_with_cache(
            &dir.join("native/state.redb"),
            identity,
            16 * 1024 * 1024,
        )
        .expect("native header records the configured generation identity");
        let image = store
            .checkpoint_reader()
            .unwrap()
            .read_checkpoint()
            .unwrap();
        let generation = &image.globals["generation"];
        assert_eq!(generation.version, 1);
        assert_eq!(generation.captured_tick, image.tick);
        assert_eq!(
            generation.payload,
            crate::native_checkpoint::generation_records(&config).unwrap()["generation"].payload
        );
        let contract: serde_json::Value = serde_json::from_slice(&generation.payload).unwrap();
        assert_eq!(contract["identity"], serde_json::json!(identity));
        assert_eq!(contract["base"]["seed"], config.seed);
        assert_eq!(contract["base"]["revision"], mc_worldgen::WORLDGEN_REVISION);
        assert_eq!(contract["generators"]["0"]["generator"], "vanilla_like");
        assert_eq!(
            contract["generators"]["0"]["min_y"],
            mc_world::OVERWORLD_GEOMETRY.min_y()
        );
        assert_eq!(
            contract["generators"]["0"]["height"],
            mc_world::OVERWORLD_GEOMETRY.height()
        );
        generation.payload.clone()
    };
    let generation = read_generation();
    Core::open(config.clone())
        .expect("matching revision reopens")
        .shutdown();
    assert_eq!(read_generation(), generation);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn mismatched_seed_refuses_before_generation_or_recovery() {
    let dir = contract_test_dir();
    let config = test_config(dir.clone(), 512);
    Core::open(config.clone()).unwrap().shutdown();
    let before = crate::native_checkpoint::tests::read_manifest(&config)
        .unwrap()
        .tick;
    let mut changed = config.clone();
    changed.seed += 1;
    let error = Core::open(changed).unwrap_err().to_string();
    assert_eq!(error, "storage: invalid native record: identity mismatch");
    assert_eq!(
        crate::native_checkpoint::tests::read_manifest(&config)
            .unwrap()
            .tick,
        before
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn generation_global_revision_mismatch_refuses_without_advancing_checkpoint() {
    let dir = contract_test_dir();
    let config = test_config(dir.clone(), 512);
    // Seed the mismatched descriptor in the first commit: native generation
    // descriptors cannot be mutated once a checkpoint has been committed.
    let template_dir = contract_test_dir();
    let template_config = test_config(template_dir.clone(), 512);
    crate::native_checkpoint::tests::write_manifest(&template_config, &Default::default()).unwrap();
    let template = crate::native_checkpoint::tests::fixture_store(&template_config).unwrap();
    let mut image = template.read_checkpoint().unwrap();
    drop(template);
    std::fs::remove_dir_all(template_dir).unwrap();
    let path = dir.join("native/state.redb");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let store = mc_world::native::NativeStore::create(
        &path,
        crate::native_checkpoint::native_identity(&config),
    )
    .unwrap();
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let mut generation: serde_json::Value =
        serde_json::from_slice(&image.globals["generation"].payload).unwrap();
    generation["base"]["revision"] = (mc_worldgen::WORLDGEN_REVISION - 1).into();
    image.globals.get_mut("generation").unwrap().payload = serde_json::to_vec(&generation).unwrap();
    store
        .commit_checkpoint(
            mc_world::native::NativeCheckpoint {
                tick: image.tick,
                previous_tick: 0,
                chunks: &[],
                entities: &image.entities,
                players: &[],
                globals: &image.globals,
            },
            &registry.blocks,
            &registry.items,
        )
        .unwrap();
    drop(store);
    let error = Core::open(config.clone()).unwrap_err().to_string();
    assert_eq!(
        error,
        "worldgen: native generation: configured generation identity mismatch"
    );
    assert_eq!(
        crate::native_checkpoint::tests::read_manifest(&config)
            .unwrap()
            .tick,
        image.tick
    );
    assert!(!dir.join("journal").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unversioned_v2_history_refuses_without_stamping_current_revision() {
    for history_file in [
        "CHECKPOINT-MANIFEST.json",
        "journal/ticks-1-1.json",
        "region/r.0.0.mca",
        "level.dat",
    ] {
        let dir = contract_test_dir();
        let history_path = dir.join(history_file);
        std::fs::create_dir_all(history_path.parent().unwrap()).unwrap();
        let history = b"existing history must remain untouched";
        std::fs::write(&history_path, history).unwrap();
        let mut config = test_config(dir.clone(), 512);
        config.vanilla_data_dir = Some(dir.join("missing-content"));
        let error = Core::open(config).unwrap_err().to_string();
        assert_eq!(
            error,
            format!(
                "storage: {} has persisted history but no native checkpoint; use solaris-v2 world import-anvil --source ANVIL --destination NATIVE --receipt JSON",
                dir.display(),
            )
        );
        assert!(!dir.join("native").exists());
        assert_eq!(std::fs::read(&history_path).unwrap(), history);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
#[ignore = "requires the local licensed vanilla content cache"]
fn canonical_startup_loads_modern_jigsaw_and_nether_bastions() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = test_config(directory.path().to_path_buf(), 512);
    config.vanilla_data_dir = Some(
        std::env::var_os("SOLARIS_CONTENT_CACHE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
            }),
    );
    // Exercise the serving path, including all three modern Overworld
    // closures and the Nether bastion closure, not a synthetic plan source.
    Core::open(config)
        .expect("canonical startup loads the shipped jigsaw resources")
        .shutdown();
}

#[test]
fn scoreboard_survives_core_checkpoint_and_recovery_and_syncs_rejoining_viewer() {
    use crate::scoreboard::ScoreboardEvent;
    let (core, dir) = open_core(512);
    join_and_drain(&core, ConnectionId(70), 2);
    let mut rx = core.subscribe();
    for command in [
        "team add allies",
        "team join allies probe",
        "team modify allies friendlyFire false",
        "scoreboard objectives add wins dummy",
        "scoreboard players set probe wins 42",
        "scoreboard objectives setdisplay sidebar wins",
    ] {
        core.submit(PlayerInput::Command {
            connection: ConnectionId(70),
            command: command.to_owned(),
            permission_level: 4,
        });
    }
    let mut applied = false;
    for _ in 0..100 {
        rx.blocking_recv().expect("live core tick");
        let changes = core.try_drain_outbox();
        applied |= changes.iter().flat_map(|(_, queue)| queue).any(|change| matches!(change, PlayerChange::Scoreboard { event: ScoreboardEvent::Display { slot: 1, objective }, .. } if objective == "wins"));
        if applied {
            break;
        }
    }
    assert!(applied, "scoreboard commands committed");
    core.shutdown();
    let config = test_config(dir.clone(), 512);
    {
        let store = mc_world::native::NativeReadOnlyStore::open_with_cache(
            &dir.join("native/state.redb"),
            crate::native_checkpoint::native_identity(&config),
            16 * 1024 * 1024,
        )
        .expect("clean shutdown leaves a readable native checkpoint");
        let image = store
            .checkpoint_reader()
            .unwrap()
            .read_checkpoint()
            .unwrap();
        let record = &image.globals["scoreboard"];
        assert_eq!(record.version, 1);
        assert_eq!(record.captured_tick, image.tick);
        let tag = crate::native_checkpoint::decode_tag(record, "scoreboard").unwrap();
        let scoreboard = crate::scoreboard::Scoreboard::from_nbt(&tag).unwrap();
        assert_eq!(scoreboard.scores["probe"]["wins"].value, 42);
        assert_eq!(scoreboard.display_slots[&1], "wins");
        assert!(!scoreboard.teams["allies"].friendly_fire);
        assert!(scoreboard.teams["allies"].members.contains("probe"));
    }
    for path in [
        "data/minecraft/scoreboard.dat",
        "region",
        "entities",
        "playerdata",
        "level.dat",
        "dimensions",
        "CHECKPOINT-MANIFEST.json",
    ] {
        assert!(
            !dir.join(path).exists(),
            "runtime emitted Anvil output: {path}"
        );
    }
    let restored = Core::open(config).expect("recover scoreboard");
    let (drained, _) = join_and_drain(&restored, ConnectionId(71), 2);
    let events: Vec<_> = drained[&ConnectionId(71)]
        .iter()
        .filter_map(|change| match change {
            PlayerChange::Scoreboard { event, .. } => Some(event),
            _ => None,
        })
        .collect();
    assert!(events.iter().any(|event| matches!(event, ScoreboardEvent::Score { owner, objective, score: 42 } if owner == "probe" && objective == "wins")));
    assert!(events.iter().any(|event| matches!(event, ScoreboardEvent::Display { slot: 1, objective } if objective == "wins")));
    assert!(events.iter().any(|event| matches!(event, ScoreboardEvent::Team { name, method: 0, team: Some(team), members } if name == "allies" && !team.friendly_fire && members.contains(&"probe".to_owned()))));
    restored.shutdown();
    std::fs::remove_dir_all(dir).expect("remove test world");
}

#[test]
fn command_storage_survives_native_checkpoint_and_reopen() {
    let (core, dir) = open_core(512);
    join_and_drain(&core, ConnectionId(70), 2);
    let mut rx = core.subscribe();
    core.submit(PlayerInput::Command {
        connection: ConnectionId(70),
        command: "data merge storage test:map {value:37}".into(),
        permission_level: 4,
    });
    let mut applied = false;
    for _ in 0..100 {
        rx.blocking_recv().expect("live core tick");
        applied |= core.try_drain_outbox().iter().flat_map(|(_, queue)| queue)
            .any(|change| matches!(change, PlayerChange::Chat { message } if message == "Storage data updated"));
        if applied {
            break;
        }
    }
    assert!(applied);
    core.shutdown();
    let config = test_config(dir.clone(), 512);
    let store = mc_world::native::NativeReadOnlyStore::open_with_cache(
        &dir.join("native/state.redb"),
        crate::native_checkpoint::native_identity(&config),
        16 * 1024 * 1024,
    )
    .unwrap();
    let image = store
        .checkpoint_reader()
        .unwrap()
        .read_checkpoint()
        .unwrap();
    let tag =
        crate::native_checkpoint::decode_tag(&image.globals["command_storage"], "command_storage")
            .unwrap();
    let storage = crate::command_storage::from_tag(tag).unwrap();
    assert_eq!(
        *crate::command_nbt_path::get(&storage["test:map"], "value").unwrap(),
        mc_nbt::Tag::Int(37)
    );
    drop(store);
    let restored = Core::open(config).expect("recover command storage");
    restored.shutdown();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn bossbars_survive_core_checkpoint_and_reopen() {
    let (core, dir) = open_core(512);
    join_and_drain(&core, ConnectionId(72), 2);
    let mut rx = core.subscribe();
    for command in [
        "bossbar add solaris:probe \"Probe\"",
        "bossbar set solaris:probe value 42",
    ] {
        core.submit(PlayerInput::Command {
            connection: ConnectionId(72),
            command: command.to_owned(),
            permission_level: 4,
        });
    }
    for _ in 0..20 {
        rx.blocking_recv().expect("live core tick");
        core.try_drain_outbox();
    }
    core.shutdown();
    let config = test_config(dir.clone(), 512);
    let stored_value = || {
        let store = mc_world::native::NativeReadOnlyStore::open_with_cache(
            &dir.join("native/state.redb"),
            crate::native_checkpoint::native_identity(&config),
            16 * 1024 * 1024,
        )
        .expect("clean shutdown leaves a readable native checkpoint");
        let image = store
            .checkpoint_reader()
            .unwrap()
            .read_checkpoint()
            .unwrap();
        let tag =
            crate::native_checkpoint::decode_tag(&image.globals["bossbars"], "bossbars").unwrap();
        crate::bossbar::BossBars::from_nbt(&tag).unwrap().bars["solaris:probe"].value
    };
    assert_eq!(stored_value(), 42);
    let restored = Core::open(config.clone()).expect("recover bossbars");
    restored.shutdown();
    assert_eq!(
        stored_value(),
        42,
        "bar survives a recover and re-checkpoint"
    );
    std::fs::remove_dir_all(dir).expect("remove test world");
}
